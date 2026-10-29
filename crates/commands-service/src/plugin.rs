//! 插件侧的只读数据面：官方插件 id 归一、默认启用集合、官方 cache 根回退，以及
//! `installed_plugins.json` 的读取。
//!
//! 真源：
//! - `packages/shared/src/plugin-marketplaces.ts`（只取 `canonicalPluginId` /
//!   `isDefaultEnabledOfficialPluginId` / `MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID`）
//! - `packages/shared/src/node/officialPluginCache.ts`（`resolveOfficialPluginCacheRoot`）
//! - `packages/services/src/plugins/installedPluginRoots.ts`

use std::path::{Path, PathBuf};

/// 官方市场 id（改名后的当前取值）。
pub const MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID: &str = "mode-plugins-official";
/// 改名前的官方市场 id：存量设置、安装记录与缓存目录仍在用。
pub const MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID: &str = "zcode-plugins-official";

pub fn canonical_plugin_id(id: &str) -> String {
    let suffix = format!("@{MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID}");
    match id.strip_suffix(&suffix) {
        Some(prefix) => format!("{prefix}@{MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID}"),
        None => id.to_string(),
    }
}

/// 必须与 `plugin-marketplaces.ts` 的 `DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS` 逐条一致。
pub const DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS: &[&str] = &[
    "browser-use@mode-plugins-official",
    "image-search@mode-plugins-official",
    "documents@mode-plugins-official",
    "pdf@mode-plugins-official",
    "presentations@mode-plugins-official",
    "spreadsheets@mode-plugins-official",
    "node-repl-host@mode-plugins-official",
    "skill-creator@mode-plugins-official",
    "plugin-creator@mode-plugins-official",
    "mode-guide@mode-plugins-official",
];

pub fn is_default_enabled_official_plugin_id(id: &str) -> bool {
    DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS.contains(&id)
        || DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS.contains(&canonical_plugin_id(id).as_str())
}

/// 官方 cache 根：新目录优先，不存在时沿用旧目录（存量插件文件原地可用），都不存在用新目录。
pub fn resolve_official_plugin_cache_root(plugin_storage_root: &Path) -> PathBuf {
    let current = plugin_storage_root
        .join("cache")
        .join(MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID);
    if current.exists() {
        return current;
    }
    let legacy = plugin_storage_root
        .join("cache")
        .join(MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID);
    if legacy.exists() {
        return legacy;
    }
    current
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPluginRoot {
    pub default_enabled: bool,
    pub marketplace: String,
    pub root_path: PathBuf,
}

/// 读 `<plugin storage>/installed_plugins.json`；任何读取/解析失败都返回空表。
pub fn read_installed_plugin_roots(plugin_storage_root: &Path) -> Vec<InstalledPluginRoot> {
    let Ok(raw) = std::fs::read_to_string(plugin_storage_root.join("installed_plugins.json")) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(plugins) = parsed
        .as_object()
        .and_then(|object| object.get("plugins"))
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };

    let mut roots = Vec::new();
    for item in plugins {
        let Some(record) = item.as_object() else {
            continue;
        };
        let read = |key: &str| {
            record
                .get(key)
                .and_then(|value| value.as_str())
                .map(str::trim)
                .unwrap_or("")
                .to_string()
        };
        let id = read("id");
        let marketplace = read("marketplace");
        let install_path = read("installPath");
        if id.is_empty() || marketplace.is_empty() || install_path.is_empty() {
            continue;
        }
        let path = PathBuf::from(&install_path);
        if !path.has_root() {
            continue;
        }
        // 官方市场改名后，存量记录的 marketplace 还是旧 id：读时归一（id 与 marketplace 一起）。
        let canonical_id = canonical_plugin_id(&id);
        let canonical_marketplace = canonical_id
            .rsplit_once('@')
            .map(|(_, suffix)| suffix.to_string())
            .unwrap_or_default();
        roots.push(InstalledPluginRoot {
            default_enabled: false,
            marketplace: if canonical_marketplace.is_empty() {
                marketplace
            } else {
                canonical_marketplace
            },
            root_path: path,
        });
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_legacy_marketplace_only() {
        assert_eq!(
            canonical_plugin_id("foo@zcode-plugins-official"),
            "foo@mode-plugins-official"
        );
        assert_eq!(canonical_plugin_id("foo@other"), "foo@other");
        assert_eq!(
            canonical_plugin_id("a@zcode-plugins-official@zcode-plugins-official"),
            "a@zcode-plugins-official@mode-plugins-official"
        );
    }

    #[test]
    fn default_enabled_set_matches_canonical_and_legacy_ids() {
        assert!(is_default_enabled_official_plugin_id(
            "pdf@mode-plugins-official"
        ));
        assert!(is_default_enabled_official_plugin_id(
            "pdf@zcode-plugins-official"
        ));
        assert!(!is_default_enabled_official_plugin_id(
            "computer-use@mode-plugins-official"
        ));
    }
}
