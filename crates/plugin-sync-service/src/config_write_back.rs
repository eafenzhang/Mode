//! 插件安装/卸载的 config 写回（LT-15 P3 切四）。
//!
//! 真源：`apps/mode-cli/packages/adapters/src/config/file-config.adapter.ts` 的
//! `enablePluginsByDefaultInFileConfig` / `removePluginFromFileConfig` /
//! `addSuppressedBuiltinInFileConfig` / `removeSuppressedBuiltinInFileConfig`。
//! 只操作 Mode 配置 JSON 的 `plugins` 段，不碰 provider/模型等其他段；
//! 写回走 `atomic-file`（两空格缩进 + 尾随换行，与 TS 同形）。

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::installed_records::{
    MODE_OFFICIAL_PLUGIN_MARKETPLACE, MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID,
};

/// 官方插件 id 归一（name 别名 + 市场段归一，读时归一不改写数据）。
pub fn canonicalize_config_plugin_id(id: &str) -> String {
    let (name, marketplace) = match id.rfind('@') {
        Some(at) if at > 0 && at < id.len() - 1 => (&id[..at], &id[at + 1..]),
        _ => return id.to_string(),
    };
    let name = if name == "mode-cua" {
        "computer-use"
    } else {
        name
    };
    let marketplace = if marketplace == MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID {
        MODE_OFFICIAL_PLUGIN_MARKETPLACE
    } else {
        marketplace
    };
    format!("{name}@{marketplace}")
}

/// 同一插件在用户配置里可能出现的全部存量写法（含改名前的名字与市场段）。
pub fn config_plugin_id_aliases(id: &str) -> Vec<String> {
    let canonical = canonicalize_config_plugin_id(id);
    // name 独立持有，避免借用 canonical 后又 move（官方改名 + mode-cua 别名展开）。
    let name = match canonical.rfind('@') {
        Some(at) => canonical[..at].to_string(),
        None => return vec![id.to_string()],
    };
    let names: Vec<String> = if name == "computer-use" {
        vec!["computer-use".to_string(), "mode-cua".to_string()]
    } else {
        vec![name]
    };
    let mut ids = std::collections::HashSet::new();
    ids.insert(canonical);
    for n in &names {
        ids.insert(format!("{n}@{MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID}"));
        ids.insert(format!("{n}@{MODE_OFFICIAL_PLUGIN_MARKETPLACE}"));
    }
    ids.into_iter().collect()
}

