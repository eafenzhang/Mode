//! `agents-state.json` 的读取、归一、写回与旧数据迁移。
//!
//! 真源：`subagentsService.ts` 的 `readAgentStateFile` / `writeAgentStateFile` /
//! `normalizeBuiltInSelectionOverrides`、`subagents-types.ts` 的
//! `parsePluginSubagentModelSelectionOverrides`、`subagent-state-migration.ts` 的
//! `importSubagentStateSelections` 与 `shared/node/subagentMarkdownMigration.ts`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use atomic_file::{write_json_atomic, WriteOptions};
use serde_json::{Map, Value};

use crate::model::{
    migrate_legacy_model_provider_id, migrate_official_glm_model_id,
    migrate_subagent_markdown_provider, parse_markdown_selection, parse_selection_value, Selection,
};
use crate::storage::{resolve_subagent_state_file, StorageOptions};
use plugin_discovery::js;

const BUILT_IN_NAMES: [&str; 2] = ["Explore", "general-purpose"];

#[derive(Debug, Clone, Default)]
pub struct AgentsState {
    pub built_in_overrides: BTreeMap<String, Selection>,
    pub plugin_overrides: BTreeMap<String, Selection>,
    pub disabled_ids: Vec<String>,
    /// 旧版本字段与未知键原样保留，避免读写丢字段。
    pub other: Map<String, Value>,
}

impl AgentsState {
    pub fn to_value(&self) -> Value {
        let mut object = self.other.clone();
        let mut built_in = Map::new();
        for (name, selection) in &self.built_in_overrides {
            built_in.insert(name.clone(), serde_json::to_value(selection).unwrap());
        }
        object.insert(
            "builtInModelSelectionOverrides".into(),
            Value::Object(built_in),
        );
        let mut plugin = Map::new();
        for (id, selection) in &self.plugin_overrides {
            plugin.insert(id.clone(), serde_json::to_value(selection).unwrap());
        }
        object.insert(
            "pluginAgentModelSelectionOverrides".into(),
            Value::Object(plugin),
        );
        object.insert(
            "disabledAgentIds".into(),
            Value::Array(
                self.disabled_ids
                    .iter()
                    .map(|id| Value::String(id.clone()))
                    .collect(),
            ),
        );
        Value::Object(object)
    }
}

pub fn read_agent_state(options: &StorageOptions) -> AgentsState {
    let path = resolve_subagent_state_file(options);
    best_effort_migrate_state_file(&path);
    let Some(raw) = js::read_text(&path) else {
        return AgentsState::default();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
        return AgentsState::default();
    };
    let Some(object) = parsed.as_object() else {
        return AgentsState::default();
    };

    let mut built_in_overrides = BTreeMap::new();
    if let Some(structured) = object
        .get("builtInModelSelectionOverrides")
        .and_then(Value::as_object)
    {
        for name in BUILT_IN_NAMES {
            if let Some(selection) = structured.get(name).and_then(parse_selection_value) {
                built_in_overrides.insert(name.to_string(), selection);
            }
        }
    }

    let plugin_overrides = parse_plugin_overrides(object.get("pluginAgentModelSelectionOverrides"));

    let disabled_ids = object
        .get("disabledAgentIds")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|id| !js::js_trim(id).is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let mut other = object.clone();
    other.remove("builtInModelSelectionOverrides");
    other.remove("pluginAgentModelSelectionOverrides");
    other.remove("disabledAgentIds");

    AgentsState {
        built_in_overrides,
        plugin_overrides,
        disabled_ids,
        other,
    }
}

fn parse_plugin_overrides(value: Option<&Value>) -> BTreeMap<String, Selection> {
    let mut overrides = BTreeMap::new();
    let Some(object) = value.and_then(Value::as_object) else {
        return overrides;
    };
    for (id, candidate) in object {
        if !id.starts_with("plugin:") {
            continue;
        }
        if let Some(selection) = parse_selection_value(candidate) {
            overrides.insert(id.clone(), selection);
        }
    }
    overrides
}

