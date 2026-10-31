//! 技能归档同步服务（候选扫描 / tar.gz 导出 / 导入）。
//!
//! 真源：`packages/services/src/skill-sync/skillSyncService.ts`（clean-room：只读本仓库 TS）。
//! 行为契约与已知差异见 `docs/specs/skill-sync-service.md`。
//!
//! 频道注册暂缓：导出结果的 `archive` 是嵌套 `Uint8Array`，Rust 侧 rpc-wire 的
//! Object 分支目前按不透明 JSON 承载（见 `docs/specs/rpc-wire-format.md`），
//! 需要先补嵌套字节还原或由适配器实现标记编解码。

mod archive;
mod paths;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub use paths::{normalize_skill_sync_relative_path, resolve_skill_sync_path_within};

use archive::{create_archive, extract_archive, ArchiveEntry};
use skills_service::{
    should_walk_skill_directory_entry, walk_skill_markdown_paths, MAX_SKILL_SCAN_DEPTH,
};

const SKILL_FILE_NAME: &str = "SKILL.md";
const MODE_DATA_ROOT_DIR_NAME: &str = ".mode";
const DEFAULT_MAX_ARCHIVE_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSyncCandidate {
    pub id: String,
    pub name: String,
    pub directory_name: String,
    pub description: String,
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateListResult {
    pub candidates: Vec<SkillSyncCandidate>,
    pub max_archive_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSkillStatus {
    pub directory_name: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteStatusResult {
    pub statuses: Vec<RemoteSkillStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedSkill {
    pub id: String,
    pub name: String,
    pub directory_name: String,
}

pub struct ExportResult {
    pub archive: Vec<u8>,
    pub archive_bytes: usize,
    pub skills: Vec<ExportedSkill>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResultItem {
    pub name: String,
    pub directory_name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub results: Vec<ImportResultItem>,
}

#[derive(Debug, Clone, Default)]
pub struct SkillSyncServiceOptions {
    pub home_dir: Option<PathBuf>,
    pub max_archive_bytes: Option<u64>,
}

pub struct SkillSyncService {
    home_dir: PathBuf,
    max_archive_bytes: u64,
}

impl SkillSyncService {
    pub fn new(options: SkillSyncServiceOptions) -> Self {
        let home_dir = options.home_dir.unwrap_or_else(resolve_user_home_dir);
        Self {
            home_dir,
            max_archive_bytes: options
                .max_archive_bytes
                .unwrap_or(DEFAULT_MAX_ARCHIVE_BYTES),
        }
    }

    fn user_mode_skill_root(&self) -> PathBuf {
        self.home_dir.join(MODE_DATA_ROOT_DIR_NAME).join("skills")
    }

    fn user_agents_skill_root(&self) -> PathBuf {
        self.home_dir.join(".agents").join("skills")
    }

    pub fn list_local_user_skill_candidates(&self) -> CandidateListResult {
        CandidateListResult {
            candidates: self.collect_user_skill_candidates(),
            max_archive_bytes: self.max_archive_bytes,
        }
    }

    pub fn list_remote_user_skill_statuses(
        &self,
        directory_names: &[String],
        skills: &[(String, String)],
    ) -> Result<RemoteStatusResult, String> {
        let root = self.user_mode_skill_root();
        let existing_by_name = self.user_skill_directory_path_by_name();
        let requested_by_directory: HashMap<String, String> = skills
            .iter()
            .filter_map(|(directory_name, name)| {
                normalize_skill_sync_relative_path(directory_name, "unsafe skill sync path")
                    .ok()
                    .map(|normalized| (normalized, name.clone()))
            })
            .collect();
        let mut statuses = Vec::new();
        for directory_name in directory_names {
            let normalized =
                normalize_skill_sync_relative_path(directory_name, "unsafe skill sync path")?;
            let path =
                resolve_skill_sync_path_within(&root, &normalized, "unsafe skill sync path")?;
            if path.exists() {
                statuses.push(RemoteSkillStatus {
                    directory_name: normalized,
                    exists: true,
                    path: Some(path.to_string_lossy().into_owned()),
                });
                continue;
            }
            let existing_path = requested_by_directory
                .get(&normalized)
                .and_then(|name| existing_by_name.get(&normalize_name_key(name)));
            statuses.push(match existing_path {
                Some(existing) => RemoteSkillStatus {
                    directory_name: normalized,
                    exists: true,
                    path: Some(existing.to_string_lossy().into_owned()),
                },
                None => RemoteSkillStatus {
                    directory_name: normalized,
                    exists: false,
                    path: None,
                },
            });
        }
        Ok(RemoteStatusResult { statuses })
    }

    pub fn export_skills_archive(&self, skill_ids: &[String]) -> Result<ExportResult, String> {
        let candidates = self.collect_user_skill_candidates();
        let by_id: HashMap<&str, &SkillSyncCandidate> = candidates
            .iter()
            .map(|candidate| (candidate.id.as_str(), candidate))
            .collect();
        let mut selected = Vec::new();
        for id in skill_ids {
            let Some(candidate) = by_id.get(id.as_str()) else {
                return Err(format!("skill sync candidate not found: {id}"));
            };
            selected.push((*candidate).clone());
        }
        let selected_bytes: u64 = selected.iter().map(|candidate| candidate.size_bytes).sum();
        if selected_bytes > self.max_archive_bytes {
            return Err(size_limit_error(
                selected_bytes,
                self.max_archive_bytes,
                "selected-content",
            ));
        }
        let entries: Vec<ArchiveEntry> = selected
            .iter()
            .map(|candidate| {
                let directory = Path::new(&candidate.path)
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                let source = plugin_discovery::js::realpath(&directory).unwrap_or(directory);
                ArchiveEntry {
                    source_path: source,
                    archive_path: candidate.directory_name.clone(),
                }
            })
            .collect();
        let archive = create_archive(&entries)?;
        if archive.len() as u64 > self.max_archive_bytes {
            return Err(size_limit_error(
                archive.len() as u64,
                self.max_archive_bytes,
                "archive",
            ));
        }
        Ok(ExportResult {
            archive_bytes: archive.len(),
            archive,
            skills: selected
                .iter()
                .map(|candidate| ExportedSkill {
                    id: candidate.id.clone(),
                    name: candidate.name.clone(),
                    directory_name: candidate.directory_name.clone(),
                })
                .collect(),
        })
    }

    pub fn check_remote_user_skill_write_access(&self) -> serde_json::Value {
        crate::write_access(&self.user_mode_skill_root())
    }

    pub fn import_skills_archive(
        &self,
        archive: &[u8],
        overwrite: bool,
    ) -> Result<ImportResult, String> {
        if overwrite {
            return Err("skill sync overwrite is not supported".to_string());
        }
        if archive.len() as u64 > self.max_archive_bytes {
            return Err(size_limit_error(
                archive.len() as u64,
                self.max_archive_bytes,
                "archive",
            ));
        }
        let target_root = self.user_mode_skill_root();
        std::fs::create_dir_all(&target_root).map_err(|error| error.to_string())?;
        let temp_root = target_root.join(format!(".sync-tmp-{}", unique_suffix()));
        std::fs::create_dir_all(&temp_root).map_err(|error| error.to_string())?;
        let result = (|| -> Result<ImportResult, String> {
            extract_archive(archive, &temp_root, Some(self.max_archive_bytes))?;
            let extracted = collect_extracted_skill_directories(&temp_root);
            let mut existing_by_name = self.user_skill_directory_path_by_name();
            let mut results = Vec::new();
            for (directory_name, source_path) in extracted {
                let target_path = resolve_skill_sync_path_within(
                    &target_root,
                    &directory_name,
                    "unsafe skill sync path",
                )?;
                let skill_path = source_path.join(SKILL_FILE_NAME);
                let Some(content) = read_skill_file_or_null(&skill_path) else {
                    results.push(ImportResultItem {
                        name: Path::new(&directory_name)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        directory_name,
                        status: "failed".to_string(),
                        path: None,
                        error: Some("SKILL.md is missing".to_string()),
                    });
                    continue;
                };
                let metadata = parse_skill_metadata(&content, &directory_name);
                let name_key = normalize_name_key(&metadata.0);
                if target_path.exists() {
                    results.push(ImportResultItem {
                        name: metadata.0,
                        directory_name,
                        status: "skipped".to_string(),
                        path: Some(target_path.to_string_lossy().into_owned()),
                        error: None,
                    });
                    continue;
                }
                if let Some(existing) = existing_by_name.get(&name_key) {
                    results.push(ImportResultItem {
                        name: metadata.0,
                        directory_name,
                        status: "skipped".to_string(),
                        path: Some(existing.to_string_lossy().into_owned()),
                        error: None,
                    });
                    continue;
                }
                let copy_result = (|| -> std::io::Result<()> {
                    if let Some(parent) = target_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    copy_directory_recursive(&source_path, &target_path)
                })();
                match copy_result {
                    Ok(()) => {
                        existing_by_name.insert(name_key, target_path.clone());
                        results.push(ImportResultItem {
                            name: metadata.0,
                            directory_name,
                            status: "synced".to_string(),
                            path: Some(target_path.to_string_lossy().into_owned()),
                            error: None,
                        });
                    }
                    Err(error) => results.push(ImportResultItem {
                        name: metadata.0,
                        directory_name,
                        status: "failed".to_string(),
                        path: Some(target_path.to_string_lossy().into_owned()),
                        error: Some(error.to_string()),
                    }),
                }
            }
            Ok(ImportResult { results })
        })();
        let _ = std::fs::remove_dir_all(&temp_root);
        result
    }

    fn collect_user_skill_candidates(&self) -> Vec<SkillSyncCandidate> {
        let mode_root = self.user_mode_skill_root();
        let raw_mode = collect_candidates_in_root(&mode_root);
        let mode_directory_names = collect_covered_skill_directory_names(&mode_root);
        let mode_name_keys: HashSet<String> = raw_mode
            .iter()
            .map(|candidate| normalize_name_key(&candidate.name))
            .collect();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut mode_candidates = dedupe_by_canonical_path(raw_mode, &mut seen);
        let agents_candidates_raw = collect_candidates_in_root(&self.user_agents_skill_root())
            .into_iter()
            .filter(|candidate| {
                !mode_directory_names.contains(&candidate.directory_name)
                    && !mode_name_keys.contains(&normalize_name_key(&candidate.name))
            })
            .collect();
        let mut agents_candidates = dedupe_by_canonical_path(agents_candidates_raw, &mut seen);
        mode_candidates.append(&mut agents_candidates);
        mode_candidates.sort_by(|left, right| {
            plugin_discovery::js::locale_compare(&left.directory_name, &right.directory_name)
        });
        mode_candidates
    }

    fn user_skill_directory_path_by_name(&self) -> HashMap<String, PathBuf> {
        let mut result = collect_skill_directory_path_by_name(&self.user_mode_skill_root());
        for (key, path) in collect_skill_directory_path_by_name(&self.user_agents_skill_root()) {
            result.entry(key).or_insert(path);
        }
        result
    }
}

/// `checkRemoteSyncDirectoryWriteAccess`：目录内写一个临时标记并删除。
fn write_access(directory: &Path) -> serde_json::Value {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let marker = directory.join(format!(
        ".mode-sync-preflight-{}-{nanos}.tmp",
        std::process::id()
    ));
    let path_text = directory.to_string_lossy().into_owned();
    let result = (|| -> Result<(), std::io::Error> {
        std::fs::create_dir_all(directory)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)?;
        use std::io::Write;
        file.write_all(b"ok")?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&marker);
    match result {
        Ok(()) => serde_json::json!({ "ok": true, "path": path_text }),
        Err(error) => {
            serde_json::json!({ "ok": false, "path": path_text, "error": error.to_string() })
        }
    }
}

fn resolve_user_home_dir() -> PathBuf {
    std::env::var("HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("USERPROFILE")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .map(PathBuf::from)
        .unwrap_or_else(plugin_discovery::js::node_homedir)
}

fn size_limit_error(actual: u64, max: u64, phase: &str) -> String {
    format!("skill sync size limit exceeded: {actual}/{max} ({phase})")
}

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

fn normalize_name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn candidate_id(directory_path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(directory_path.to_string_lossy().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_skill_file_or_null(path: &Path) -> Option<String> {
    match std::fs::read(path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => None,
    }
}

/// `parseSkillMetadata`：frontmatter 严格解析失败或缺失时回退目录名。
fn parse_skill_metadata(content: &str, fallback_name: &str) -> (String, String) {
    let Some(frontmatter) = split_frontmatter(content) else {
        return (fallback_name.to_string(), String::new());
    };
    let Some(scalars) = parse_top_level_scalars(&frontmatter) else {
        return (fallback_name.to_string(), String::new());
    };
    let name = scalars
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback_name.to_string());
    let description = scalars
        .get("description")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    (name, description)
}

fn split_frontmatter(content: &str) -> Option<String> {
    let normalized = content.replace("\r\n", "\n");
    // `/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/`
    if !normalized.starts_with("---\n") {
        return None;
    }
    let rest = &normalized[4..];
    let end = rest.find("\n---")?;
    let after = &rest[end + 4..];
    if !(after.is_empty() || after.starts_with('\n')) {
        return None;
    }
    Some(rest[..end].to_string())
}

fn parse_top_level_scalars(frontmatter: &str) -> Option<BTreeMap<String, serde_json::Value>> {
    let mut map = BTreeMap::new();
    for raw in frontmatter.split('\n') {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            continue;
        }
        if !line.contains(':') && !line.trim().is_empty() {
            // 顶层出现无 `:` 的行：与 YAML 解析失败同口径。
            return None;
        }
        let separator = line.find(':')?;
        let key = line[..separator].trim();
        if key.is_empty() {
            return None;
        }
        map.insert(key.to_string(), parse_scalar(line[separator + 1..].trim())?);
    }
    Some(map)
}

fn parse_scalar(value: &str) -> Option<serde_json::Value> {
    if value.is_empty() {
        return Some(serde_json::Value::Null);
    }
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        return Some(serde_json::Value::String(
            value[1..value.len() - 1].to_string(),
        ));
    }
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        return Some(serde_json::Value::String(
            value[1..value.len() - 1].replace("''", "'"),
        ));
    }
    if value.starts_with('[') || value.starts_with('{') {
        // 流式集合：只有合法 JSON 才视为解析成功，否则与 YAML 报错同口径。
        return serde_json::from_str::<serde_json::Value>(value).ok();
    }
    if matches!(value, "|" | ">" | ">-" | "|-" | ">+" | "|+") {
        // 块标量需要后续行，顶层扫描器不展开；按 YAML 成功但值为空处理，
        // 与「name/description 取不到字符串」一致。
        return Some(serde_json::Value::Null);
    }
    Some(match value {
        "true" | "True" => serde_json::Value::Bool(true),
        "false" | "False" => serde_json::Value::Bool(false),
        "null" | "~" => serde_json::Value::Null,
        _ => serde_json::Value::String(value.to_string()),
    })
}

fn collect_candidates_in_root(root: &Path) -> Vec<SkillSyncCandidate> {
    if !root.exists() {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for skill_path in walk_skill_markdown_paths(root) {
        let Some(directory_path) = skill_path.parent() else {
            continue;
        };
        let relative = directory_path
            .strip_prefix(root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if relative.is_empty() {
            continue;
        }
        let Some(content) = read_skill_file_or_null(&skill_path) else {
            continue;
        };
        let fallback = Path::new(&relative)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (name, description) = parse_skill_metadata(&content, &fallback);
        candidates.push(SkillSyncCandidate {
            id: candidate_id(directory_path),
            name,
            directory_name: relative,
            description,
            path: skill_path.to_string_lossy().into_owned(),
            size_bytes: compute_recursive_size(directory_path, &mut HashSet::new()),
        });
    }
    candidates.sort_by(|left, right| {
        plugin_discovery::js::locale_compare(&left.directory_name, &right.directory_name)
    });
    candidates
}

fn dedupe_by_canonical_path(
    candidates: Vec<SkillSyncCandidate>,
    seen: &mut HashSet<PathBuf>,
) -> Vec<SkillSyncCandidate> {
    let mut result = Vec::new();
    for candidate in candidates {
        let path = PathBuf::from(&candidate.path);
        let canonical = plugin_discovery::js::realpath(&path).unwrap_or(path);
        if !seen.insert(canonical) {
            continue;
        }
        result.push(candidate);
    }
    result
}

fn collect_covered_skill_directory_names(root: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if !root.exists() {
        return names;
    }
    let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];
    while let Some((directory, relative, depth)) = stack.pop() {
        if !relative.is_empty()
            && read_skill_file_or_null(&directory.join(SKILL_FILE_NAME)).is_some()
        {
            names.insert(relative.clone());
        }
        if depth >= MAX_SKILL_SCAN_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !should_walk_skill_directory_entry(&name) {
                continue;
            }
            let path = entry.path();
            let is_directory = std::fs::metadata(&path)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false);
            if !is_directory {
                continue;
            }
            let child_relative = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            stack.push((path, child_relative, depth + 1));
        }
    }
    names
}

fn collect_skill_directory_path_by_name(root: &Path) -> HashMap<String, PathBuf> {
    let mut result = HashMap::new();
    if !root.exists() {
        return result;
    }
    for skill_path in walk_skill_markdown_paths(root) {
        let Some(directory_path) = skill_path.parent() else {
            continue;
        };
        let Some(content) = read_skill_file_or_null(&skill_path) else {
            continue;
        };
        let fallback = directory_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (name, _) = parse_skill_metadata(&content, &fallback);
        result
            .entry(normalize_name_key(&name))
            .or_insert_with(|| directory_path.to_path_buf());
    }
    result
}

fn compute_recursive_size(path: &Path, visited: &mut HashSet<PathBuf>) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.file_type().is_symlink() {
        let Ok(target) = std::fs::metadata(path) else {
            return 0;
        };
        if target.is_file() {
            return target.len();
        }
        if target.is_dir() {
            return compute_directory_size(path, visited);
        }
        return 0;
    }
    if metadata.is_file() {
        return metadata.len();
    }
    if !metadata.is_dir() {
        return 0;
    }
    compute_directory_size(path, visited)
}

fn compute_directory_size(path: &Path, visited: &mut HashSet<PathBuf>) -> u64 {
    let canonical = plugin_discovery::js::realpath(path).unwrap_or_else(|| path.to_path_buf());
    if !visited.insert(canonical) {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| compute_recursive_size(&entry.path(), visited))
        .sum()
}

fn collect_extracted_skill_directories(root: &Path) -> Vec<(String, PathBuf)> {
    let mut directories = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, relative)) = stack.pop() {
        if !relative.is_empty() && directory.join(SKILL_FILE_NAME).exists() {
            directories.push((relative.clone(), directory.clone()));
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() || !should_walk_skill_directory_entry(&name) {
                continue;
            }
            let child_relative = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            stack.push((entry.path(), child_relative));
        }
    }
    directories.sort_by(|left, right| plugin_discovery::js::locale_compare(&left.0, &right.0));
    directories
}

fn copy_directory_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(target).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "target exists",
        ));
    }
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return copy_symlink(source, target);
    }
    if !metadata.is_dir() {
        std::fs::copy(source, target)?;
        return Ok(());
    }
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        copy_directory_recursive(&entry.path(), &target.join(entry.file_name()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(source)?, target)
}

#[cfg(windows)]
fn copy_symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    let resolved = std::fs::metadata(source)?;
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(std::fs::read_link(source)?, target)
    } else {
        std::os::windows::fs::symlink_file(std::fs::read_link(source)?, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_falls_back_to_directory_name() {
        assert_eq!(
            parse_skill_metadata("no frontmatter", "fallback"),
            ("fallback".to_string(), String::new())
        );
        assert_eq!(
            parse_skill_metadata(
                "---\nname: alpha\ndescription: 'quoted desc'\n---\nbody\n",
                "fallback"
            ),
            ("alpha".to_string(), "quoted desc".to_string())
        );
        assert_eq!(
            parse_skill_metadata("---\nname: [broken\n---\n", "fallback"),
            ("fallback".to_string(), String::new())
        );
    }
}
