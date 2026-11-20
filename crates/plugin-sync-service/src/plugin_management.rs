//! 插件安装/卸载编排（LT-15 P3 切二：卸载编排首切）。
//!
//! 真源：`apps/mode-cli/packages/adapters/src/plugins/marketplace.ts` 的
//! `uninstallMarketplacePlugin` / `getPluginDataDir` / `getPluginCacheDir` /
//! `legacyOfficialPluginId` / `sanitizePluginId`。
//!
//! 卸载语义：先写记录（splice + 原子保存），再删文件（installPath 与 data 目录，
//! `removeCache=false` 时跳过删除只摘记录；`keepData=true` 保留用户数据目录）。
//! 记录层复用 [`crate::installed_records`]。

use std::path::{Path, PathBuf};

use crate::config_write_back::{remove_plugin_from_config, remove_suppressed_builtin};
use crate::installed_records::{
    load_installed_plugins_state, save_installed_plugins_state, InstalledPluginRecord,
    MODE_OFFICIAL_PLUGIN_MARKETPLACE, MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID,
};

/// 官方市场改名后的 id 段对应改名前的旧 id 段（缓存/data 目录兜底用）。
fn legacy_official_plugin_id(plugin_id: &str) -> Option<String> {
    let suffix = format!("@{MODE_OFFICIAL_PLUGIN_MARKETPLACE}");
    plugin_id
        .strip_suffix(&suffix)
        .map(|name| format!("{name}@{MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID}"))
}

/// 路径安全化：白名单字符以外的段替换为 `-`（与 TS sanitizePluginId 同口径）。
pub fn sanitize_plugin_id(plugin_id: &str) -> String {
    plugin_id
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() || matches!(value, '_' | '.' | '@' | '-') {
                value
            } else {
                '-'
            }
        })
        .collect()
}

fn exists(path: &Path) -> bool {
    path.try_exists().unwrap_or(false)
}

/// 官方市场改名后，已装插件的文件仍挂在旧市场名的缓存目录里：新目录存在沿用新目录，
/// 否则旧目录存在沿用旧目录，都不存在落当前目录。
pub fn plugin_cache_dir(
    storage_root: &Path,
    marketplace: &str,
    name: &str,
    version: &str,
) -> PathBuf {
    let current = storage_root
        .join("cache")
        .join(sanitize_plugin_id(marketplace))
        .join(sanitize_plugin_id(name))
        .join(sanitize_plugin_id(version));
    if marketplace == MODE_OFFICIAL_PLUGIN_MARKETPLACE {
        if exists(&current) {
            return current;
        }
        let legacy = storage_root
            .join("cache")
            .join(sanitize_plugin_id(
                MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID,
            ))
            .join(sanitize_plugin_id(name))
            .join(sanitize_plugin_id(version));
        if exists(&legacy) {
            return legacy;
        }
    }
    current
}

/// per-plugin 持久化数据目录：官方市场改名后旧 id 目录兜底（与 cache 同口径）。
pub fn plugin_data_dir(storage_root: &Path, plugin_id: &str) -> PathBuf {
    let current = storage_root
        .join("data")
        .join(sanitize_plugin_id(plugin_id));
    if let Some(legacy_id) = legacy_official_plugin_id(plugin_id) {
        let legacy = storage_root
            .join("data")
            .join(sanitize_plugin_id(&legacy_id));
        if !exists(&current) && exists(&legacy) {
            return legacy;
        }
    }
    current
}

