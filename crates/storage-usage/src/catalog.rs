//! 存储分类目录：路径 → 类别/聚合 key、清理范围与保护规则。
//!
//! 真源：`packages/services/src/storage/domain/storageCatalog.ts`。
//! 匹配顺序：根级特例 → 文件规则（精确、按声明顺序）→ 前缀规则（最长优先）→ 其他。

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageRootId {
    Home,
    DataBaseDir,
}

impl StorageRootId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::DataBaseDir => "dataBaseDir",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "home" => Some(Self::Home),
            "dataBaseDir" => Some(Self::DataBaseDir),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StorageCategoryId {
    SessionStore,
    SubagentTranscripts,
    ToolOutputs,
    ModelTrajectory,
    DevTraces,
    Logs,
    Backups,
    Exports,
    Runtimes,
    Config,
    Other,
}

pub const STORAGE_CATEGORY_IDS: [StorageCategoryId; 11] = [
    StorageCategoryId::SessionStore,
    StorageCategoryId::SubagentTranscripts,
    StorageCategoryId::ToolOutputs,
    StorageCategoryId::ModelTrajectory,
    StorageCategoryId::DevTraces,
    StorageCategoryId::Logs,
    StorageCategoryId::Backups,
    StorageCategoryId::Exports,
    StorageCategoryId::Runtimes,
    StorageCategoryId::Config,
    StorageCategoryId::Other,
];

impl StorageCategoryId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStore => "sessionStore",
            Self::SubagentTranscripts => "subagentTranscripts",
            Self::ToolOutputs => "toolOutputs",
            Self::ModelTrajectory => "modelTrajectory",
            Self::DevTraces => "devTraces",
            Self::Logs => "logs",
            Self::Backups => "backups",
            Self::Exports => "exports",
            Self::Runtimes => "runtimes",
            Self::Config => "config",
            Self::Other => "other",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sessionStore" => Some(Self::SessionStore),
            "subagentTranscripts" => Some(Self::SubagentTranscripts),
            "toolOutputs" => Some(Self::ToolOutputs),
            "modelTrajectory" => Some(Self::ModelTrajectory),
            "devTraces" => Some(Self::DevTraces),
            "logs" => Some(Self::Logs),
            "backups" => Some(Self::Backups),
            "exports" => Some(Self::Exports),
            "runtimes" => Some(Self::Runtimes),
            "config" => Some(Self::Config),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageCleanability {
    None,
    Safe,
    Confirm,
}

impl StorageCleanability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Safe => "safe",
            Self::Confirm => "confirm",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageCatalogContext {
    pub root_id: StorageRootId,
    pub has_custom_data_base_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageClassification {
    pub category_id: StorageCategoryId,
    pub entry_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageCleanScope {
    pub prefix: String,
    pub recursive: bool,
}

struct FileRule {
    category_id: StorageCategoryId,
    pattern: Regex,
    entry_key_segments: Option<usize>,
}

fn file_rules() -> &'static [FileRule] {
    static RULES: OnceLock<Vec<FileRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let rule = |category_id, pattern: &str, entry_key_segments| FileRule {
            category_id,
            pattern: Regex::new(pattern).expect("存储文件规则正则可编译"),
            entry_key_segments,
        };
        vec![
            rule(
                StorageCategoryId::SubagentTranscripts,
                r"^cli/agents/[^/]+/[^/]+/transcript\.jsonl$",
                Some(3),
            ),
            rule(
                StorageCategoryId::SessionStore,
                r"^v2/[^/]+\.sqlite(?:-wal|-shm)?$",
                None,
            ),
            rule(
                StorageCategoryId::SessionStore,
                r"^cli/db/db\.sqlite(?:-wal|-shm)?$",
                None,
            ),
            rule(
                StorageCategoryId::ToolOutputs,
                r"^v2/checkpoints/(?:.+/)?(?:pending|tmp)/",
                None,
            ),
            rule(
                StorageCategoryId::Backups,
                r"^cli/db/db\.sqlite\.[^/]+$",
                None,
            ),
            rule(
                StorageCategoryId::Backups,
                r"^cli/config\.json\.bak[^/]*$",
                None,
            ),
            rule(StorageCategoryId::Backups, r"^v2/[^/]+\.bak$", None),
            rule(
                StorageCategoryId::Backups,
                r"^v2/[^/]+\.backup\.json$",
                None,
            ),
            rule(
                StorageCategoryId::Backups,
                r"^v2/setting\.json\.(?:corrupt-|[^/]*backup)[^/]*$",
                None,
            ),
            rule(
                StorageCategoryId::Backups,
                r"^v2/config\.json\.pre-[^/]+$",
                None,
            ),
            rule(
                StorageCategoryId::ToolOutputs,
                r"^v2/coding-plan-cache\.json$",
                None,
            ),
            rule(
                StorageCategoryId::ToolOutputs,
                r"^v2/bots-model-cache[^/]*\.json$",
                None,
            ),
            rule(
                StorageCategoryId::Logs,
                r"^computer-use/run/[^/]+\.log$",
                None,
            ),
            rule(StorageCategoryId::Config, r"^v2/[^/]+\.json$", None),
            rule(StorageCategoryId::Config, r"^cli/config\.json$", None),
            rule(StorageCategoryId::Config, r"^agents/[^/]+\.md$", None),
            rule(StorageCategoryId::Config, r"^AGENTS\.md$", None),
        ]
    })
}

