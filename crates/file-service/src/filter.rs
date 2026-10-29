//! 默认工作区文件搜索过滤器（`workspaceFileMentionFilter.ts`）。

use crate::types::EntryType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceFileSearchDecision {
    pub include: bool,
    pub traverse: bool,
}

const SKIPPED_DIRECTORY_NAMES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "bower_components",
    "jspm_packages",
    "__pycache__",
    "site-packages",
    "venv",
    "coverage",
    "htmlcov",
    "lcov-report",
    "cmakefiles",
    "pods",
    "deriveddata",
    "storybook-static",
    "playwright-report",
    "test-results",
    "allure-results",
    "allure-report",
    "cdk.out",
    "eggs",
    "pip-wheel-metadata",
    "wheels",
];
const SKIPPED_DIRECTORY_PREFIXES: &[&str] = &["cmake-build-", "bazel-"];
const SKIPPED_DIRECTORY_SUFFIXES: &[&str] = &[".egg-info", ".dist-info"];
const SKIPPED_FILE_NAMES: &[&str] = &["coverage.out", "lcov.info"];
const SKIPPED_FILE_EXTENSIONS: &[&str] = &[
    ".a", ".aar", ".beam", ".class", ".dll", ".dylib", ".ear", ".exe", ".gcda", ".gcno", ".gem",
    ".hi", ".idb", ".ilk", ".jar", ".lib", ".node", ".nupkg", ".o", ".obj", ".pdb", ".profdata",
    ".profraw", ".pyc", ".pyo", ".rlib", ".so", ".tsbuildinfo", ".war",
];

fn should_skip_directory(name: &str) -> bool {
    let normalized = name.to_lowercase();
    SKIPPED_DIRECTORY_NAMES.contains(&normalized.as_str())
        || SKIPPED_DIRECTORY_PREFIXES
            .iter()
            .any(|prefix| normalized.starts_with(prefix))
        || SKIPPED_DIRECTORY_SUFFIXES
            .iter()
            .any(|suffix| normalized.ends_with(suffix))
}

/// Node `path.extname`：dotfile 与 `.`/`..` 没有扩展名，`foo.` 的扩展名是 `.`。
pub fn node_extname(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    if base == "." || base == ".." {
        return String::new();
    }
    match base.rfind('.') {
        None | Some(0) => String::new(),
        Some(index) => base[index..].to_string(),
    }
}

fn should_skip_file(name: &str) -> bool {
    let normalized = name.to_lowercase();
    normalized == ".env"
        || normalized.starts_with(".env.")
        || SKIPPED_FILE_NAMES.contains(&normalized.as_str())
        || SKIPPED_FILE_EXTENSIONS.contains(&node_extname(&normalized).as_str())
}

fn is_inside_hidden_directory(relative_path: &str) -> bool {
    let segments: Vec<&str> = relative_path.split('/').collect();
    let directory_segments = &segments[..segments.len().saturating_sub(1)];
    directory_segments
        .iter()
        .any(|segment| segment.starts_with('.'))
}

/// 默认过滤器；`ignore_rules_active` 表示 `.modeignore` 已加载（目录黑名单退役）。
pub fn workspace_file_search_decision(
    name: &str,
    relative_path: &str,
    r#type: EntryType,
    ignore_rules_active: bool,
) -> WorkspaceFileSearchDecision {
    if r#type == EntryType::Directory {
        if !ignore_rules_active && should_skip_directory(name) {
            return WorkspaceFileSearchDecision {
                include: false,
                traverse: false,
            };
        }
        // 隐藏目录不占候选，但保留遍历（.github 里的文件仍能被名称搜索命中）。
        let hidden_directory =
            name.starts_with('.') || is_inside_hidden_directory(relative_path);
        return WorkspaceFileSearchDecision {
            include: !hidden_directory,
            traverse: true,
        };
    }
    WorkspaceFileSearchDecision {
        include: !should_skip_file(name),
        traverse: false,
    }
}

pub fn default_workspace_file_search_filter(
    name: &str,
    relative_path: &str,
    r#type: EntryType,
    ignore_rules_active: bool,
) -> WorkspaceFileSearchDecision {
    workspace_file_search_decision(name, relative_path, r#type, ignore_rules_active)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extname_matches_node_edge_cases() {
        assert_eq!(node_extname(".env"), "");
        assert_eq!(node_extname(".."), "");
        assert_eq!(node_extname("."), "");
        assert_eq!(node_extname("foo."), ".");
        assert_eq!(node_extname("foo"), "");
        assert_eq!(node_extname("a..b"), ".b");
        assert_eq!(node_extname("..."), ".");
        assert_eq!(node_extname("a.b/c"), "");
        assert_eq!(node_extname("a/.b.c"), ".c");
    }

    #[test]
    fn skips_noise_files() {
        for name in [".env", ".env.local", "a.dll", "x.PYC", "coverage.out", "app.o"] {
            assert!(
                !workspace_file_search_decision(name, name, EntryType::File, true).include,
                "{name} 应被排除"
            );
        }
        assert!(workspace_file_search_decision("main.rs", "src/main.rs", EntryType::File, true).include);
        assert!(workspace_file_search_decision(".envrc", ".envrc", EntryType::File, true).include);
    }

    #[test]
    fn directory_blacklist_retires_when_rules_active() {
        let active = workspace_file_search_decision("node_modules", "node_modules", EntryType::Directory, true);
        assert!(active.include, "规则文件生效时目录黑名单退役");
        assert!(active.traverse);

        let inactive = workspace_file_search_decision("node_modules", "node_modules", EntryType::Directory, false);
        assert!(!inactive.include);
        assert!(!inactive.traverse);
    }

    #[test]
    fn hidden_directories_are_not_candidates_but_are_traversed() {
        let decision = workspace_file_search_decision(".github", ".github", EntryType::Directory, true);
        assert!(!decision.include);
        assert!(decision.traverse);
        let nested = workspace_file_search_decision(
            "workflows",
            ".github/workflows",
            EntryType::Directory,
            true,
        );
        assert!(!nested.include);
        assert!(nested.traverse);
    }
}
