//! 技能扫描根：workspace 祖先链、用户根、扫描根归并。
//!
//! 真源：`skillsService.ts` 的 `getWorkspaceModeSkillRoot` / `getWorkspaceAgentsSkillRoot` /
//! `getUserModeSkillRoot` / `getUserAgentsSkillRoot` / `resolveAncestorWorkspaceRoots` /
//! `dedupeRoots` / `filterNestedScanRoots` / `dedupeScanRootsByRealpath`。

use std::path::{Path, PathBuf};

use crate::fsx;

pub const MODE_DATA_ROOT_DIR_NAME: &str = ".mode";
const GIT_MARKER: &str = ".git";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillScope {
    Workspace,
    User,
    Plugin,
}

impl SkillScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SkillScope::Workspace => "workspace",
            SkillScope::User => "user",
            SkillScope::Plugin => "plugin",
        }
    }
}

/// 一个待扫描的技能根及其来源（scope / 插件归属）。
#[derive(Debug, Clone)]
pub(crate) struct SkillRootDescriptor {
    pub scope: SkillScope,
    pub root_path: PathBuf,
    pub plugin_name: Option<String>,
    pub plugin_id: Option<String>,
}

pub fn workspace_mode_skill_root(workspace_path: &Path) -> PathBuf {
    workspace_path.join(MODE_DATA_ROOT_DIR_NAME).join("skills")
}

pub fn workspace_agents_skill_root(workspace_path: &Path) -> PathBuf {
    workspace_path.join(".agents").join("skills")
}

pub fn user_mode_skill_root(home_dir: &Path) -> PathBuf {
    home_dir.join(MODE_DATA_ROOT_DIR_NAME).join("skills")
}

pub fn user_agents_skill_root(home_dir: &Path) -> PathBuf {
    home_dir.join(".agents").join("skills")
}

/// 从 workspacePath 向上到 worktree 根，每层先 `.mode/skills` 后 `.agents/skills`。
pub(crate) fn resolve_ancestor_workspace_roots(workspace_path: &Path) -> Vec<PathBuf> {
    let worktree_root = find_worktree_root(workspace_path);
    let mut base_directories: Vec<PathBuf> = Vec::new();
    match worktree_root {
        None => base_directories.push(workspace_path.to_path_buf()),
        Some(worktree_root) => {
            let mut current = workspace_path.to_path_buf();
            loop {
                base_directories.push(current.clone());
                let parent_is_self = current
                    .parent()
                    .map(|parent| parent == current)
                    .unwrap_or(true);
                if current == worktree_root || parent_is_self {
                    break;
                }
                match current.parent() {
                    Some(parent) => current = parent.to_path_buf(),
                    None => break,
                }
            }
        }
    }

    let mut roots = Vec::with_capacity(base_directories.len() * 2);
    for directory in base_directories {
        // 与 TS 一致：两个根都收集，不做「`.mode` 有技能就截断 `.agents`」的兜底。
        roots.push(workspace_mode_skill_root(&directory));
        roots.push(workspace_agents_skill_root(&directory));
    }
    roots
}

fn find_worktree_root(working_directory: &Path) -> Option<PathBuf> {
    let mut current = working_directory.to_path_buf();
    loop {
        if fsx::exists(&current.join(GIT_MARKER)) {
            return Some(current);
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return None,
        }
    }
}

/// 按字符串去重，保留首次出现顺序（TS 的 `new Set(paths)`）。
pub(crate) fn dedupe_roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut result: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !result.iter().any(|existing| existing == path) {
            result.push(path.clone());
        }
    }
    result
}

pub(crate) fn normalize_scan_root_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    text.trim_end_matches('/').to_string()
}

/// 去掉会被「已存在」的更深层扫描根完全覆盖的祖先目录。
pub(crate) fn filter_nested_scan_roots(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut existing: Vec<String> = Vec::new();
    for path in paths {
        if fsx::exists(path) {
            existing.push(normalize_scan_root_path(path));
        }
    }
    let normalized: Vec<String> = paths
        .iter()
        .map(|path| normalize_scan_root_path(path))
        .collect();

    paths
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            let current = &normalized[*index];
            if !existing.iter().any(|candidate| candidate == current) {
                return true;
            }
            let covered = normalized.iter().enumerate().any(|(other_index, other)| {
                other_index != *index
                    && existing.iter().any(|candidate| candidate == other)
                    && other.starts_with(&format!("{current}/"))
            });
            !covered
        })
        .map(|(_, path)| path.clone())
        .collect()
}

/// 存在路径按 realpath 去重，但返回原路径。
pub(crate) fn dedupe_scan_roots_by_realpath(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen: Vec<String> = Vec::new();
    let mut result = Vec::new();
    for path in filter_nested_scan_roots(paths) {
        let canonical = if fsx::exists(&path) {
            normalize_scan_root_path(&fsx::realpath(&path).unwrap_or_else(|| path.clone()))
        } else {
            normalize_scan_root_path(&path)
        };
        if seen.iter().any(|existing| existing == &canonical) {
            continue;
        }
        seen.push(canonical);
        result.push(path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "zc-skills-paths-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("创建临时目录");
        path
    }

    #[test]
    fn ancestor_roots_walk_up_to_git_marker() {
        let root = temp_dir("ancestor");
        let workspace = root.join("ws");
        std::fs::create_dir_all(workspace.join(".git")).unwrap();
        std::fs::create_dir_all(workspace.join("pkg").join("sub")).unwrap();
        let roots = resolve_ancestor_workspace_roots(&workspace.join("pkg").join("sub"));
        assert_eq!(
            roots,
            vec![
                workspace
                    .join("pkg")
                    .join("sub")
                    .join(".mode")
                    .join("skills"),
                workspace
                    .join("pkg")
                    .join("sub")
                    .join(".agents")
                    .join("skills"),
                workspace.join("pkg").join(".mode").join("skills"),
                workspace.join("pkg").join(".agents").join("skills"),
                workspace.join(".mode").join("skills"),
                workspace.join(".agents").join("skills"),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nested_existing_roots_are_filtered() {
        let root = temp_dir("nested");
        let parent = root.join("a");
        let child = parent.join("b");
        std::fs::create_dir_all(&child).unwrap();
        let filtered = filter_nested_scan_roots(&[parent.clone(), child.clone()]);
        assert_eq!(filtered, vec![child]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
