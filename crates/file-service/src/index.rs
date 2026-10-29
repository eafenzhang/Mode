//! 工作区文件索引：扫描、排序、列式打包与 60s 指纹缓存（`fileService.ts` 的索引段）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio::fs;
use tokio::sync::{Mutex as AsyncMutex, OnceCell};

use crate::codec::pack_entries;
use crate::error::{file_result, FileResult};
use crate::filter::workspace_file_search_decision;
use crate::ignore::{
    is_workspace_file_search_path_ignored, load_workspace_file_search_ignore_rules, IgnoreRules,
    WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME,
};
use crate::search::WorkspaceFileSearchCandidate;
use crate::types::{EntryType, WorkspaceFileEntry};

const CACHE_TTL: Duration = Duration::from_secs(60);
const SCAN_CONCURRENCY: usize = 8;
const CACHE_MAX_ENTRIES: usize = 4;

pub struct CachedWorkspaceFileIndex {
    pub packed: String,
    /// 搜索候选只在第一次搜索时解码（对应 TS 的 `index.candidates`）。
    pub candidates: OnceCell<Vec<WorkspaceFileSearchCandidate>>,
}

struct CacheEntry {
    at: Instant,
    signature: String,
    index: Arc<CachedWorkspaceFileIndex>,
}

#[derive(Default)]
pub struct WorkspaceFileIndex {
    cache: std::sync::Mutex<HashMap<String, CacheEntry>>,
    /// 同一 workspace 的并发扫描串行化；不同 workspace 互不阻塞。
    key_locks: std::sync::Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl WorkspaceFileIndex {
    /// 取索引；命中缓存的条件是同一 key + 同一 `rootPath` + `.modeignore` 指纹且未过期。
    /// `refresh` 跳过缓存（仍在同一把 key 锁下排队）。
    pub async fn ensure(
        &self,
        root_path: &str,
        workspace_identity: Option<&str>,
        refresh: bool,
        on_warn: Option<&(dyn Fn(&str) + Send + Sync)>,
    ) -> FileResult<Arc<CachedWorkspaceFileIndex>> {
        let key = workspace_identity
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(root_path)
            .to_string();
        let key_lock = {
            let mut locks = self.key_locks.lock().unwrap();
            Arc::clone(
                locks
                    .entry(key.clone())
                    .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
            )
        };
        let _guard = key_lock.lock().await;

        let signature = format!("{root_path}\n{}", ignore_fingerprint(root_path).await);
        {
            let mut cache = self.cache.lock().unwrap();
            cache.retain(|_, entry| entry.at.elapsed() < CACHE_TTL);
            if !refresh {
                if let Some(entry) = cache.get(&key) {
                    if entry.signature == signature {
                        return Ok(Arc::clone(&entry.index));
                    }
                }
            }
        }

        let outcome = load_workspace_file_search_ignore_rules(Path::new(root_path)).await;
        if let Some(warning) = outcome.warning.as_deref() {
            if let Some(on_warn) = on_warn {
                on_warn(&format!("[file-service] {warning}"));
            }
        }
        let rules = Arc::new(outcome.rules);
        let entries = scan_workspace(root_path, &rules).await?;
        let index = Arc::new(CachedWorkspaceFileIndex {
            packed: pack_entries(&entries),
            candidates: OnceCell::new(),
        });

        let mut cache = self.cache.lock().unwrap();
        cache.insert(
            key,
            CacheEntry {
                at: Instant::now(),
                signature,
                index: Arc::clone(&index),
            },
        );
        while cache.len() > CACHE_MAX_ENTRIES {
            let oldest = cache
                .iter()
                .min_by_key(|(_, entry)| entry.at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    cache.remove(&key);
                }
                None => break,
            }
        }
        Ok(index)
    }
}

async fn ignore_fingerprint(root_path: &str) -> String {
    let path = Path::new(root_path).join(WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME);
    match fs::metadata(path).await {
        Ok(metadata) => {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis())
                .unwrap_or(0);
            format!("{modified}:{}", metadata.len())
        }
        Err(_) => "none".to_string(),
    }
}