fn prefix_rules(category_id: StorageCategoryId) -> &'static [&'static str] {
    match category_id {
        StorageCategoryId::SessionStore => {
            &["v2/sessions", "v2/session-bindings", "v2/checkpoints"]
        }
        StorageCategoryId::SubagentTranscripts => &[],
        StorageCategoryId::ToolOutputs => &[
            "cli/artifacts",
            "cli/agents",
            "cli/sessions",
            "cli/exec",
            "cli/image-cache",
            "cli/pdf-cache",
            "clipboard",
            "git-checkpoint-index",
            "editor-icon",
            "tmp",
            "cache",
        ],
        StorageCategoryId::ModelTrajectory => &["cli/debug", "cli/rollout"],
        StorageCategoryId::DevTraces => &[
            "v2/dev",
            "v2/acp-traffic-proxy",
            "v2/acp-stream-diagnostics",
        ],
        StorageCategoryId::Logs => &["v2/logs", "cli/log", "logs", "v2/crash", "v2/perf"],
        StorageCategoryId::Backups => &[
            "backup",
            "v2/backup",
            "v2/migrations",
            "cli/db/backup",
            "cli/db/backups",
        ],
        StorageCategoryId::Exports => &["export-log", "export-log-stage"],
        StorageCategoryId::Runtimes => &[
            "agents",
            "bundled-agents",
            "lite",
            "computer-use",
            "cli/plugins",
        ],
        StorageCategoryId::Config => &[
            "v2/agent-config",
            "v2/bots-runtime-locks",
            "v2/bot-attachments",
            "v2/certs",
            "v2/acp-auth",
            "v2/acp-config",
            "v2/provider",
            "cli/models",
            "cli/memories",
            "cli/workflows",
            "security",
            "commands",
            "skills",
            "workflows",
            "workspace",
            "mailbox",
            "server",
            "controller",
            "launcher",
            "dev-signing",
            "cua-helper-dev-identity",
            "perf-task-manifests",
            "plugin-workspace",
            "projects",
        ],
        StorageCategoryId::Other => &[],
    }
}

fn prefix_index() -> &'static [(String, StorageCategoryId)] {
    static INDEX: OnceLock<Vec<(String, StorageCategoryId)>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = Vec::new();
        for category_id in STORAGE_CATEGORY_IDS {
            for prefix in prefix_rules(category_id) {
                index.push((prefix.to_string(), category_id));
            }
        }
        // JS 的稳定排序在长度相同时保留 PREFIX_RULES 的声明顺序。
        index.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));
        index
    })
}

fn cleanability(category_id: StorageCategoryId) -> StorageCleanability {
    match category_id {
        StorageCategoryId::SubagentTranscripts
        | StorageCategoryId::ModelTrajectory
        | StorageCategoryId::DevTraces
        | StorageCategoryId::Logs
        | StorageCategoryId::Exports => StorageCleanability::Safe,
        StorageCategoryId::Backups => StorageCleanability::Confirm,
        StorageCategoryId::SessionStore
        | StorageCategoryId::ToolOutputs
        | StorageCategoryId::Runtimes
        | StorageCategoryId::Config
        | StorageCategoryId::Other => StorageCleanability::None,
    }
}

fn is_under_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || (path.starts_with(prefix) && path.as_bytes().get(prefix.len()).copied() == Some(b'/'))
}

fn entry_key_below(path: &str, prefix: &str) -> String {
    let rest = path.get(prefix.len() + 1..).unwrap_or("");
    let next = rest.split('/').next().unwrap_or("");
    if next.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}/{next}")
    }
}

