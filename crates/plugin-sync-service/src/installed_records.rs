//! `installed_plugins.json` 记录层（LT-15 P3 首切）。
//!
//! 真源：`apps/mode-cli/packages/adapters/src/plugins/marketplace.ts` 的
//! `loadInstalledPluginsSync` / `normalizeInstalledPluginsState` / `saveInstalledPlugins`。
//! 读时归一（旧 map 形态、官方市场改名、scope 别名），写回原子化；与 TS 写盘字节对齐
//! （两空格缩进 + 尾随换行）。安装/卸载编排在此基础上分层。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const INSTALLED_PLUGINS_FILE: &str = "installed_plugins.json";
pub const MODE_OFFICIAL_PLUGIN_MARKETPLACE: &str = "mode-plugins-official";
pub const MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID: &str = "zcode-plugins-official";
pub const DEFAULT_VERSION: &str = "0.0.0";

/// 安装记录。`scope` 只认 `user`/`workspace`；`source` 是未知形状，原样保留。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPluginRecord {
    pub id: String,
    pub name: String,
    pub marketplace: String,
    pub version: String,
    pub install_path: String,
    pub installed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub scope: InstalledPluginScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_transaction_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstalledPluginScope {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "workspace")]
    Workspace,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InstalledPluginsState {
    pub version: u64,
    pub plugins: Vec<InstalledPluginRecord>,
}

/// `<name>@<marketplace>` 拆分：最后一个 `@` 前是 name；无 `@`、开头 `@`、末尾 `@` 都拒绝。
pub fn parse_plugin_id(id: &str) -> Result<(String, String), String> {
    let Some(at) = id.rfind('@') else {
        return Err(format!("Plugin id must use <name>@<marketplace>: {id}"));
    };
    if at == 0 || at == id.len() - 1 {
        return Err(format!("Plugin id must use <name>@<marketplace>: {id}"));
    }
    Ok((id[..at].to_string(), id[at + 1..].to_string()))
}

/// 官方市场 id 归一（读时归一；不改写用户数据）。
pub fn canonical_official_marketplace_id(id: &str) -> String {
    if id == MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID {
        MODE_OFFICIAL_PLUGIN_MARKETPLACE.to_string()
    } else {
        id.to_string()
    }
}

/// 插件 id 的官方市场段归一（`<name>@zcode-plugins-official` → `<name>@mode-plugins-official`）。
pub fn canonical_plugin_id(id: &str) -> String {
    let suffix = format!("@{MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID}");
    if id.ends_with(&suffix) {
        format!(
            "{}@{MODE_OFFICIAL_PLUGIN_MARKETPLACE}",
            &id[..id.len() - suffix.len()]
        )
    } else {
        id.to_string()
    }
}

pub fn installed_plugins_file(storage_root: &Path) -> PathBuf {
    storage_root.join(INSTALLED_PLUGINS_FILE)
}

/// 读 installed_plugins.json：缺失/坏 JSON 都按空状态处理（TS readJsonFileSync 同口径）。
pub fn load_installed_plugins_state(storage_root: &Path) -> InstalledPluginsState {
    let parsed = read_json(storage_root, INSTALLED_PLUGINS_FILE);
    normalize_installed_plugins_state(&parsed)
}

pub fn load_installed_plugin_records(storage_root: &Path) -> Vec<InstalledPluginRecord> {
    load_installed_plugins_state(storage_root).plugins
}

/// 原子写回（两空格缩进 + 尾随换行，与 TS writeJsonFile 字节对齐）。
pub fn save_installed_plugins_state(
    storage_root: &Path,
    state: &InstalledPluginsState,
) -> Result<(), String> {
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(state).map_err(|error| error.to_string())?
    );
    atomic_file::write_text_atomic(
        &installed_plugins_file(storage_root),
        &text,
        &atomic_file::WriteOptions::default(),
    )
    .map_err(|error| error.to_string())
}