/// 卸载编排：定位记录 → splice → 原子保存 →（可选）删安装目录与用户数据目录
/// → config 写回（清 enabledPlugins/options + 清遗留 suppression，让状态自愈）。
///
/// `remove_cache=false` 时只摘记录不删文件（`keepData` 不参与该分支，与 TS 一致）。
/// 找不到 `plugin_id` 返回 `Ok(None)`；config 写回失败传 `Err`（TS 同序：卸载成功后
/// 写回报错会让调用方感知，此时记录层已落盘，重试按记录不存在走幂等清理）。
/// `config_path=None` 跳过写回（对拍测试/纯编排复用）。
pub fn uninstall_marketplace_plugin(
    storage_root: &Path,
    plugin_id: &str,
    remove_cache: bool,
    keep_data: bool,
    config_path: Option<&Path>,
) -> Result<Option<InstalledPluginRecord>, String> {
    let mut state = load_installed_plugins_state(storage_root);
    let Some(index) = state
        .plugins
        .iter()
        .position(|record| record.id == plugin_id)
    else {
        return Ok(None);
    };
    let removed = state.plugins.remove(index);
    save_installed_plugins_state(storage_root, &state).map_err(|error| error.to_string())?;
    if remove_cache {
        let _ = std::fs::remove_dir_all(&removed.install_path);
        if !keep_data {
            let _ = std::fs::remove_dir_all(plugin_data_dir(storage_root, plugin_id));
        }
    }
    if let Some(path) = config_path {
        remove_plugin_from_config(path, &removed.id)?;
        remove_suppressed_builtin(path, &removed.id)?;
    }
    Ok(Some(removed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installed_records::{
        normalize_installed_plugins_state, InstalledPluginScope, InstalledPluginsState,
    };
    use serde_json::{json, Value};

    fn tmp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "plugin-management-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    fn seed_state(root: &Path, entries: Value) {
        std::fs::write(
            root.join("installed_plugins.json"),
            serde_json::to_string_pretty(&entries).expect("serialize"),
        )
        .expect("write");
    }

    fn state(root: &Path) -> InstalledPluginsState {
        let value: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("installed_plugins.json")).expect("read"),
        )
        .expect("json");
        normalize_installed_plugins_state(&value)
    }

    #[test]
    fn sanitize_keeps_allowlist_and_replaces_others() {
        assert_eq!(
            sanitize_plugin_id("demo@mode-plugins-official"),
            "demo@mode-plugins-official"
        );
        assert_eq!(sanitize_plugin_id("weird id/路径!"), "weird-id----");
    }

    #[test]
    fn legacy_official_id_only_for_official_suffix() {
        assert_eq!(
            legacy_official_plugin_id("demo@mode-plugins-official").as_deref(),
            Some("demo@zcode-plugins-official")
        );
        assert!(legacy_official_plugin_id("demo@other").is_none());
    }

    #[test]
    fn data_dir_prefers_legacy_when_current_missing() {
        let root = tmp_root("data-dir");
        let legacy = root
            .join("data")
            .join(sanitize_plugin_id("demo@zcode-plugins-official"));
        std::fs::create_dir_all(&legacy).expect("legacy");
        let resolved = plugin_data_dir(&root, "demo@mode-plugins-official");
        assert_eq!(resolved, legacy);

        std::fs::create_dir_all(
            root.join("data")
                .join(sanitize_plugin_id("demo@mode-plugins-official")),
        )
        .expect("current");
        assert_eq!(
            plugin_data_dir(&root, "demo@mode-plugins-official"),
            root.join("data")
                .join(sanitize_plugin_id("demo@mode-plugins-official"))
        );
    }

    #[test]
    fn cache_dir_falls_back_to_legacy_marketplace_name() {
        let root = tmp_root("cache-dir");
        let legacy = root
            .join("cache")
            .join("zcode-plugins-official")
            .join("demo")
            .join("1.0.0");
        std::fs::create_dir_all(&legacy).expect("legacy");
        assert_eq!(
            plugin_cache_dir(&root, "mode-plugins-official", "demo", "1.0.0"),
            legacy
        );
        let fresh = plugin_cache_dir(&root, "other", "demo", "1.0.0");
        assert_eq!(
            fresh,
            root.join("cache").join("other").join("demo").join("1.0.0")
        );
    }

    #[test]
    fn uninstall_clears_config_and_leftover_suppression() {
        let root = tmp_root("uninstall-config");
        let install_path = root.join("plugins").join("demo");
        std::fs::create_dir_all(&install_path).expect("install");
        seed_state(
            &root,
            json!({
                "version": 1,
                "plugins": [
                    { "id": "demo@other", "name": "demo", "marketplace": "other", "version": "1",
                      "installPath": install_path.display().to_string(), "installedAt": "t",
                      "scope": "user" },
                ],
            }),
        );
        let config_path = root.join("config.json");
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&json!({
                "plugins": {
                    "enabledPlugins": { "demo@other": true },
                    "options": { "demo@other": { "key": "value" } },
                    "suppressedBuiltins": ["demo@other", "keep@other"],
                },
            }))
            .unwrap(),
        )
        .unwrap();
        uninstall_marketplace_plugin(&root, "demo@other", true, false, Some(&config_path))
            .expect("removed")
            .expect("record");
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read"))
                .expect("json");
        assert!(config["plugins"]["enabledPlugins"]
            .get("demo@other")
            .is_none());
        assert!(config["plugins"]["options"].get("demo@other").is_none());
        // 遗留 suppression 被清（状态自愈），无关条目保留。
        let suppressed = config["plugins"]["suppressedBuiltins"]
            .as_array()
            .expect("array");
        assert_eq!(suppressed, &[json!("keep@other")]);
    }

    #[test]
    fn uninstall_removes_record_and_files_with_data_kept() {
        let root = tmp_root("uninstall-keep");
        let install_path = root.join("plugins").join("demo");
        std::fs::create_dir_all(&install_path).expect("install");
        let data_dir = root.join("data").join(sanitize_plugin_id("demo@other"));
        std::fs::create_dir_all(&data_dir).expect("data");
        seed_state(
            &root,
            json!({
                "version": 1,
                "plugins": [
                    { "id": "demo@other", "name": "demo", "marketplace": "other", "version": "1.0.0",
                      "installPath": install_path.display().to_string(), "installedAt": "t",
                      "scope": "user" },
                    { "id": "keep@other", "name": "keep", "marketplace": "other", "version": "1",
                      "installPath": "/p", "installedAt": "t", "scope": "user" },
                ],
            }),
        );
        let removed = uninstall_marketplace_plugin(&root, "demo@other", true, true, None)
            .expect("removed")
            .expect("record");
        assert_eq!(removed.id, "demo@other");
        assert!(!install_path.exists(), "安装目录应删除");
        assert!(data_dir.exists(), "keepData=true 保留数据目录");
        let after = state(&root);
        assert_eq!(after.plugins.len(), 1);
        assert_eq!(after.plugins[0].id, "keep@other");
        assert!(
            uninstall_marketplace_plugin(&root, "demo@other", true, true, None)
                .expect("idempotent")
                .is_none()
        );
    }

    #[test]
    fn uninstall_without_remove_cache_keeps_files_but_removes_record() {
        let root = tmp_root("uninstall-nocache");
        let install_path = root.join("plugins").join("demo");
        std::fs::create_dir_all(&install_path).expect("install");
        seed_state(
            &root,
            json!({
                "version": 1,
                "plugins": [
                    { "id": "demo@other", "name": "demo", "marketplace": "other", "version": "1",
                      "installPath": install_path.display().to_string(), "installedAt": "t",
                      "scope": "user" },
                ],
            }),
        );
        let removed = uninstall_marketplace_plugin(&root, "demo@other", false, false, None)
            .expect("removed")
            .expect("record");
        assert_eq!(removed.id, "demo@other");
        assert!(install_path.exists(), "removeCache=false 不删文件");
        assert!(state(&root).plugins.is_empty());
    }

    #[test]
    fn uninstall_removes_data_dir_by_default() {
        let root = tmp_root("uninstall-data");
        let install_path = root.join("plugins").join("demo");
        std::fs::create_dir_all(&install_path).expect("install");
        let data_dir = root.join("data").join(sanitize_plugin_id("demo@other"));
        std::fs::create_dir_all(&data_dir).expect("data");
        seed_state(
            &root,
            json!({
                "version": 1,
                "plugins": [
                    { "id": "demo@other", "name": "demo", "marketplace": "other", "version": "1",
                      "installPath": install_path.display().to_string(), "installedAt": "t",
                      "scope": "user" },
                ],
            }),
        );
        let removed = uninstall_marketplace_plugin(&root, "demo@other", true, false, None)
            .expect("removed")
            .expect("record");
        assert_eq!(removed.id, "demo@other");
        assert!(!install_path.exists());
        assert!(!data_dir.exists(), "keepData=false 删用户数据目录");
    }

    #[test]
    fn state_save_is_atomic_and_readable_after() {
        let root = tmp_root("atomic-save");
        seed_state(
            &root,
            json!({
                "version": 1,
                "plugins": [
                    { "id": "demo@other", "name": "demo", "marketplace": "other", "version": "1",
                      "installPath": "/p", "installedAt": "t", "scope": "user" },
                ],
            }),
        );
        let state = load_installed_plugins_state(&root);
        assert_eq!(state.plugins.len(), 1);
        assert_eq!(state.plugins[0].scope, InstalledPluginScope::User);
        // 覆盖写后仍然可读（原子替换）。
        save_installed_plugins_state(&root, &state).expect("save");
        assert_eq!(load_installed_plugins_state(&root).plugins.len(), 1);
    }
}
