//! Node `fs` / `path` / 排序语义的最小复刻（`js` 模块）与插件来源发现。
//!
//! 真源：
//! - `packages/shared/src/plugin-marketplaces.ts`（官方市场 id 归一、默认启用集合）
//! - `packages/shared/src/node/officialPluginCache.ts`（缓存根回退与版本目录扫描）
//! - `packages/services/src/plugins/installedPluginRoots.ts`（`installed_plugins.json`）
//! - `packages/services/src/skills/skillsService.ts` 与 `subagentsService.ts` 的插件段
//!
//! 行为差异（skills 只要扁平版本目录、subagents 要按名字分组的版本根）在
//! `official_cache_version_roots` 与 `scan_official_plugin_cache_roots` 两个入口分别保留。

pub mod js;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub const MODE_INLINE_PLUGIN_MARKETPLACE: &str = "inline";
pub const MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID: &str = "mode-plugins-official";
pub const MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID: &str = "zcode-plugins-official";

pub const PLUGIN_MANIFEST_PATHS: [&str; 4] = [
    ".mode-plugin/plugin.json",
    ".zcode-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".codex-plugin/plugin.json",
];

const INSTALLED_PLUGINS_FILE: &str = "installed_plugins.json";

const DEFAULT_ENABLED_OFFICIAL_PLUGIN_NAMES: [&str; 10] = [
    "browser-use",
    "image-search",
    "documents",
    "pdf",
    "presentations",
    "spreadsheets",
    "node-repl-host",
    "skill-creator",
    "plugin-creator",
    "mode-guide",
];

/// `<name>@zcode-plugins-official` → `<name>@mode-plugins-official`；其余 id 原样返回。
pub fn canonical_plugin_id(id: &str) -> String {
    let suffix = format!("@{MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID}");
    match id.strip_suffix(&suffix) {
        Some(head) => format!("{head}@{MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID}"),
        None => id.to_string(),
    }
}

pub fn is_default_enabled_official_plugin_id(id: &str) -> bool {
    let canonical = canonical_plugin_id(id);
    DEFAULT_ENABLED_OFFICIAL_PLUGIN_NAMES
        .iter()
        .any(|name| canonical == format!("{name}@{MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID}"))
}

#[derive(Debug, Clone)]
pub struct PluginConfig {
    pub dirs: Vec<String>,
    pub enabled: bool,
    pub enabled_plugins: BTreeMap<String, bool>,
    pub storage_dir: String,
    pub suppressed_builtins: Vec<String>,
}

pub fn read_plugin_config(config: &serde_json::Map<String, Value>) -> PluginConfig {
    let plugins = config
        .get("plugins")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut enabled_plugins = BTreeMap::new();
    if let Some(entries) = plugins.get("enabledPlugins").and_then(Value::as_object) {
        for (id, value) in entries {
            if let Some(flag) = value.as_bool() {
                enabled_plugins.insert(canonical_plugin_id(id), flag);
            }
        }
    }

    PluginConfig {
        dirs: read_string_array(plugins.get("dirs")),
        enabled: plugins
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        enabled_plugins,
        storage_dir: read_storage_dir(config),
        suppressed_builtins: read_string_array(plugins.get("suppressedBuiltins"))
            .iter()
            .map(|id| canonical_plugin_id(id))
            .collect(),
    }
}

fn read_string_array(value: Option<&Value>) -> Vec<String> {
    match value.and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    }
}

fn read_storage_dir(config: &serde_json::Map<String, Value>) -> String {
    let storage = config.get("storage").and_then(Value::as_object);
    match storage
        .and_then(|object| object.get("dir"))
        .and_then(Value::as_str)
    {
        Some(dir) if !js::js_trim(dir).is_empty() => dir.to_string(),
        _ => "~/.mode".to_string(),
    }
}

