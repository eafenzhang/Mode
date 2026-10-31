//! Git 路径与命令环境（`packages/services/src/git/config.ts`）。

use std::path::{Path, PathBuf};

pub const DEFAULT_GIT_DISCOVERY_TIMEOUT_MS: u64 = 3_000;
pub const DEFAULT_GIT_COMMAND_TIMEOUT_MS: u64 = 15_000;
pub const DEFAULT_GIT_PUSH_TIMEOUT_MS: u64 = 10 * 60_000;
pub const DEFAULT_GIT_DIFF_TIMEOUT_MS: u64 = 20_000;
pub const DEFAULT_GIT_OUTPUT_BYTES: usize = 512 * 1024;
pub const DEFAULT_GIT_PUSH_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_GIT_DIFF_BYTES: usize = 1024 * 1024;
pub const GIT_UNTRACKED_STAT_MAX_BYTES: u64 = 1024 * 1024;
pub const GIT_UNTRACKED_STAT_CHUNK_BYTES: usize = 64 * 1024;
pub const GIT_UNTRACKED_STAT_CONCURRENCY: usize = 4;

const GIT_LOCAL_ENV_VARS: [&str; 16] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_DIR",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_INTERNAL_SUPER_PREFIX",
    "GIT_NAMESPACE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

fn windows_git_candidates() -> Vec<PathBuf> {
    let program_w6432 = std::env::var("ProgramW6432").ok();
    let program_files = std::env::var("ProgramFiles").ok();
    let base = program_w6432
        .or(program_files)
        .unwrap_or_else(|| "C:\\Program Files".to_string());
    let base_x86 = std::env::var("ProgramFiles(x86)")
        .unwrap_or_else(|_| "C:\\Program Files (x86)".to_string());
    vec![
        Path::new(&base).join("Git").join("cmd").join("git.exe"),
        Path::new(&base).join("Git").join("bin").join("git.exe"),
        Path::new(&base_x86).join("Git").join("cmd").join("git.exe"),
        Path::new(&base_x86).join("Git").join("bin").join("git.exe"),
    ]
}

pub fn git_binary_candidates() -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(overridden) = std::env::var("MODE_GIT_BINARY") {
        let trimmed = overridden.trim();
        if !trimmed.is_empty() {
            candidates.push(trimmed.to_string());
        }
    }
    candidates.push("git".to_string());
    if cfg!(windows) {
        candidates.extend(
            windows_git_candidates()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned()),
        );
    }
    candidates
}

/// `getGitCommandEnv`：继承父进程环境，移除 Git local env（防止 hook 串仓），
/// 再叠加 Mode 的非交互约束。
pub fn git_command_env() -> Vec<(String, Option<String>)> {
    let mut overrides: Vec<(String, Option<String>)> = GIT_LOCAL_ENV_VARS
        .iter()
        .map(|name| (name.to_string(), None))
        .collect();
    for (key, value) in [
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_PAGER", "cat"),
        ("PAGER", "cat"),
        ("TERM", "dumb"),
        ("LC_ALL", "C"),
        ("LANG", "C"),
    ] {
        overrides.push((key.to_string(), Some(value.to_string())));
    }
    overrides
}

pub fn git_null_device_path() -> &'static str {
    if cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

pub fn normalize_git_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn normalize_workspace_in_repo_path(path: &str) -> String {
    let normalized = normalize_git_path(path);
    let trimmed = normalized
        .strip_prefix("./")
        .or_else(|| normalized.strip_prefix('/'))
        .unwrap_or(&normalized)
        .trim_end_matches('/')
        .to_string();
    if trimmed.is_empty() {
        ".".to_string()
    } else {
        trimmed
    }
}

pub fn is_path_in_workspace_scope(repo_relative_path: &str, workspace_in_repo_path: &str) -> bool {
    let normalized_path = strip_leading_dot_slash(&normalize_git_path(repo_relative_path));
    let normalized_workspace = normalize_workspace_in_repo_path(workspace_in_repo_path);
    if normalized_workspace == "." {
        return true;
    }
    normalized_path == normalized_workspace
        || normalized_path.starts_with(&format!("{normalized_workspace}/"))
}

pub fn to_workspace_relative_git_path(
    repo_relative_path: &str,
    workspace_in_repo_path: &str,
) -> String {
    let normalized_path = strip_leading_dot_slash(&normalize_git_path(repo_relative_path));
    let normalized_workspace = normalize_workspace_in_repo_path(workspace_in_repo_path);
    if normalized_workspace == "." {
        return normalized_path;
    }
    if normalized_path == normalized_workspace {
        return ".".to_string();
    }
    normalized_path
        .strip_prefix(&format!("{normalized_workspace}/"))
        .map(str::to_string)
        .unwrap_or(normalized_path)
}

fn strip_leading_dot_slash(path: &str) -> String {
    path.strip_prefix("./").unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_scope_helpers() {
        assert!(is_path_in_workspace_scope("pkg/a.md", "."));
        assert!(is_path_in_workspace_scope("pkg/a.md", "pkg"));
        assert!(!is_path_in_workspace_scope("pkgx/a.md", "pkg"));
        assert_eq!(to_workspace_relative_git_path("pkg/a.md", "pkg"), "a.md");
        assert_eq!(to_workspace_relative_git_path("pkg", "pkg"), ".");
        assert_eq!(normalize_workspace_in_repo_path("./pkg///"), "pkg");
        assert_eq!(normalize_workspace_in_repo_path(""), ".");
        assert_eq!(normalize_git_path("a\\b\\c"), "a/b/c");
    }
}