fn read_json(storage_root: &Path, file_name: &str) -> Value {
    let path = storage_root.join(file_name);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    }
}

/// 读时归一：非对象 → 空；plugins 为对象（旧 map 形态）→ 按 entry 归一；
/// plugins 为数组 → 过滤合法条目并归一官方市场段。
pub fn normalize_installed_plugins_state(value: &Value) -> InstalledPluginsState {
    let Some(record) = value.as_object() else {
        return InstalledPluginsState {
            version: 1,
            plugins: Vec::new(),
        };
    };
    match record.get("plugins") {
        Some(Value::Object(entries)) => InstalledPluginsState {
            version: 1,
            plugins: entries
                .iter()
                .flat_map(|(plugin_id, entry)| normalize_record_from_map(plugin_id, entry))
                .collect(),
        },
        Some(Value::Array(plugins)) => InstalledPluginsState {
            version: 1,
            plugins: plugins
                .iter()
                .filter(|entry| is_installed_plugin_record(entry))
                .filter_map(|entry| {
                    serde_json::from_value::<InstalledPluginRecord>(entry.clone()).ok()
                })
                .map(|mut record| {
                    record.id = canonical_plugin_id(&record.id);
                    record.marketplace = canonical_official_marketplace_id(&record.marketplace);
                    record
                })
                .collect(),
        },
        _ => InstalledPluginsState {
            version: 1,
            plugins: Vec::new(),
        },
    }
}

fn normalize_record_from_map(plugin_id: &str, entry: &Value) -> Vec<InstalledPluginRecord> {
    let entries: Vec<Value> = match entry {
        Value::Array(items) => items.clone(),
        item @ Value::Object(_) => vec![item.clone()],
        _ => return Vec::new(),
    };
    entries
        .into_iter()
        .filter_map(|item| {
            let record = item.as_object()?;
            let install_path = record
                .get("installPath")
                .and_then(Value::as_str)
                .unwrap_or("");
            if install_path.is_empty() {
                return None;
            }
            let (name, marketplace) = parse_plugin_id(plugin_id).ok()?;
            let scope = match record.get("scope").and_then(Value::as_str) {
                Some("project") | Some("local") => InstalledPluginScope::Workspace,
                _ => InstalledPluginScope::User,
            };
            let mut canonical = InstalledPluginRecord {
                id: plugin_id.to_string(),
                name,
                marketplace,
                version: record
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or(DEFAULT_VERSION)
                    .to_string(),
                install_path: install_path.to_string(),
                installed_at: record
                    .get("installedAt")
                    .and_then(Value::as_str)
                    .unwrap_or("1970-01-01T00:00:00.000Z")
                    .to_string(),
                updated_at: record
                    .get("lastUpdated")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                scope,
                dependencies: None,
                source: None,
                cache_transaction_id: None,
            };
            canonical.id = canonical_plugin_id(&canonical.id);
            canonical.marketplace = canonical_official_marketplace_id(&canonical.marketplace);
            Some(canonical)
        })
        .collect()
}

