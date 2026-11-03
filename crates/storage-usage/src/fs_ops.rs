//! 数据根遍历与清理执行（真实文件系统）。
//!
//! 真源：`packages/services/src/storage/adapters/fsWalker.ts`、`fsCleaner.ts`。
//! TS 的有界并发与 `setImmediate` 让出是宿主事件循环的性能细节，不影响可观测
//! 结果（条目集合、错误码、计数、删除结果）；Rust 侧顺序遍历、取消用注入的
//! abort 探测。TS 逐条回调，Rust 侧由调用方传 `FnMut` 消费；`volumeProbe` 与
//! `rootsResolver` 依赖平台 statfs/网卡信息，仍归宿主。

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use indexmap::IndexMap;

use crate::catalog::{normalize_storage_relative_path, StorageCleanScope};
use crate::clean::StorageCleanCandidate;
use crate::scan_job::StoragePathError;
use crate::usage::StorageScanEntry;

fn io_error_code(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "ENOENT".to_string(),
        io::ErrorKind::PermissionDenied => "EACCES".to_string(),
        _ => "UNKNOWN".to_string(),
    }
}

fn path_error(path: &str, error: &io::Error) -> StoragePathError {
    StoragePathError {
        path: path.to_string(),
        code: io_error_code(error),
    }
}

fn mtime_ms(metadata: &std::fs::Metadata) -> i64 {
    // TS 的 mtimeMs 带亚毫秒精度；对拍侧取整到毫秒（向下取整口径一致）。
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn to_relative(root_path: &Path, absolute_path: &Path) -> String {
    match absolute_path.strip_prefix(root_path) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => String::new(),
    }
}

/// 遍历结果（TS `WalkStorageRootResult` + Rust 侧取消旗标）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WalkStorageRootResult {
    pub directories_scanned: usize,
    pub files_scanned: usize,
    /// 根目录本身不存在时为 true（视为空根，不算错误）。
    pub missing_root: bool,
    /// Rust 侧：abort 探测命中后提前返回（TS 侧以 AbortError 拒绝整个遍历）。
    pub aborted: bool,
}

/// 遍历单个数据根：不跟随符号链接；只产出常规文件。
/// 条目/错误经回调即时消费；`is_aborted` 在每个目录与每个条目前探测。
pub fn walk_storage_root(
    root_path: &Path,
    on_entry: &mut dyn FnMut(StorageScanEntry),
    on_error: &mut dyn FnMut(StoragePathError),
    is_aborted: &dyn Fn() -> bool,
) -> WalkStorageRootResult {
    let mut result = WalkStorageRootResult::default();
    let mut pending: Vec<PathBuf> = vec![root_path.to_path_buf()];
    while let Some(directory_path) = pending.pop() {
        if is_aborted() {
            result.aborted = true;
            return result;
        }
        let mut entries = match std::fs::read_dir(&directory_path) {
            Ok(entries) => entries,
            Err(error) => {
                if directory_path.as_path() == root_path && error.kind() == io::ErrorKind::NotFound
                {
                    result.missing_root = true;
                    continue;
                }
                on_error(path_error(&to_relative(root_path, &directory_path), &error));
                continue;
            }
        };
        result.directories_scanned += 1;
        let mut children: Vec<PathBuf> = Vec::new();
        for entry in entries.by_ref() {
            if is_aborted() {
                result.aborted = true;
                return result;
            }
            let Ok(entry) = entry else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            // 符号链接一律跳过（不跟随），与 TS `dirent.isSymbolicLink()` 一致。
            if file_type.is_symlink() {
                continue;
            }
            let entry_path = entry.path();
            if file_type.is_dir() {
                children.push(entry_path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let metadata = match std::fs::symlink_metadata(&entry_path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    // 扫描期间文件可能被日志轮转或 Agent 删除；ENOENT 属于正常竞态。
                    if error.kind() != io::ErrorKind::NotFound {
                        on_error(path_error(&to_relative(root_path, &entry_path), &error));
                    }
                    continue;
                }
            };
            if !metadata.is_file() {
                continue;
            }
            result.files_scanned += 1;
            on_entry(StorageScanEntry {
                relative_path: to_relative(root_path, &entry_path),
                bytes: metadata.len() as i64,
                mtime_ms: mtime_ms(&metadata),
            });
        }
        // TS 用 LIFO 栈（`pending.pop`）展开目录；Rust 保持同一展开顺序。
        pending.extend(children);
    }
    result
}

/// 浅层枚举：只列 `prefix` 目录第一层的常规文件；目录缺失/不可读返回空。
fn list_shallow(root_path: &Path, prefix: &str) -> Vec<StorageCleanCandidate> {
    let directory_path = root_path.join(prefix);
    let Ok(entries) = std::fs::read_dir(&directory_path) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        candidates.push(StorageCleanCandidate {
            relative_path: format!("{prefix}/{name}"),
            bytes: metadata.len() as i64,
            mtime_ms: mtime_ms(&metadata),
        });
    }
    candidates
}

