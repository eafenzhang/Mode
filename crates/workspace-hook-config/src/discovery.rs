//! Workspace Hook 配置文件发现与读取（对齐 TS `workspace-hook-config.ts`）。
//!
//! 发现顺序即 `discoveryOrder`，是 bundle digest 的输入：任何重排都会让已有信任记录失效，
//! 因此这里的目录上溯、候选文件名、去重与显式配置追加顺序必须与 TS 逐条一致。

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::schema::{validate_hooks_config, ValidationError};

pub const DEFAULT_WORKSPACE_HOOK_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_WORKSPACE_HOOK_MAX_OUTPUT_BYTES: u64 = 32_768;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPathRef {
    pub path: PathBuf,
    pub explicit_project_config: bool,
}

#[derive(Debug, Clone)]
pub struct SourceInput {
    pub canonical_path: PathBuf,
    pub base_dir: PathBuf,
    pub discovery_order: usize,
    pub config_file_kind: &'static str,
    pub explicit_project_config: bool,
    pub editable: bool,
    pub hooks: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReadError {
    pub path: PathBuf,
    /// `io`（读文件失败）/ `syntax`（JSON 语法错）/ `schema`（结构校验失败）。
    pub category: &'static str,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimeRootInput {
    pub enabled: Option<bool>,
    pub timeout_ms: Option<f64>,
    pub max_output_bytes: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeRoot {
    pub enabled: bool,
    pub timeout_ms: u64,
    pub max_output_bytes: u64,
}

/// 词法归一：折叠 `.` 与 `..`，不解析符号链接（与 Node `path.resolve` 的可见行为一致）。
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path.resolve(value)`：相对路径按进程 cwd 展开。
pub fn resolve_path(value: &Path) -> PathBuf {
    if value.is_absolute() {
        return normalize_lexical(value);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    normalize_lexical(&cwd.join(value))
}

fn has_worktree_marker(directory: &Path) -> bool {
    let marker = directory.join(".git");
    match std::fs::metadata(&marker) {
        Ok(metadata) => metadata.is_dir() || metadata.is_file(),
        Err(_) => false,
    }
}

/// 从 cwd 向上走到 worktree 根（含 `.git`），返回「根 → cwd」顺序的目录列表。
/// 找不到标记时只扫 cwd 自身（与 TS 一致，沿途祖先全部丢弃）。
fn project_config_directories(start: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut current = start.to_path_buf();
    loop {
        directories.push(current.clone());
        if has_worktree_marker(&current) {
            directories.reverse();
            return directories;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    vec![start.to_path_buf()]
}

fn candidate_paths(directories: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for directory in directories {
        paths.push(directory.join("mode.json"));
        paths.push(directory.join(".mode").join("config.json"));
    }
    paths
}

fn deduplicate(refs: Vec<ConfigPathRef>) -> Vec<ConfigPathRef> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for reference in refs {
        let resolved = resolve_path(&reference.path);
        if seen.insert(resolved) {
            result.push(reference);
        }
    }
    result
}

/// 发现 workspace 的 hook 配置候选文件（自动发现在前，显式配置在后）。
pub fn discover_workspace_hook_config_paths(
    working_directory: &Path,
    explicit_project_config_path: Option<&Path>,
) -> Vec<ConfigPathRef> {
    let start = resolve_path(working_directory);
    let mut refs: Vec<ConfigPathRef> = candidate_paths(&project_config_directories(&start))
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| ConfigPathRef {
            path,
            explicit_project_config: false,
        })
        .collect();
    if let Some(explicit) = explicit_project_config_path {
        let explicit = resolve_path(explicit);
        if explicit.exists() {
            refs.push(ConfigPathRef {
                path: explicit,
                explicit_project_config: true,
            });
        }
    }
    deduplicate(refs)
}

/// 把一条已读取的配置归一为 digest 输入。
pub fn create_workspace_hook_source_input(
    path: &Path,
    working_directory: &Path,
    hooks: Value,
    discovery_order: usize,
    explicit_project_config: bool,
) -> SourceInput {
    let canonical_path = resolve_path(path);
    let config_directory = canonical_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let base_dir = if config_directory.file_name().and_then(|name| name.to_str()) == Some(".mode") {
        config_directory
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    } else {
        config_directory
    };
    let config_file_kind = if explicit_project_config {
        "explicit"
    } else if canonical_path.file_name().and_then(|name| name.to_str()) == Some("mode.json") {
        "mode.json"
    } else {
        ".mode/config.json"
    };
    let editable = !explicit_project_config
        && canonical_path == resolve_path(&working_directory.join(".mode").join("config.json"));
    SourceInput {
        canonical_path,
        base_dir,
        discovery_order,
        config_file_kind,
        explicit_project_config,
        editable,
        hooks,
    }
}

/// 读取发现到的项目级 hook 配置。
pub fn read_workspace_hook_project_sources(
    working_directory: &Path,
    explicit_project_config_path: Option<&Path>,
) -> (Vec<SourceInput>, Vec<SourceReadError>) {
    let refs = discover_workspace_hook_config_paths(working_directory, explicit_project_config_path);
    let mut sources = Vec::new();
    let mut errors = Vec::new();
    for (discovery_order, reference) in refs.iter().enumerate() {
        let raw = match std::fs::read_to_string(&reference.path) {
            Ok(raw) => raw,
            Err(_) => {
                errors.push(SourceReadError {
                    path: reference.path.clone(),
                    category: "io",
                });
                continue;
            }
        };
        let parsed: Value = match serde_json::from_str(&raw) {
            Ok(parsed) => parsed,
            Err(_) => {
                errors.push(SourceReadError {
                    path: reference.path.clone(),
                    category: "syntax",
                });
                continue;
            }
        };
        let Some(object) = parsed.as_object() else {
            continue;
        };
        let Some(hooks) = object.get("hooks") else {
            continue;
        };
        match validate_hooks_config(hooks) {
            Ok(()) => sources.push(create_workspace_hook_source_input(
                &reference.path,
                working_directory,
                hooks.clone(),
                discovery_order,
                reference.explicit_project_config,
            )),
            Err(ValidationError(_)) => errors.push(SourceReadError {
                path: reference.path.clone(),
                category: "schema",
            }),
        }
    }
    (sources, errors)
}

/// 多份配置根的 hooks 开关归并：`enabled` 一旦为 true 即保持；超时与输出上限取最后一份显式值。
pub fn resolve_workspace_hook_runtime_root(roots: &[Option<RuntimeRootInput>]) -> RuntimeRoot {
    let mut enabled = false;
    let mut timeout_ms = DEFAULT_WORKSPACE_HOOK_TIMEOUT_MS as f64;
    let mut max_output_bytes = DEFAULT_WORKSPACE_HOOK_MAX_OUTPUT_BYTES as f64;
    for root in roots.iter().flatten() {
        if root.enabled == Some(true) {
            enabled = true;
        }
        if let Some(value) = root.timeout_ms {
            timeout_ms = value;
        }
        if let Some(value) = root.max_output_bytes {
            max_output_bytes = value;
        }
    }
    RuntimeRoot {
        enabled,
        timeout_ms: crate::resolve_timeout_ms("process", Some(timeout_ms), None, 60_000),
        max_output_bytes: crate::resolve_max_output_bytes(max_output_bytes),
    }
}

/// 路径文本按 `/` 归一，供 JSON 输出与对拍比较。
pub fn path_to_forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

impl ConfigPathRef {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "path": path_to_forward_slashes(&self.path),
            "explicitProjectConfig": self.explicit_project_config,
        })
    }
}

impl SourceInput {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "canonicalPath": path_to_forward_slashes(&self.canonical_path),
            "baseDir": path_to_forward_slashes(&self.base_dir),
            "discoveryOrder": self.discovery_order,
            "configFileKind": self.config_file_kind,
            "explicitProjectConfig": self.explicit_project_config,
            "editable": self.editable,
            "hooks": self.hooks,
        })
    }
}

impl SourceReadError {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "path": path_to_forward_slashes(&self.path),
            "category": self.category,
        })
    }
}