/// `path.startsWith("~/")` 展开 home；相对路径按当前工作目录 `resolve()`。
pub fn resolve_config_path(home_dir: &Path, path: &str) -> PathBuf {
    let expanded: PathBuf = match path.strip_prefix("~/") {
        Some(rest) => home_dir.join(rest),
        None => PathBuf::from(path),
    };
    if js::is_absolute(&expanded.to_string_lossy()) {
        return expanded;
    }
    let base = std::env::current_dir().unwrap_or_default();
    js::normalize_lexical(&base.join(expanded))
}

pub fn resolve_cli_storage_root(home_dir: &Path, storage_dir: &str) -> PathBuf {
    let storage_root = resolve_config_path(home_dir, storage_dir);
    let is_cli = storage_root
        .file_name()
        .map(|name| name == "cli")
        .unwrap_or(false);
    if is_cli {
        storage_root
    } else {
        storage_root.join("cli")
    }
}

pub fn resolve_plugin_storage_root(home_dir: &Path, storage_dir: &str) -> PathBuf {
    resolve_cli_storage_root(home_dir, storage_dir).join("plugins")
}

/// 官方缓存根：新目录优先，不存在则回退旧目录；都不存在时仍返回新路径（扫描得空）。
pub fn resolve_official_plugin_cache_root(plugin_storage_root: &Path) -> PathBuf {
    let current = plugin_storage_root
        .join("cache")
        .join(MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID);
    if js::exists(&current) {
        return current;
    }
    let legacy = plugin_storage_root
        .join("cache")
        .join(MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID);
    if js::exists(&legacy) {
        return legacy;
    }
    current
}

/// `<plugins>/cache/<官方市场>/<name>/<version>/` 的扁平版本目录。
///
/// skills 侧口径：不按名字分组、不过滤备份/临时目录，按路径 localeCompare 升序。
pub fn official_cache_version_roots(plugin_storage_root: &Path) -> Vec<PathBuf> {
    let cache_root = resolve_official_plugin_cache_root(plugin_storage_root);
    let Ok(plugin_entries) = std::fs::read_dir(&cache_root) else {
        return Vec::new();
    };

    let mut roots: Vec<PathBuf> = Vec::new();
    for plugin_entry in plugin_entries.flatten() {
        if !is_directory(&plugin_entry) {
            continue;
        }
        let plugin_dir = plugin_entry.path();
        let Ok(version_entries) = std::fs::read_dir(&plugin_dir) else {
            continue;
        };
        for version_entry in version_entries.flatten() {
            if is_directory(&version_entry) {
                roots.push(version_entry.path());
            }
        }
    }
    roots.sort_by(|left, right| {
        js::locale_compare(&left.to_string_lossy(), &right.to_string_lossy())
    });
    roots
}

/// 按插件名分组的官方缓存根（subagents 口径）：过滤备份/临时目录，
/// 版本目录按数字感知降序，最后按名字 localeCompare 升序。
#[derive(Debug, Clone)]
pub struct OfficialPluginCacheRoot {
    pub name: String,
    pub version_roots: Vec<PathBuf>,
}

pub fn scan_official_plugin_cache_roots(
    plugin_storage_root: &Path,
) -> Vec<OfficialPluginCacheRoot> {
    let cache_root = resolve_official_plugin_cache_root(plugin_storage_root);
    let Ok(plugin_entries) = std::fs::read_dir(&cache_root) else {
        return Vec::new();
    };

    let mut roots = Vec::new();
    for plugin_entry in plugin_entries.flatten() {
        if !is_directory(&plugin_entry) {
            continue;
        }
        let plugin_dir = plugin_entry.path();
        let Ok(version_entries) = std::fs::read_dir(&plugin_dir) else {
            continue;
        };
        let mut versions: Vec<String> = version_entries
            .flatten()
            .filter(|entry| {
                is_directory(entry)
                    && !is_transient_cache_entry_name(&entry.file_name().to_string_lossy())
            })
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        versions.sort_by(|left, right| js::locale_compare_numeric_desc(left, right));
        if versions.is_empty() {
            continue;
        }
        roots.push(OfficialPluginCacheRoot {
            name: plugin_entry.file_name().to_string_lossy().into_owned(),
            version_roots: versions
                .into_iter()
                .map(|version| plugin_dir.join(version))
                .collect(),
        });
    }
    roots.sort_by(|left, right| js::locale_compare(&left.name, &right.name));
    roots
}