/// 递归枚举：以 `root_path/prefix` 为根走一遍 walker，产出加回前缀。
fn list_recursive(root_path: &Path, prefix: &str) -> Vec<StorageCleanCandidate> {
    let mut candidates = Vec::new();
    let scope_root = root_path.join(prefix);
    let _ = walk_storage_root(
        &scope_root,
        &mut |entry| {
            candidates.push(StorageCleanCandidate {
                relative_path: format!("{prefix}/{}", entry.relative_path),
                bytes: entry.bytes,
                mtime_ms: entry.mtime_ms,
            });
        },
        &mut |_| {},
        &|| false,
    );
    candidates
}

/// 枚举候选：递归/非递归两种范围，重叠范围按归一化路径去重（后者覆盖前者，
/// 位置保持首次出现），对应 TS `createFsStorageCleaner().listCandidates`。
pub fn list_storage_candidates(
    root_path: &Path,
    scopes: &[StorageCleanScope],
) -> Vec<StorageCleanCandidate> {
    let mut by_path: IndexMap<String, StorageCleanCandidate> = IndexMap::new();
    for scope in scopes {
        let list = if scope.recursive {
            list_recursive(root_path, &scope.prefix)
        } else {
            list_shallow(root_path, &scope.prefix)
        };
        for candidate in list {
            let key = normalize_storage_relative_path(&candidate.relative_path);
            by_path.insert(key, candidate);
        }
    }
    by_path.into_values().collect()
}

/// 逃逸判定：TS `relative(rootPath, absolutePath)` 含 `..` 段即拒绝；Rust 按
/// 归一化相对路径串判定（`..` 出现在开头或中间段）。常规 `a/../b` 重写在 TS
/// 会物理解析为合法子路径，Rust 会拒绝——候选来自根内遍历，不携带 `..`，
/// 夹具不覆盖该形态。Windows 盘符前缀（`C:/…`）在 TS `join` 后会被 relative
/// 判为逃逸，Rust 侧显式按盘符拒绝（`Path::join` 遇盘符会整体替换，必须先挡）。
fn is_outside_root(relative_path: &str) -> bool {
    let bytes = relative_path.as_bytes();
    let has_drive_prefix = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    has_drive_prefix
        || relative_path == ".."
        || relative_path.starts_with("../")
        || relative_path.contains("/../")
}

/// 删除目标并自底向上清理变空的父目录（保留 `keep_directories` 类别顶层目录）。
pub fn delete_storage_files(
    root_path: &Path,
    targets: &[StorageCleanCandidate],
    keep_directories: &[String],
) -> crate::service::StorageDeleteResult {
    let mut result = crate::service::StorageDeleteResult {
        deleted_count: 0,
        freed_bytes: 0,
        failures: Vec::new(),
    };
    let mut deleted: Vec<String> = Vec::new();
    for target in targets {
        let relative_path = normalize_storage_relative_path(&target.relative_path);
        if is_outside_root(&relative_path) {
            result.failures.push(StoragePathError {
                path: relative_path,
                code: "EOUTSIDE".to_string(),
            });
            continue;
        }
        let absolute_path = root_path.join(&relative_path);
        match std::fs::remove_file(&absolute_path) {
            Ok(()) => {
                result.deleted_count += 1;
                result.freed_bytes += target.bytes;
                deleted.push(relative_path);
            }
            Err(error) => {
                result.failures.push(path_error(&relative_path, &error));
            }
        }
    }
    prune_empty_parents(root_path, &deleted, keep_directories);
    result
}

/// 先处理最深的目录，父目录才有机会变空；只 `rmdir`（不递归），失败静默。
fn prune_empty_parents(
    root_path: &Path,
    deleted_relative_paths: &[String],
    keep_directories: &[String],
) {
    let keep: HashSet<&String> = keep_directories.iter().collect();
    let mut directories: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for path in deleted_relative_paths {
        let mut current = match parent_relative(path) {
            Some(parent) => parent,
            None => continue,
        };
        loop {
            if current.is_empty() || keep.contains(&current) {
                break;
            }
            if seen.insert(current.clone()) {
                directories.push(current.clone());
            }
            match parent_relative(&current) {
                Some(parent) => current = parent,
                None => break,
            }
        }
    }
    // TS 按段数降序（稳定排序）；Rust 同口径。
    // sort_by_key + Reverse：clippy 把纯 key 比较的 sort_by 判成 lint（-D warnings）。
    directories.sort_by_key(|path| std::cmp::Reverse(path.split('/').count()));
    for directory in directories {
        let _ = std::fs::remove_dir(root_path.join(&directory));
    }
}