pub fn write_agent_state(options: &StorageOptions, state: &AgentsState) -> std::io::Result<()> {
    let path = resolve_subagent_state_file(options);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_json_atomic(&path, &state.to_value(), &WriteOptions::default())
        .map_err(|error| std::io::Error::other(error.to_string()))
}

/// `importSubagentStateSelections`：只在存储导入时解释旧双 map 与旧 Provider。
pub fn import_subagent_state_selections(input: &Map<String, Value>) -> Map<String, Value> {
    let current = input.contains_key("builtInModelSelectionOverrides");
    let mut selections = BTreeMap::new();
    for name in BUILT_IN_NAMES {
        let selection = if current {
            input
                .get("builtInModelSelectionOverrides")
                .and_then(Value::as_object)
                .and_then(|object| object.get(name))
                .and_then(parse_selection_value)
        } else {
            let model = input
                .get("builtInModelOverrides")
                .and_then(Value::as_object)
                .and_then(|object| object.get(name));
            let thought_level = input
                .get("builtInThoughtLevelOverrides")
                .and_then(Value::as_object)
                .and_then(|object| object.get(name));
            parse_markdown_selection(model, thought_level)
        };
        let Some(selection) = selection else {
            continue;
        };
        let original_provider = selection.provider_id.clone();
        let provider_id = if !current && original_provider.starts_with("builtin:") {
            migrate_legacy_model_provider_id(&original_provider)
        } else {
            Some(original_provider.clone())
        };
        let migrated = match provider_id {
            Some(provider_id) => Selection {
                provider_id,
                model_id: if current {
                    selection.model_id
                } else {
                    migrate_official_glm_model_id(&original_provider, &selection.model_id)
                },
                options: selection.options,
            },
            None => selection,
        };
        selections.insert(name.to_string(), migrated);
    }

    let plugin_selections = if input.contains_key("pluginAgentModelSelectionOverrides") {
        parse_plugin_overrides(input.get("pluginAgentModelSelectionOverrides"))
    } else {
        import_legacy_plugin_overrides(input)
    };

    let mut next = input.clone();
    let mut built_in = Map::new();
    for (name, selection) in selections {
        built_in.insert(name, serde_json::to_value(&selection).unwrap());
    }
    next.insert(
        "builtInModelSelectionOverrides".into(),
        Value::Object(built_in),
    );
    let mut plugin = Map::new();
    for (id, selection) in plugin_selections {
        plugin.insert(id, serde_json::to_value(&selection).unwrap());
    }
    next.insert(
        "pluginAgentModelSelectionOverrides".into(),
        Value::Object(plugin),
    );
    next
}

fn import_legacy_plugin_overrides(input: &Map<String, Value>) -> BTreeMap<String, Selection> {
    let mut overrides = BTreeMap::new();
    let Some(models) = input
        .get("pluginAgentModelOverrides")
        .and_then(Value::as_object)
    else {
        return overrides;
    };
    let thought_levels = input
        .get("pluginAgentThoughtLevelOverrides")
        .and_then(Value::as_object);
    for (id, model) in models {
        if !id.starts_with("plugin:") {
            continue;
        }
        let thought_level = thought_levels.and_then(|object| object.get(id));
        let Some(selection) = parse_markdown_selection(Some(model), thought_level) else {
            continue;
        };
        let original_provider = selection.provider_id.clone();
        let provider_id = if original_provider.starts_with("builtin:") {
            migrate_legacy_model_provider_id(&original_provider)
        } else {
            Some(original_provider.clone())
        };
        let migrated = match provider_id {
            Some(provider_id) => Selection {
                provider_id,
                model_id: migrate_official_glm_model_id(&original_provider, &selection.model_id),
                options: selection.options,
            },
            None => selection,
        };
        overrides.insert(id.clone(), migrated);
    }
    overrides
}

/// `migrateSubagentStateFile`：损坏文件保持原样，不覆盖原文。
pub fn best_effort_migrate_state_file(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !metadata.is_file() {
        return;
    }
    let Ok(original) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&original) else {
        return;
    };
    let Some(object) = parsed.as_object() else {
        return;
    };
    let next = import_subagent_state_selections(object);
    if next == *object {
        return;
    }
    // 外部编辑器不持有应用锁：写前比较原文，不能覆盖并发修改。
    if std::fs::read_to_string(path).unwrap_or_default() != original {
        return;
    }
    let _ = write_json_atomic(path, &Value::Object(next), &WriteOptions::default());
}