fn is_transient_cache_entry_name(name: &str) -> bool {
    name.contains(".backup") || name.contains(".seed-lock") || name.contains(".tmp-")
}

fn is_directory(entry: &std::fs::DirEntry) -> bool {
    entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
}

#[derive(Debug, Clone)]
pub struct InstalledPluginRecord {
    pub id: String,
    pub install_path: String,
    pub name: Option<String>,
    pub marketplace: Option<String>,
    pub scope: Option<String>,
    pub project_path: Option<String>,
}

/// `installed_plugins.json` 的原始记录（不做 trim 与 id 归一，调用方按各自口径处理）。
pub fn read_installed_plugin_records(plugin_storage_root: &Path) -> Vec<InstalledPluginRecord> {
    let Some(raw) = js::read_text(&plugin_storage_root.join(INSTALLED_PLUGINS_FILE)) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(items) = parsed.get("plugins").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut records = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(id) = object.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(install_path) = object.get("installPath").and_then(Value::as_str) else {
            continue;
        };
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string);
        let marketplace = object
            .get("marketplace")
            .and_then(Value::as_str)
            .map(str::to_string);
        let scope = object
            .get("scope")
            .and_then(Value::as_str)
            .map(str::to_string);
        let project_path = object
            .get("projectPath")
            .and_then(Value::as_str)
            .map(str::to_string);
        records.push(InstalledPluginRecord {
            id: id.to_string(),
            install_path: install_path.to_string(),
            name,
            marketplace,
            scope,
            project_path,
        });
    }
    records
}

/// 读第一个存在的插件清单（返回原始对象；名字/字段校验由调用方完成）。
pub fn read_plugin_manifest_object(root_path: &Path) -> Option<serde_json::Map<String, Value>> {
    for relative in PLUGIN_MANIFEST_PATHS {
        let candidate = root_path.join(relative);
        let Some(raw) = js::read_text(&candidate) else {
            continue;
        };
        match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Object(object)) => return Some(object),
            _ => continue,
        }
    }
    None
}

/// `/^[a-z0-9][a-z0-9._-]{0,127}$/`。
pub fn is_valid_plugin_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return false;
    }
    if !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
        return false;
    }
    bytes[1..].iter().all(|value| {
        value.is_ascii_lowercase() || value.is_ascii_digit() || matches!(value, b'.' | b'_' | b'-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_legacy_marketplace_only() {
        assert_eq!(
            canonical_plugin_id("pdf@zcode-plugins-official"),
            "pdf@mode-plugins-official"
        );
        assert_eq!(canonical_plugin_id("pdf@inline"), "pdf@inline");
        assert!(is_default_enabled_official_plugin_id(
            "node-repl-host@zcode-plugins-official"
        ));
        assert!(!is_default_enabled_official_plugin_id(
            "computer-use@mode-plugins-official"
        ));
    }

    #[test]
    fn plugin_name_regex_rejects_uppercase() {
        assert!(is_valid_plugin_name("pdf"));
        assert!(is_valid_plugin_name("a.b_c-d"));
        assert!(!is_valid_plugin_name("Pdf"));
        assert!(!is_valid_plugin_name(".pdf"));
        assert!(!is_valid_plugin_name(""));
    }

    #[test]
    fn transient_cache_entries_are_filtered() {
        assert!(is_transient_cache_entry_name("1.0.0.backup"));
        assert!(is_transient_cache_entry_name("2.0.0.tmp-1"));
        assert!(is_transient_cache_entry_name("x.seed-lock"));
        assert!(!is_transient_cache_entry_name("1.0.0"));
    }

    #[test]
    fn numeric_version_sort_is_descending() {
        // 降序：10 排在 2 前面，9 排在 10 前面。
        assert_eq!(
            js::locale_compare_numeric_desc("10.0.0", "2.0.0"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            js::locale_compare_numeric_desc("9.0.0", "10.0.0"),
            std::cmp::Ordering::Greater
        );
    }
}