fn protected_basename(basename: &str) -> bool {
    matches!(
        basename,
        "setting.json"
            | "setting.json.lock"
            | "credentials.json"
            | ".credentials.json"
            | ".tokens"
            | "mode-stdio-tap.json"
    )
}

pub fn normalize_storage_relative_path(relative_path: &str) -> String {
    relative_path
        .replace('\\', "/")
        .trim_matches('/')
        .to_string()
}

pub fn classify_storage_path(
    raw_path: &str,
    context: &StorageCatalogContext,
) -> StorageClassification {
    let path = normalize_storage_relative_path(raw_path);
    if context.root_id == StorageRootId::Home
        && context.has_custom_data_base_dir
        && is_under_prefix(&path, "v2")
    {
        return StorageClassification {
            category_id: StorageCategoryId::Other,
            entry_key: "v2".to_string(),
        };
    }
    if is_under_prefix(&path, "agent") {
        return StorageClassification {
            category_id: StorageCategoryId::Other,
            entry_key: "agent".to_string(),
        };
    }
    for rule in file_rules() {
        if rule.pattern.is_match(&path) {
            let entry_key = match rule.entry_key_segments {
                Some(segments) => path.split('/').take(segments).collect::<Vec<_>>().join("/"),
                None => path.clone(),
            };
            return StorageClassification {
                category_id: rule.category_id,
                entry_key,
            };
        }
    }
    for (prefix, category_id) in prefix_index() {
        if is_under_prefix(&path, prefix) {
            return StorageClassification {
                category_id: *category_id,
                entry_key: entry_key_below(&path, prefix),
            };
        }
    }
    StorageClassification {
        category_id: StorageCategoryId::Other,
        entry_key: path.split('/').next().unwrap_or("").to_string(),
    }
}

pub fn get_storage_category_cleanability(category_id: StorageCategoryId) -> StorageCleanability {
    cleanability(category_id)
}

pub fn is_protected_storage_path(raw_path: &str) -> bool {
    let path = normalize_storage_relative_path(raw_path);
    let basename = path.rsplit('/').next().unwrap_or(&path);
    if protected_basename(basename) {
        return true;
    }
    is_under_prefix(&path, "v2/crash/live")
}

fn file_rule_scopes(category_id: StorageCategoryId) -> &'static [&'static str] {
    match category_id {
        StorageCategoryId::Backups => &["cli/db", "cli", "v2"],
        StorageCategoryId::Logs => &["computer-use/run"],
        _ => &[],
    }
}

fn recursive_file_rule_scopes(category_id: StorageCategoryId) -> &'static [&'static str] {
    match category_id {
        StorageCategoryId::SubagentTranscripts => &["cli/agents"],
        _ => &[],
    }
}

pub fn get_storage_clean_scopes(category_id: StorageCategoryId) -> Vec<StorageCleanScope> {
    if category_id == StorageCategoryId::Other
        || cleanability(category_id) == StorageCleanability::None
    {
        return Vec::new();
    }
    let mut scopes: Vec<StorageCleanScope> = prefix_rules(category_id)
        .iter()
        .chain(recursive_file_rule_scopes(category_id).iter())
        .map(|prefix| StorageCleanScope {
            prefix: (*prefix).to_string(),
            recursive: true,
        })
        .collect();
    scopes.extend(
        file_rule_scopes(category_id)
            .iter()
            .map(|prefix| StorageCleanScope {
                prefix: (*prefix).to_string(),
                recursive: false,
            }),
    );
    scopes
}

impl StorageClassification {
    pub fn to_json(&self) -> Value {
        json!({
            "categoryId": self.category_id.as_str(),
            "entryKey": self.entry_key,
        })
    }
}

impl StorageCleanScope {
    pub fn to_json(&self) -> Value {
        json!({ "prefix": self.prefix, "recursive": self.recursive })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_windows_separators_and_edge_slashes() {
        assert_eq!(normalize_storage_relative_path(r"\v2\logs///"), "v2/logs");
    }

    #[test]
    fn chooses_longest_prefix() {
        let classification = classify_storage_path(
            "v2/crash/live/core.bin",
            &StorageCatalogContext {
                root_id: StorageRootId::Home,
                has_custom_data_base_dir: false,
            },
        );
        assert_eq!(classification.category_id, StorageCategoryId::Logs);
        assert_eq!(classification.entry_key, "v2/crash/live");
    }
}