/// 相对路径的父目录（`/` 分隔）；`a` 的父是 `""`（TS `dirname` → `.`，视为到顶）。
fn parent_relative(path: &str) -> Option<String> {
    let (parent, _) = path.rsplit_once('/')?;
    Some(parent.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// 本 crate 自带的临时目录（同 `data-root` 的做法，不引测试依赖）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let base = std::env::temp_dir().join(format!(
                "storage-usage-fsops-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            std::fs::create_dir_all(&base).expect("mkdir temp");
            Self(base)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write_file(path: &Path, size: usize) -> u64 {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, vec![0u8; size]).expect("write");
        let metadata = fs::symlink_metadata(path).expect("stat");
        metadata.len()
    }

    fn sort_entries(mut entries: Vec<StorageScanEntry>) -> Vec<StorageScanEntry> {
        entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        entries
    }

    fn collect_walk(
        root: &Path,
    ) -> (
        Vec<StorageScanEntry>,
        Vec<StoragePathError>,
        WalkStorageRootResult,
    ) {
        let mut entries = Vec::new();
        let mut errors = Vec::new();
        let result = walk_storage_root(
            root,
            &mut |entry| entries.push(entry),
            &mut |error| errors.push(error),
            &|| false,
        );
        (entries, errors, result)
    }

    #[test]
    fn walk_collects_regular_files_and_skips_symlinks() {
        let temp = TempDir::new();
        let root = temp.path();
        let a_bytes = write_file(&root.join("a.txt"), 3);
        write_file(&root.join("sub/nested/b.log"), 7);
        write_file(&root.join("sub/empty-marker"), 0);
        #[cfg(unix)]
        {
            if std::os::unix::fs::symlink(root.join("a.txt"), root.join("sub/link.txt")).is_ok() {
                // 链接文件与链接目录都不应产出条目。
                let _ = std::os::unix::fs::symlink(root.join("sub"), root.join("link-dir"));
            }
        }
        #[cfg(windows)]
        {
            let _ =
                std::os::windows::fs::symlink_file(root.join("a.txt"), root.join("sub/link.txt"));
        }
        let (entries, errors, result) = collect_walk(root);
        assert!(errors.is_empty());
        assert!(!result.missing_root);
        assert!(!result.aborted);
        let paths: Vec<String> = sort_entries(entries.clone())
            .into_iter()
            .map(|entry| entry.relative_path)
            .collect();
        assert_eq!(
            paths,
            vec![
                "a.txt".to_string(),
                "sub/empty-marker".to_string(),
                "sub/nested/b.log".to_string(),
            ]
        );
        let a_entry = entries
            .iter()
            .find(|entry| entry.relative_path == "a.txt")
            .expect("a.txt");
        assert_eq!(a_entry.bytes, a_bytes as i64);
        assert!(a_entry.mtime_ms > 0);
        assert_eq!(result.files_scanned, 3);
        assert!(result.directories_scanned >= 2);
    }

    #[test]
    fn walk_missing_root_is_empty_result() {
        let temp = TempDir::new();
        let (entries, errors, result) = collect_walk(&temp.path().join("absent"));
        assert!(entries.is_empty());
        assert!(errors.is_empty());
        assert!(result.missing_root);
        assert_eq!(result.files_scanned, 0);
    }

    #[test]
    fn walk_abort_flag_stops_traversal() {
        let temp = TempDir::new();
        let root = temp.path();
        write_file(&root.join("a.txt"), 1);
        write_file(&root.join("sub/b.txt"), 1);
        let mut entries = Vec::new();
        let result =
            walk_storage_root(root, &mut |entry| entries.push(entry), &mut |_| {}, &|| {
                true
            });
        assert!(result.aborted);
        assert!(entries.is_empty());
    }

    #[test]
    fn candidates_dedupe_overlapping_scopes_by_normalized_path() {
        let temp = TempDir::new();
        let root = temp.path();
        write_file(&root.join("cli/db/backup/x.db"), 5);
        write_file(&root.join("cli/db/backup/deep/y.db"), 6);
        write_file(&root.join("cli/db/top.db"), 2);
        let scopes = vec![
            StorageCleanScope {
                prefix: "cli/db".to_string(),
                recursive: false,
            },
            StorageCleanScope {
                prefix: "cli/db/backup".to_string(),
                recursive: true,
            },
        ];
        let mut candidates = list_storage_candidates(root, &scopes);
        candidates.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let paths: Vec<String> = candidates
            .iter()
            .map(|candidate| candidate.relative_path.clone())
            .collect();
        assert_eq!(
            paths,
            vec![
                "cli/db/backup/deep/y.db".to_string(),
                "cli/db/backup/x.db".to_string(),
                "cli/db/top.db".to_string(),
            ]
        );
    }

    #[test]
    fn delete_removes_files_reports_failures_and_prunes_empty_parents() {
        let temp = TempDir::new();
        let root = temp.path();
        write_file(&root.join("logs/a/1.log"), 10);
        write_file(&root.join("logs/a/2.log"), 20);
        write_file(&root.join("logs/keep.log"), 5);
        let outside = TempDir::new();
        let outside_file = outside.path().join("outside.txt");
        write_file(&outside_file, 1);
        let candidates = vec![
            StorageCleanCandidate {
                relative_path: "logs/a/1.log".to_string(),
                bytes: 10,
                mtime_ms: 0,
            },
            StorageCleanCandidate {
                relative_path: "logs/a/2.log".to_string(),
                bytes: 20,
                mtime_ms: 0,
            },
            StorageCleanCandidate {
                relative_path: "logs/gone.log".to_string(),
                bytes: 3,
                mtime_ms: 0,
            },
            StorageCleanCandidate {
                relative_path: outside_file.to_string_lossy().replace('\\', "/"),
                bytes: 1,
                mtime_ms: 0,
            },
        ];
        let result = delete_storage_files(root, &candidates, &["logs".to_string()]);
        assert_eq!(result.deleted_count, 2);
        assert_eq!(result.freed_bytes, 30);
        // 缺失文件 ENOENT + 逃逸路径 EOUTSIDE。
        assert!(result
            .failures
            .iter()
            .any(|failure| failure.path == "logs/gone.log" && failure.code == "ENOENT"));
        assert!(result
            .failures
            .iter()
            .any(|failure| failure.code == "EOUTSIDE"));
        // a/ 及其文件已删；keep.log 与 logs 目录保留。
        assert!(!root.join("logs/a").exists());
        assert!(root.join("logs/keep.log").exists());
    }

    #[test]
    fn delete_keeps_keep_directories_but_prunes_deeper_empty_dirs() {
        let temp = TempDir::new();
        let root = temp.path();
        write_file(&root.join("cli/db/backup/x.db"), 4);
        let candidates = vec![StorageCleanCandidate {
            relative_path: "cli/db/backup/x.db".to_string(),
            bytes: 4,
            mtime_ms: 0,
        }];
        let result = delete_storage_files(root, &candidates, &["cli/db".to_string()]);
        assert_eq!(result.deleted_count, 1);
        assert_eq!(result.freed_bytes, 4);
        assert!(!root.join("cli/db/backup").exists(), "变空的 backup 被清理");
        assert!(root.join("cli/db").exists(), "keepDirectories 保留");
        // TS 的剪枝集合在 keep 边界停止：cli 与 cli/db 从未被 rmdir。
        assert!(root.join("cli").exists());
    }

    #[test]
    fn prune_only_touches_parents_of_deleted_paths() {
        let temp = TempDir::new();
        let root = temp.path();
        write_file(&root.join("logs/a/1.log"), 1);
        write_file(&root.join("logs/other/2.log"), 1);
        let candidates = vec![StorageCleanCandidate {
            relative_path: "logs/a/1.log".to_string(),
            bytes: 1,
            mtime_ms: 0,
        }];
        let _ = delete_storage_files(root, &candidates, &["logs".to_string()]);
        assert!(!root.join("logs/a").exists());
        assert!(root.join("logs/other/2.log").exists(), "无关分支不受影响");
        assert!(root.join("logs").exists());
    }

    #[test]
    fn temp_path_helper_is_not_exported_and_paths_are_forward_slashed() {
        // 相对路径统一 `/` 分隔（Windows 上的目录分隔符转换）。
        let temp = TempDir::new();
        let root = temp.path();
        write_file(&root.join("x/y/z.txt"), 1);
        let (entries, _, _) = collect_walk(root);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].relative_path.contains('\\'));
    }
}