fn is_installed_plugin_record(value: &Value) -> bool {
    let Some(record) = value.as_object() else {
        return false;
    };
    [
        "id",
        "name",
        "marketplace",
        "version",
        "installPath",
        "installedAt",
    ]
    .iter()
    .all(|key| record.get(*key).is_some_and(Value::is_string))
        && matches!(
            record.get("scope").and_then(Value::as_str),
            Some("user") | Some("workspace")
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "plugin-sync-installed-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    #[test]
    fn parse_plugin_id_rejects_missing_and_edge_at_signs() {
        assert_eq!(
            parse_plugin_id("foo@bar").expect("valid"),
            ("foo".to_string(), "bar".to_string())
        );
        assert_eq!(
            parse_plugin_id("foo@bar@baz").expect("valid"),
            ("foo@bar".to_string(), "baz".to_string())
        );
        assert!(parse_plugin_id("no-at").is_err());
        assert!(parse_plugin_id("@bar").is_err());
        assert!(parse_plugin_id("foo@").is_err());
    }

    #[test]
    fn missing_or_broken_file_loads_as_empty() {
        let root = tmp_root("missing");
        assert!(load_installed_plugin_records(&root).is_empty());
        std::fs::write(installed_plugins_file(&root), "not json").expect("write");
        assert!(load_installed_plugin_records(&root).is_empty());
    }

    #[test]
    fn save_load_round_trip_preserves_records() {
        let root = tmp_root("round-trip");
        let state = InstalledPluginsState {
            version: 1,
            plugins: vec![InstalledPluginRecord {
                id: "demo@mode-plugins-official".to_string(),
                name: "demo".to_string(),
                marketplace: "mode-plugins-official".to_string(),
                version: "1.2.3".to_string(),
                install_path: "/plugins/demo".to_string(),
                installed_at: "2026-11-21T00:00:00.000Z".to_string(),
                updated_at: Some("2026-11-21T01:00:00.000Z".to_string()),
                scope: InstalledPluginScope::User,
                dependencies: Some(vec!["helper".to_string()]),
                source: Some(json!({ "source": "file", "path": "/tmp/demo" })),
                cache_transaction_id: Some("tx-1".to_string()),
            }],
        };
        save_installed_plugins_state(&root, &state).expect("save");
        let loaded = load_installed_plugins_state(&root);
        assert_eq!(loaded, state);
        // 字节对齐：两空格缩进 + 尾随换行。
        let bytes = std::fs::read(installed_plugins_file(&root)).expect("read");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(text.ends_with("\n"));
        assert!(text.contains("  \"installPath\": \"/plugins/demo\""));
    }

    #[test]
    fn normalizes_legacy_map_form_with_scope_aliases_and_canonical_ids() {
        let value = json!({
            "version": 1,
            "plugins": {
                "demo@zcode-plugins-official": {
                    "installPath": "/plugins/demo",
                    "installedAt": "2026-01-01T00:00:00.000Z",
                    "lastUpdated": "2026-02-01T00:00:00.000Z",
                    "scope": "project",
                },
                "helper@other": [
                    { "installPath": "/plugins/helper", "version": "2.0.0" },
                    { "installPath": "" },
                ],
                "bad-id": { "installPath": "/plugins/bad" },
            },
        });
        let state = normalize_installed_plugins_state(&value);
        let by_id = |id: &str| {
            state
                .plugins
                .iter()
                .find(|record| record.id == id)
                .cloned()
                .expect("record")
        };
        let demo = by_id("demo@mode-plugins-official");
        assert_eq!(demo.marketplace, "mode-plugins-official");
        assert_eq!(demo.scope, InstalledPluginScope::Workspace);
        assert_eq!(demo.updated_at.as_deref(), Some("2026-02-01T00:00:00.000Z"));
        let helper = by_id("helper@other");
        assert_eq!(helper.version, "2.0.0");
        assert!(state.plugins.iter().all(|record| record.id != "bad-id"));
    }

    #[test]
    fn array_form_filters_invalid_entries() {
        let value = json!({
            "version": 1,
            "plugins": [
                { "id": "ok@other", "name": "ok", "marketplace": "other", "version": "1",
                  "installPath": "/p", "installedAt": "t", "scope": "user" },
                { "id": "bad-scope@other", "name": "b", "marketplace": "other", "version": "1",
                  "installPath": "/p", "installedAt": "t", "scope": "local" },
                { "id": "no-path@other", "name": "n", "marketplace": "other", "version": "1",
                  "installedAt": "t", "scope": "user" },
                "not-an-object",
            ],
        });
        let state = normalize_installed_plugins_state(&value);
        assert_eq!(state.plugins.len(), 1);
        assert_eq!(state.plugins[0].id, "ok@other");
    }
}