fn is_skippable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
    )
}

fn relative_posix(root: &Path, target: &Path) -> String {
    let relative = target.strip_prefix(root).unwrap_or(target);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// JS `localeCompare` 依赖运行环境 locale；这里用确定性口径（忽略大小写 → 原文 → 码点）。
pub(crate) fn compare_names(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

fn compare_entries(left: &WorkspaceFileEntry, right: &WorkspaceFileEntry) -> std::cmp::Ordering {
    match (left.r#type, right.r#type) {
        (EntryType::Directory, EntryType::File) => std::cmp::Ordering::Less,
        (EntryType::File, EntryType::Directory) => std::cmp::Ordering::Greater,
        _ => compare_names(&left.relative_path, &right.relative_path),
    }
}

async fn scan_workspace(
    root_path: &str,
    rules: &Arc<IgnoreRules>,
) -> FileResult<Vec<WorkspaceFileEntry>> {
    let root = PathBuf::from(root_path);
    let pending = Arc::new(AsyncMutex::new(vec![root.clone()]));
    let mut set = tokio::task::JoinSet::new();

    for _ in 0..SCAN_CONCURRENCY {
        let pending = Arc::clone(&pending);
        let root = root.clone();
        let rules = Arc::clone(rules);
        set.spawn(async move { scan_worker(&root, &pending, &rules).await });
    }

    let mut entries = Vec::new();
    let mut first_error: Option<crate::error::FileServiceError> = None;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(mut local)) => entries.append(&mut local),
            Ok(Err(error)) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(file_result(format!("workspace scan task failed: {error}")));
                }
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    entries.sort_by(compare_entries);
    Ok(entries)
}

async fn scan_worker(
    root: &Path,
    pending: &AsyncMutex<Vec<PathBuf>>,
    rules: &Arc<IgnoreRules>,
) -> FileResult<Vec<WorkspaceFileEntry>> {
    let mut local = Vec::new();
    loop {
        let directory = {
            let mut queue = pending.lock().await;
            queue.pop()
        };
        let Some(directory) = directory else {
            break;
        };

        let mut reader = match fs::read_dir(&directory).await {
            Ok(reader) => reader,
            Err(error) if is_skippable(&error) => continue,
            Err(error) => return Err(file_result(error.to_string())),
        };

        while let Some(entry) = reader
            .next_entry()
            .await
            .map_err(|error| file_result(error.to_string()))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let relative_path = relative_posix(root, &path);
            if relative_path == WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME {
                continue;
            }

            let file_type = entry
                .file_type()
                .await
                .map_err(|error| file_result(error.to_string()))?;
            let is_symbolic_link = file_type.is_symlink();
            let entry_type = if file_type.is_dir() {
                EntryType::Directory
            } else if !is_symbolic_link {
                EntryType::File
            } else {
                match fs::metadata(&path).await {
                    Ok(metadata) if metadata.is_dir() => EntryType::Directory,
                    _ => EntryType::File,
                }
            };

            if is_workspace_file_search_path_ignored(rules.as_ref(), &relative_path, entry_type) {
                continue;
            }
            let decision =
                workspace_file_search_decision(&name, &relative_path, entry_type, true);
            if decision.include {
                local.push(WorkspaceFileEntry {
                    name,
                    path: path.to_string_lossy().into_owned(),
                    relative_path,
                    r#type: entry_type,
                });
            }
            if entry_type == EntryType::Directory && !is_symbolic_link && decision.traverse {
                pending.lock().await.push(path);
            }
        }
    }
    Ok(local)
}

/// UTF-16 码元数（JS `String.length`）。
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// 最近修改时间（毫秒，作为 `mtimeMs` 的近似）。
pub fn modified_ms(metadata: &std::fs::Metadata) -> f64 {
    match metadata.modified() {
        Ok(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs_f64() * 1000.0,
            Err(error) => -(error.duration().as_secs_f64() * 1000.0),
        },
        Err(_) => 0.0,
    }
}
