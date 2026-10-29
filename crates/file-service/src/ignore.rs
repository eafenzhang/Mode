//! `.modeignore`（工作区文件搜索忽略规则）：模板、分区同步、匹配与原子写。
//!
//! 行为真源 `packages/services/src/file/workspaceFileIgnore.ts`。gitignore 语义交给
//! `ignore` crate（与 TS 的 `ignore` npm 包同源实现），**不手写解析**。

use std::path::{Path, PathBuf};

use atomic_file::{write_text_atomic, WriteOptions};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use tokio::fs;

use crate::error::{file_result, FileResult};
use crate::types::{WorkspaceFileSearchIgnoreContent, EntryType};

pub const WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME: &str = ".modeignore";
/// 改名前的忽略文件名（.zcodeignore）：读取时继续兼容。
pub const LEGACY_WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME: &str = ".zcodeignore";
const GITIGNORE_FILE_NAME: &str = ".gitignore";

const BUILTIN_IGNORE_LINES: [&str; 28] = [
    ".git/",
    ".hg/",
    ".svn/",
    "node_modules/",
    "bower_components/",
    "jspm_packages/",
    "__pycache__/",
    "site-packages/",
    "venv/",
    "coverage/",
    "htmlcov/",
    "lcov-report/",
    "cmakefiles/",
    "cmake-build-*/",
    "bazel-*/",
    "pods/",
    "deriveddata/",
    "storybook-static/",
    "playwright-report/",
    "test-results/",
    "allure-results/",
    "allure-report/",
    "cdk.out/",
    "*.egg-info/",
    "*.dist-info/",
    "eggs/",
    "pip-wheel-metadata/",
    "wheels/",
];

const TEMPLATE_HEADER: [&str; 5] = [
    "# Mode 工作区文件搜索忽略规则（.zcodeignore）",
    "# 语法与 .gitignore 一致，只影响 Mode 的 @ 文件候选 / Command Center / 文件树搜索，",
    "# 不影响文件树浏览、上传或 Agent 文件访问。",
    "# 修改 .gitignore 不会自动同步到本文件；可在设置页「从 .gitignore 同步」。",
    "",
];

const WORKSPACE_FILE_SEARCH_IGNORE_SYNC_MARKER: &str =
    "# ===== ↑ 以上同步自 .gitignore（「从 .gitignore 同步」只重写以上部分）=====";
const WORKSPACE_FILE_SEARCH_IGNORE_DEFAULTS_MARKER: &str =
    "# ----- ↑ 以上为 Mode 默认排除规则（自定义规则请写在本行下方，不会被同步/恢复改动）-----";
const CUSTOM_SECTION_HINT: &str = "# 自定义规则写在下方（本行提示可删除）";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreSource {
    File,
    CreatedFromGitignore,
    CreatedFromTemplate,
    FallbackGitignore,
    FallbackBuiltin,
}

impl IgnoreSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::CreatedFromGitignore => "created-from-gitignore",
            Self::CreatedFromTemplate => "created-from-template",
            Self::FallbackGitignore => "fallback-gitignore",
            Self::FallbackBuiltin => "fallback-builtin",
        }
    }

    pub fn rules_active(self) -> bool {
        true
    }
}

pub struct IgnoreRules {
    matcher: Gitignore,
    pub source: IgnoreSource,
}

impl IgnoreRules {
    pub fn source(&self) -> IgnoreSource {
        self.source
    }
}

/// 加载结果：`warning` 只在降级时出现（调用方决定日志落点）。
pub struct IgnoreLoadOutcome {
    pub rules: IgnoreRules,
    pub warning: Option<String>,
}

/// 新名优先、旧名兜底；都不存在时返回新名（新建一律用新名）。
pub fn resolve_workspace_ignore_path(root_path: &Path) -> PathBuf {
    let primary = root_path.join(WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME);
    if primary.exists() {
        return primary;
    }
    let legacy = root_path.join(LEGACY_WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME);
    if legacy.exists() {
        return legacy;
    }
    primary
}