/// `migrateUserSubagentMarkdown`：只处理用户 agents 根下的 Markdown，返回失败路径。
pub fn migrate_user_subagent_markdown(user_root: &Path) -> Vec<PathBuf> {
    let mut failures = Vec::new();
    visit_user_agents(user_root, &mut failures);
    failures
}

fn visit_user_agents(directory: &Path, failures: &mut Vec<PathBuf>) {
    let metadata = match std::fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            failures.push(directory.to_path_buf());
            return;
        }
    };
    if !metadata.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => {
            failures.push(directory.to_path_buf());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                failures.push(path);
                continue;
            }
        };
        if file_type.is_dir() {
            visit_user_agents(&path, failures);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if !(name.ends_with(".md") || name.ends_with(".markdown")) {
            continue;
        }
        if migrate_markdown_file(&path).is_err() {
            failures.push(path);
        }
    }
}

fn migrate_markdown_file(path: &Path) -> Result<(), ()> {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if !metadata.is_file() {
        return Ok(());
    }
    let Ok(original) = std::fs::read_to_string(path) else {
        return Err(());
    };
    let next = migrate_subagent_markdown_provider(&original);
    if next == original {
        return Ok(());
    }
    if std::fs::read_to_string(path).unwrap_or_default() != original {
        return Err(());
    }
    atomic_file::write_text_atomic(path, &next, &WriteOptions::default()).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_legacy_double_maps() {
        let input = Map::from_iter([
            (
                "builtInModelOverrides".to_string(),
                serde_json::json!({ "Explore": "builtin:zai/glm-4.7" }),
            ),
            (
                "builtInThoughtLevelOverrides".to_string(),
                serde_json::json!({ "Explore": "high" }),
            ),
            (
                "pluginAgentModelOverrides".to_string(),
                serde_json::json!({ "plugin:p@m:x": "builtin:zai/glm-4.7" }),
            ),
        ]);
        let next = import_subagent_state_selections(&input);
        let built_in = next["builtInModelSelectionOverrides"].as_object().unwrap();
        assert_eq!(built_in["Explore"]["providerId"], "zai-api");
        assert_eq!(built_in["Explore"]["modelId"], "GLM-4.7");
        assert_eq!(built_in["Explore"]["options"]["reasoningLevel"], "high");
        let plugin = next["pluginAgentModelSelectionOverrides"]
            .as_object()
            .unwrap();
        assert_eq!(plugin["plugin:p@m:x"]["providerId"], "zai-api");
    }

    #[test]
    fn current_map_is_authoritative_and_not_remigrated() {
        let input = Map::from_iter([(
            "builtInModelSelectionOverrides".to_string(),
            serde_json::json!({
                "Explore": { "providerId": "builtin:zai", "modelId": "old-model" }
            }),
        )]);
        let next = import_subagent_state_selections(&input);
        let built_in = next["builtInModelSelectionOverrides"].as_object().unwrap();
        assert_eq!(built_in["Explore"]["providerId"], "builtin:zai");
        assert_eq!(built_in["Explore"]["modelId"], "old-model");
    }

    #[test]
    fn disabled_ids_keep_original_text_and_drop_blanks() {
        let input = Map::from_iter([
            (
                "disabledAgentIds".to_string(),
                serde_json::json!([" a ", "", "b"]),
            ),
            (
                "builtInModelSelectionOverrides".to_string(),
                serde_json::json!({}),
            ),
            (
                "pluginAgentModelSelectionOverrides".to_string(),
                serde_json::json!({}),
            ),
        ]);
        let value = Value::Object(input);
        let parsed = serde_json::from_str::<Value>(&value.to_string()).unwrap();
        let object = parsed.as_object().unwrap();
        let disabled = object["disabledAgentIds"].as_array().unwrap();
        assert_eq!(disabled.len(), 3);
    }
}