fn read_config(file_path: &Path) -> Value {
    std::fs::read_to_string(file_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

fn write_config(file_path: &Path, value: &Value) -> Result<(), String> {
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    atomic_file::write_text_atomic(file_path, &text, &atomic_file::WriteOptions::default())
        .map_err(|error| error.to_string())
}

fn object_or_default(value: Option<&Value>) -> HashMap<String, Value> {
    value
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// 安装默认启用：把 `id → true` 写入 `plugins.enabledPlugins`（已有显式设置的 id 跳过）。
pub fn enable_plugins_in_config(
    file_path: &Path,
    plugin_ids: &[String],
) -> Result<Vec<String>, String> {
    if plugin_ids.is_empty() {
        return Ok(Vec::new());
    }
    let parsed = read_config(file_path);
    let plugins = object_or_default(parsed.get("plugins"));
    let enabled_plugins = object_or_default(plugins.get("enabledPlugins"));
    let enabled_ids: Vec<String> = plugin_ids
        .iter()
        .filter(|id| !enabled_plugins.contains_key(*id))
        .cloned()
        .collect();
    if enabled_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut next_enabled = enabled_plugins;
    for id in &enabled_ids {
        next_enabled.insert(id.clone(), Value::Bool(true));
    }
    let next = merge_plugins(&parsed, &plugins, Some(&next_enabled), None);
    write_config(file_path, &next)?;
    Ok(enabled_ids)
}

/// 卸载：从 `plugins.enabledPlugins` 和 `plugins.options` 移除该插件的全部别名。
pub fn remove_plugin_from_config(file_path: &Path, plugin_id: &str) -> Result<(), String> {
    let parsed = read_config(file_path);
    let plugins = object_or_default(parsed.get("plugins"));
    let enabled_plugins = object_or_default(plugins.get("enabledPlugins"));
    let options = object_or_default(plugins.get("options"));
    let aliases = config_plugin_id_aliases(plugin_id);
    let removed_enabled = aliases.iter().any(|id| enabled_plugins.contains_key(id));
    let removed_options = aliases.iter().any(|id| options.contains_key(id));
    if !removed_enabled && !removed_options {
        return Ok(());
    }
    let mut next_enabled = enabled_plugins;
    for id in &aliases {
        next_enabled.remove(id);
    }
    let mut next_options = options;
    for id in &aliases {
        next_options.remove(id);
    }
    let next = merge_plugins(&parsed, &plugins, Some(&next_enabled), Some(&next_options));
    write_config(file_path, &next)
}

/// 内置插件卸载：把 id 加到 `plugins.suppressedBuiltins`（归一 + 移除旧别名）。
pub fn add_suppressed_builtin(file_path: &Path, plugin_id: &str) -> Result<(), String> {
    let parsed = read_config(file_path);
    let plugins = object_or_default(parsed.get("plugins"));
    let canonical = canonicalize_config_plugin_id(plugin_id);
    let aliases = config_plugin_id_aliases(&canonical);
    let current = plugins
        .get("suppressedBuiltins")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let retained: Vec<String> = current
        .iter()
        .filter(|id| !aliases.contains(id))
        .cloned()
        .collect();
    if retained.len() == current.len() && current.contains(&canonical) {
        return Ok(());
    }
    let mut next_suppressed = retained;
    next_suppressed.push(canonical.clone());
    let next = merge_plugins(&parsed, &plugins, None, None);
    let mut next_object = next;
    if let Some(plugins_object) = next_object
        .get_mut("plugins")
        .and_then(Value::as_object_mut)
    {
        plugins_object.insert(
            "suppressedBuiltins".to_string(),
            Value::Array(next_suppressed.into_iter().map(Value::String).collect()),
        );
    }
    write_config(file_path, &next_object)
}

/// 清除遗留 suppression（卸载 marketplace 记录后让状态自愈）：从
/// `plugins.suppressedBuiltins` 移除该插件的全部别名；无变化不写盘。
pub fn remove_suppressed_builtin(file_path: &Path, plugin_id: &str) -> Result<(), String> {
    let parsed = read_config(file_path);
    let plugins = object_or_default(parsed.get("plugins"));
    let aliases = config_plugin_id_aliases(plugin_id);
    let current = plugins
        .get("suppressedBuiltins")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let next_suppressed: Vec<String> = current
        .iter()
        .filter(|id| !aliases.contains(id))
        .cloned()
        .collect();
    if next_suppressed.len() == current.len() {
        return Ok(());
    }
    let next = merge_plugins(&parsed, &plugins, None, None);
    let mut next_object = next;
    if let Some(plugins_object) = next_object
        .get_mut("plugins")
        .and_then(Value::as_object_mut)
    {
        plugins_object.insert(
            "suppressedBuiltins".to_string(),
            Value::Array(next_suppressed.into_iter().map(Value::String).collect()),
        );
    }
    write_config(file_path, &next_object)
}

/// 合并 plugins 段：保留根上的其他键，替换 enabledPlugins / options（如果传了）。
fn merge_plugins(
    parsed: &Value,
    plugins: &HashMap<String, Value>,
    enabled: Option<&HashMap<String, Value>>,
    options: Option<&HashMap<String, Value>>,
) -> Value {
    let mut next_plugins = plugins.clone();
    if let Some(enabled) = enabled {
        next_plugins.insert(
            "enabledPlugins".to_string(),
            Value::Object(
                enabled
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        );
    }
    if let Some(options) = options {
        next_plugins.insert(
            "options".to_string(),
            Value::Object(
                options
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
        );
    }
    let mut root = parsed.clone();
    root["plugins"] = Value::Object(next_plugins.into_iter().collect());
    root
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn tmp_file(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "plugin-config-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.join("config.json")
    }

    #[test]
    fn enable_plugins_writes_true_entries() {
        let path = tmp_file("enable");
        let ids = vec!["demo@m".to_string(), "helper@m".to_string()];
        let enabled = enable_plugins_in_config(&path, &ids).expect("enable");
        assert_eq!(enabled, ids);
        let config = read_config(&path);
        assert_eq!(config["plugins"]["enabledPlugins"]["demo@m"], true);
        assert_eq!(config["plugins"]["enabledPlugins"]["helper@m"], true);
        // 已有的 id 不重复写。
        let enabled = enable_plugins_in_config(&path, &ids).expect("re-enable");
        assert!(enabled.is_empty());
    }

    #[test]
    fn remove_plugin_clears_enabled_and_options() {
        let path = tmp_file("remove");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "plugins": {
                    "enabledPlugins": { "demo@m": true },
                    "options": { "demo@m": { "key": "value" } },
                },
            }))
            .unwrap(),
        )
        .unwrap();
        remove_plugin_from_config(&path, "demo@m").expect("remove");
        let config = read_config(&path);
        assert!(config["plugins"]["enabledPlugins"].get("demo@m").is_none());
        assert!(config["plugins"]["options"].get("demo@m").is_none());
    }

    #[test]
    fn suppressed_builtin_round_trip() {
        let path = tmp_file("suppress");
        add_suppressed_builtin(&path, "builtin@mode-plugins-official").expect("add");
        add_suppressed_builtin(&path, "builtin@zcode-plugins-official").expect("idempotent");
        let config = read_config(&path);
        let suppressed = config["plugins"]["suppressedBuiltins"]
            .as_array()
            .expect("array");
        assert_eq!(suppressed.len(), 1);
        assert_eq!(suppressed[0], "builtin@mode-plugins-official");
    }

    #[test]
    fn remove_suppressed_builtin_clears_aliases() {
        let path = tmp_file("unsuppress");
        add_suppressed_builtin(&path, "builtin@mode-plugins-official").expect("add");
        remove_suppressed_builtin(&path, "builtin@zcode-plugins-official").expect("remove");
        let config = read_config(&path);
        assert!(config["plugins"]["suppressedBuiltins"]
            .as_array()
            .expect("array")
            .is_empty());
        // 无变化不写盘、不报错。
        remove_suppressed_builtin(&path, "builtin@mode-plugins-official").expect("no-op");
    }

    #[test]
    fn canonicalize_renames_cua_and_marketplace() {
        assert_eq!(
            canonicalize_config_plugin_id("mode-cua@zcode-plugins-official"),
            "computer-use@mode-plugins-official"
        );
        assert_eq!(canonicalize_config_plugin_id("demo@other"), "demo@other");
        let aliases = config_plugin_id_aliases("mode-cua@mode-plugins-official");
        assert!(aliases.contains(&"computer-use@mode-plugins-official".to_string()));
        assert!(aliases.contains(&"mode-cua@zcode-plugins-official".to_string()));
        assert!(aliases.contains(&"mode-cua@mode-plugins-official".to_string()));
    }
}