fn build_builtin_defaults_section(gitignore_content: Option<&str>) -> String {
    let Some(gitignore) = gitignore_content else {
        return BUILTIN_IGNORE_LINES.join("\n");
    };
    let mut declared = std::collections::HashSet::new();
    for raw_line in gitignore.split('\n') {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        declared.insert(line.to_string());
        declared.insert(line.trim_end_matches('/').to_string());
    }
    BUILTIN_IGNORE_LINES
        .iter()
        .filter(|line| {
            let bare = line.trim_end_matches('/');
            !declared.contains(**line) && !declared.contains(bare)
        })
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

fn gitignore_section(gitignore_content: Option<&str>) -> String {
    match gitignore_content {
        Some(content) if !content.trim().is_empty() => {
            if content.ends_with('\n') {
                content.to_string()
            } else {
                format!("{content}\n")
            }
        }
        _ => format!("{}\n", TEMPLATE_HEADER.join("\n")),
    }
}

fn build_template(gitignore_content: Option<&str>) -> String {
    [
        gitignore_section(gitignore_content),
        WORKSPACE_FILE_SEARCH_IGNORE_SYNC_MARKER.to_string(),
        build_builtin_defaults_section(gitignore_content),
        WORKSPACE_FILE_SEARCH_IGNORE_DEFAULTS_MARKER.to_string(),
        CUSTOM_SECTION_HINT.to_string(),
        String::new(),
    ]
    .join("\n")
}

struct Sections {
    gitignore: String,
    defaults: String,
    custom: String,
}

fn split_sections(content: &str) -> Option<Sections> {
    let lines: Vec<&str> = content.split('\n').collect();
    let sync_index = lines
        .iter()
        .position(|line| line.trim() == WORKSPACE_FILE_SEARCH_IGNORE_SYNC_MARKER)?;
    let defaults_index = lines
        .iter()
        .position(|line| line.trim() == WORKSPACE_FILE_SEARCH_IGNORE_DEFAULTS_MARKER)?;
    if defaults_index <= sync_index {
        return None;
    }
    Some(Sections {
        gitignore: lines[..sync_index].join("\n"),
        defaults: lines[sync_index + 1..defaults_index]
            .join("\n")
            .trim()
            .to_string(),
        custom: lines[defaults_index + 1..]
            .join("\n")
            .trim_start_matches('\n')
            .to_string(),
    })
}

fn collapse_trailing_newlines(content: String) -> String {
    let mut trimmed = content.trim_end_matches('\n').to_string();
    trimmed.push('\n');
    trimmed
}

pub fn sync_workspace_file_search_ignore_from_gitignore(
    current_content: &str,
    gitignore_content: Option<&str>,
) -> String {
    let Some(sections) = split_sections(current_content) else {
        return build_template(gitignore_content);
    };
    collapse_trailing_newlines(
        [
            gitignore_section(gitignore_content),
            WORKSPACE_FILE_SEARCH_IGNORE_SYNC_MARKER.to_string(),
            sections.defaults,
            WORKSPACE_FILE_SEARCH_IGNORE_DEFAULTS_MARKER.to_string(),
            sections.custom,
        ]
        .join("\n"),
    )
}

pub fn reset_workspace_file_search_ignore_defaults(
    current_content: &str,
    gitignore_content: Option<&str>,
) -> String {
    let Some(sections) = split_sections(current_content) else {
        return build_template(gitignore_content);
    };
    collapse_trailing_newlines(
        [
            sections.gitignore,
            WORKSPACE_FILE_SEARCH_IGNORE_SYNC_MARKER.to_string(),
            build_builtin_defaults_section(Some(&sections.defaults)),
            WORKSPACE_FILE_SEARCH_IGNORE_DEFAULTS_MARKER.to_string(),
            sections.custom,
        ]
        .join("\n"),
    )
}

/// 把整份规则文本按行喂给 `ignore` builder（与 TS `ignore().add(content)` 同语义：
/// 注释、`!` 反选、anchored、目录后缀都由实现处理）。
fn build_matcher(root: &Path, content: &str) -> FileResult<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    for raw_line in content.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if let Err(error) = builder.add_line(None, line) {
            return Err(file_result(format!("invalid ignore pattern: {error}")));
        }
    }
    builder
        .build()
        .map_err(|error| file_result(format!("invalid ignore rules: {error}")))
}

async fn read_optional_file(path: &Path) -> FileResult<Option<String>> {
    match fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(file_result(error.to_string())),
    }
}

async fn read_optional_file_or_none(path: &Path) -> Option<String> {
    read_optional_file(path).await.ok().flatten()
}

async fn atomic_write_ignore_file(path: &Path, content: &str) -> FileResult<()> {
    write_text_atomic(path, content, &WriteOptions::default())
        .map_err(|error| file_result(error.to_string()))
}

/// 扫描前加载规则；自动创建与 fail-open 降级链与 TS 一致。
pub async fn load_workspace_file_search_ignore_rules(root_path: &Path) -> IgnoreLoadOutcome {
    let ignore_path = resolve_workspace_ignore_path(root_path);

    let gitignore_path = root_path.join(GITIGNORE_FILE_NAME);

    let degrade = |reason: String, gitignore: Option<String>| -> IgnoreLoadOutcome {
        match gitignore {
            Some(content) => {
                let (rules, _) = build_matcher_or_empty(root_path, &content, IgnoreSource::FallbackGitignore);
                IgnoreLoadOutcome {
                    rules,
                    warning: Some(format!("{reason}，降级为运行时使用 .gitignore 规则")),
                }
            }
            None => {
                let template = build_template(None);
                let (rules, _) = build_matcher_or_empty(root_path, &template, IgnoreSource::FallbackBuiltin);
                IgnoreLoadOutcome {
                    rules,
                    warning: Some(format!("{reason}，降级为内置默认忽略规则")),
                }
            }
        }
    };

    let existing = match read_optional_file(&ignore_path).await {
        Ok(existing) => existing,
        Err(error) => {
            let gitignore = read_optional_file_or_none(&gitignore_path).await;
            return degrade(
                format!("读取 {WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME} 失败：{error}"),
                gitignore,
            );
        }
    };
    if let Some(content) = existing {
        let (rules, warning) =
            build_matcher_or_empty(root_path, &content, IgnoreSource::File);
        return IgnoreLoadOutcome { rules, warning };
    }

    let gitignore = read_optional_file_or_none(&gitignore_path).await;
    let initial = build_template(gitignore.as_deref());
    match atomic_write_ignore_file(&ignore_path, &initial).await {
        Ok(()) => {
            let (rules, warning) = build_matcher_or_empty(
                root_path,
                &initial,
                if gitignore.is_some() {
                    IgnoreSource::CreatedFromGitignore
                } else {
                    IgnoreSource::CreatedFromTemplate
                },
            );
            IgnoreLoadOutcome {
                rules,
                warning: warning.map(|warning| format!("自动创建规则文件后解析失败：{warning}")),
            }
        }
        Err(error) => {
            let source = if gitignore.is_some() {
                IgnoreSource::FallbackGitignore
            } else {
                IgnoreSource::FallbackBuiltin
            };
            let (rules, _) = build_matcher_or_empty(root_path, &initial, source);
            IgnoreLoadOutcome {
                rules,
                warning: Some(format!(
                    "自动创建 {WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME} 失败，降级为运行时使用{}规则：{error}",
                    if gitignore.is_some() { ".gitignore" } else { "内置默认" }
                )),
            }
        }
    }
}

/// 构建失败时返回空规则并保留告警（扫描不因手改坏规则整体失败）。
fn build_matcher_or_empty(
    root: &Path,
    content: &str,
    source: IgnoreSource,
) -> (IgnoreRules, Option<String>) {
    match build_matcher(root, content) {
        Ok(matcher) => (IgnoreRules { matcher, source }, None),
        Err(error) => {
            let empty = GitignoreBuilder::new(root)
                .build()
                .expect("空规则集必须可构建");
            (
                IgnoreRules {
                    matcher: empty,
                    source,
                },
                Some(error.message),
            )
        }
    }
}

pub fn is_workspace_file_search_path_ignored(
    rules: &IgnoreRules,
    relative_path: &str,
    r#type: EntryType,
) -> bool {
    // 与 npm `ignore().ignores()` 一致：父目录被排除后子路径同样算忽略（whitelist 不能
    // 把已排除目录里的文件救回来）。`matched` 只看路径本身，不能替代这条语义。
    rules
        .matcher
        .matched_path_or_any_parents(Path::new(relative_path), r#type.is_directory())
        .is_ignore()
}

/// 设置页读取：文件不存在时返回模板预览，不落盘。
pub async fn read_workspace_file_search_ignore(
    root_path: &Path,
) -> FileResult<WorkspaceFileSearchIgnoreContent> {
    let ignore_path = resolve_workspace_ignore_path(root_path);
    let existing = read_optional_file(&ignore_path).await?;
    if let Some(content) = existing {
        return Ok(WorkspaceFileSearchIgnoreContent {
            content,
            source: "file".into(),
        });
    }
    let gitignore = read_optional_file_or_none(&root_path.join(GITIGNORE_FILE_NAME)).await;
    Ok(WorkspaceFileSearchIgnoreContent {
        content: build_template(gitignore.as_deref()),
        source: "template".into(),
    })
}

pub async fn transform_workspace_file_search_ignore(
    root_path: &Path,
    transform: &str,
) -> FileResult<String> {
    let ignore_path = resolve_workspace_ignore_path(root_path);
    let existing = read_optional_file_or_none(&ignore_path).await;
    let gitignore = read_optional_file_or_none(&root_path.join(GITIGNORE_FILE_NAME)).await;
    let current = existing.unwrap_or_else(|| build_template(gitignore.as_deref()));
    Ok(if transform == "sync-gitignore" {
        sync_workspace_file_search_ignore_from_gitignore(&current, gitignore.as_deref())
    } else {
        reset_workspace_file_search_ignore_defaults(&current, gitignore.as_deref())
    })
}

pub async fn write_workspace_file_search_ignore(
    root_path: &Path,
    content: &str,
) -> FileResult<()> {
    let ignore_path = resolve_workspace_ignore_path(root_path);
    atomic_write_ignore_file(&ignore_path, content).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_contains_sections_and_builtin_defaults() {
        let template = build_template(None);
        assert!(template.starts_with("# Mode 工作区文件搜索忽略规则"));
        assert!(template.contains(WORKSPACE_FILE_SEARCH_IGNORE_SYNC_MARKER));
        assert!(template.contains(WORKSPACE_FILE_SEARCH_IGNORE_DEFAULTS_MARKER));
        assert!(template.contains("node_modules/"));
        assert!(template.ends_with(CUSTOM_SECTION_HINT) || template.ends_with('\n'));
    }

    #[test]
    fn builtin_defaults_dedupe_gitignore_lines() {
        let gitignore = "node_modules/\n/dist/\n!keep\ncmakefiles";
        let defaults = build_builtin_defaults_section(Some(gitignore));
        assert!(!defaults.contains("node_modules/"), "已声明规则不重复");
        assert!(!defaults.contains("cmakefiles/"), "忽略尾斜杠后仍算重复");
        assert!(defaults.contains(".git/"));
    }

    #[test]
    fn sync_only_rewrites_gitignore_section() {
        let current = build_template(Some("dist/\n"));
        let synced = sync_workspace_file_search_ignore_from_gitignore(
            &current,
            Some("build/\ncustom/\n"),
        );
        assert!(synced.starts_with("build/\ncustom/\n"));
        assert!(!synced.contains("dist/"));
        assert!(synced.contains("node_modules/"), "默认段保留");
    }

    #[test]
    fn reset_only_rewrites_defaults_section() {
        let current = build_template(Some("dist/\n")).replace(
            "node_modules/",
            "# 用户删掉了 node_modules",
        );
        let reset = reset_workspace_file_search_ignore_defaults(&current, Some("dist/\n"));
        assert!(reset.contains("node_modules/"), "默认段恢复");
        assert!(reset.contains("dist/"), "gitignore 区保留");
    }

    #[test]
    fn missing_markers_fall_back_to_template() {
        let rebuilt =
            sync_workspace_file_search_ignore_from_gitignore("只写自定义规则\n", Some("dist/\n"));
        assert!(rebuilt.contains(WORKSPACE_FILE_SEARCH_IGNORE_SYNC_MARKER));
        assert!(rebuilt.contains("dist/"));
    }
}
