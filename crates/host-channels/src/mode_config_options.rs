//! `IModeTaskService` 旧 configOptions 投影与 CLI 日志路径（真源
//! `packages/shared/src/mode-agent-model-state.ts` 的
//! `modeSessionSettingsToModeConfigOptions`、`formatModelPickerValue` 与
//! `modeTaskServiceAdapter.ts` 的 `resolveModeAgentCurrentLogFilePath`）。
//!
//! 投影供 `getTaskConfigOptions` / `setConfigOption` 使用（bots/automation 消费）。
//! 只做形状转换，不参与运行时决策。

use agent_protocol::message::MessageModel;
use agent_protocol::session_read::SessionMode;
use agent_protocol::snapshot::SessionSettings;
use serde_json::{json, Value};

const MODEL_CONFIG_ID: &str = "model";
const MODEL_CONFIG_CATEGORY: &str = "model";
const MODE_CONFIG_ID: &str = "mode";
const MODE_CONFIG_CATEGORY: &str = "mode";
const THOUGHT_LEVEL_CONFIG_ID: &str = "thought_level";
const THOUGHT_LEVEL_CONFIG_CATEGORY: &str = "thought_level";
const MODE_MODEL_REASONING_SEPARATOR: &str = "$";

const MODE_AGENT_MODE_OPTIONS: [(&str, &str, &str); 4] = [
    ("build", "Ask before changes", "Ask before each file changes."),
    (
        "edit",
        "Edit automatically",
        "Edit selected files or relevant workspace files automatically.",
    ),
    (
        "plan",
        "Plan mode",
        "Inspect the code and present a plan before editing.",
    ),
    (
        "yolo",
        "Full access",
        "Edit and run commands with fewer confirmations.",
    ),
];

/// TS `formatModelPickerValue`：`provider/model[$reasoningLevel]`；未绑定为空串。
pub fn format_model_picker_value(selection: Option<&MessageModel>) -> String {
    let Some(selection) = selection else {
        return String::new();
    };
    let base = format!("{}/{}", selection.provider_id, selection.model_id);
    match selection
        .options
        .as_ref()
        .and_then(|options| options.reasoning_level.as_deref())
    {
        Some(level) if !level.is_empty() => {
            format!("{base}{MODE_MODEL_REASONING_SEPARATOR}{level}")
        }
        _ => base,
    }
}

/// TS `normalizeAvailableModeMode`：未知/auto 一律回填 build。
pub fn normalize_available_mode(mode: SessionMode) -> &'static str {
    match mode {
        SessionMode::Build => "build",
        SessionMode::Edit => "edit",
        SessionMode::Plan => "plan",
        SessionMode::Yolo => "yolo",
        SessionMode::Auto => "build",
    }
}

/// TS `modeSessionSettingsToModeConfigOptions`。
pub fn settings_to_config_options(settings: &SessionSettings) -> Vec<Value> {
    let model_option = json!({
        "id": MODEL_CONFIG_ID,
        "name": "Model",
        "category": MODEL_CONFIG_CATEGORY,
        "type": "select",
        "currentValue": format_model_picker_value(settings.model.current.as_ref()),
        "options": settings
            .model
            .available
            .iter()
            .map(|model| {
                let thought_levels: Vec<Value> = model
                    .reasoning
                    .as_ref()
                    .map(|reasoning| {
                        reasoning
                            .levels
                            .iter()
                            .map(|level| json!(level.value))
                            .collect()
                    })
                    .unwrap_or_default();
                let default_thought_level = model
                    .reasoning
                    .as_ref()
                    .and_then(|reasoning| reasoning.default_level.as_ref())
                    .filter(|default| {
                        model
                            .reasoning
                            .as_ref()
                            .is_some_and(|reasoning| {
                                reasoning.levels.iter().any(|level| &level.value == *default)
                            })
                    });
                let mut option = json!({
                    "value": format_model_picker_value(Some(&model.model)),
                    "name": model.label,
                    "modelProviderId": model.model.provider_id,
                    "modelProviderName": model
                        .provider_label
                        .as_deref()
                        .unwrap_or(&model.model.provider_id),
                });
                if let Some(description) = &model.description {
                    option["description"] = json!(description);
                }
                if !thought_levels.is_empty() {
                    option["modelThoughtLevels"] = Value::Array(thought_levels);
                }
                if let Some(default) = default_thought_level {
                    option["modelDefaultThoughtLevel"] = json!(default);
                }
                option
            })
            .collect::<Vec<Value>>(),
    });
    let mode_options: Vec<Value> = MODE_AGENT_MODE_OPTIONS
        .iter()
        .map(|(value, name, description)| {
            json!({ "value": value, "name": name, "description": description })
        })
        .collect();
    let mode_option = json!({
        "id": MODE_CONFIG_ID,
        "name": "Mode",
        "category": MODE_CONFIG_CATEGORY,
        "type": "select",
        "currentValue": normalize_available_mode(settings.mode.current),
        "options": mode_options,
    });

    let mut config_options = vec![model_option, mode_option];
    if settings.thought_level.enabled {
        let available_values: Vec<&str> = settings
            .thought_level
            .available
            .iter()
            .map(|level| level.value.as_str())
            .collect();
        let default_level = settings
            .thought_level
            .default_level
            .as_deref()
            .filter(|default| available_values.contains(default));
        let current_value = settings
            .thought_level
            .current
            .as_deref()
            .or(default_level)
            .or_else(|| available_values.first().copied())
            .unwrap_or("");
        config_options.push(json!({
            "id": THOUGHT_LEVEL_CONFIG_ID,
            "name": "Thought Level",
            "category": THOUGHT_LEVEL_CONFIG_CATEGORY,
            "type": "select",
            "currentValue": current_value,
            "options": settings
                .thought_level
                .available
                .iter()
                .map(|level| {
                    let mut option = json!({ "value": level.value, "name": level.label });
                    if let Some(description) = &level.description {
                        option["description"] = json!(description);
                    }
                    option
                })
                .collect::<Vec<Value>>(),
        }));
    }
    config_options
}

/// TS `resolveModeAgentCurrentLogFilePath`：`MODE_LOG_DIR` 或 `{home}/.mode/cli/log`。
pub fn native_session_log_file(now: chrono::DateTime<chrono::Local>) -> Result<(String, bool), String> {
    let dir = std::env::var("MODE_LOG_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(std::path::PathBuf::from)
                .filter(|home| !home.as_os_str().is_empty())
                .map(|home| home.join(".mode").join("cli").join("log"))
        })
        .ok_or_else(|| "无法解析日志目录（缺 MODE_LOG_DIR/HOME）".to_string())?;
    let file_name = log_file_name(now.date_naive());
    let path = dir.join(file_name);
    let exists = path.exists();
    Ok((path.to_string_lossy().into_owned(), exists))
}

pub fn log_file_name(date: chrono::NaiveDate) -> String {
    format!("mode-{}.jsonl", date.format("%Y-%m-%d"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_protocol::message::ModelOptions;
    use agent_protocol::snapshot::{
        ModelInputFormat, ModelOption, ModelOutputFormat, ModelProperties, ModelReasoningOptions,
        SessionModelSettings, SessionModeSettings, ThoughtLevelOption, ThoughtLevelSettings,
    };

    fn model(provider: &str, model_id: &str, level: Option<&str>) -> MessageModel {
        MessageModel {
            provider_id: provider.to_string(),
            model_id: model_id.to_string(),
            options: level.map(|level| ModelOptions {
                reasoning_level: Some(level.to_string()),
            }),
        }
    }

    fn empty_properties() -> ModelProperties {
        ModelProperties {
            input_format: ModelInputFormat {
                supports_text: true,
                supports_image: false,
                supports_video: false,
                supports_audio: false,
                supports_pdf: false,
            },
            output_format: ModelOutputFormat {
                supports_text: true,
            },
        }
    }

    fn settings_with(
        current: Option<MessageModel>,
        thought: ThoughtLevelSettings,
    ) -> SessionSettings {
        SessionSettings {
            model: SessionModelSettings {
                current,
                available: vec![ModelOption {
                    model: model("glm", "glm-5", None),
                    label: "GLM-5".to_string(),
                    provider_label: Some("Z.ai".to_string()),
                    description: Some("描述".to_string()),
                    context_window: None,
                    max_output_tokens: None,
                    reasoning: Some(ModelReasoningOptions {
                        levels: vec![ThoughtLevelOption {
                            value: "high".to_string(),
                            label: "High".to_string(),
                            description: None,
                        }],
                        default_level: Some("high".to_string()),
                    }),
                    properties: empty_properties(),
                    disabled_reason: None,
                }],
                last_used: None,
            },
            thought_level: thought,
            mode: SessionModeSettings {
                current: SessionMode::Build,
            },
            permission: None,
        }
    }

    #[test]
    fn formats_model_picker_value() {
        assert_eq!(format_model_picker_value(None), "");
        assert_eq!(
            format_model_picker_value(Some(&model("glm", "glm-5", None))),
            "glm/glm-5"
        );
        assert_eq!(
            format_model_picker_value(Some(&model("glm", "glm-5", Some("high")))),
            "glm/glm-5$high"
        );
    }

    #[test]
    fn projects_model_mode_and_thought_options() {
        let thought = ThoughtLevelSettings {
            enabled: true,
            current: None,
            default_level: Some("high".to_string()),
            available: vec![ThoughtLevelOption {
                value: "high".to_string(),
                label: "High".to_string(),
                description: Some("更慢更准".to_string()),
            }],
        };
        let settings = settings_with(Some(model("glm", "glm-5", Some("high"))), thought);
        let options = settings_to_config_options(&settings);
        assert_eq!(options.len(), 3, "{options:?}");
        assert_eq!(options[0]["id"], "model");
        assert_eq!(options[0]["currentValue"], "glm/glm-5$high");
        assert_eq!(options[0]["options"][0]["value"], "glm/glm-5");
        assert_eq!(options[0]["options"][0]["modelProviderName"], "Z.ai");
        assert_eq!(options[0]["options"][0]["modelThoughtLevels"], json!(["high"]));
        assert_eq!(options[0]["options"][0]["modelDefaultThoughtLevel"], "high");
        assert_eq!(options[1]["id"], "mode");
        assert_eq!(options[1]["currentValue"], "build");
        assert_eq!(options[1]["options"].as_array().expect("modes").len(), 4);
        assert_eq!(options[2]["id"], "thought_level");
        // current 缺失 → defaultLevel → 首项。
        assert_eq!(options[2]["currentValue"], "high");
        assert_eq!(options[2]["options"][0]["description"], "更慢更准");
    }

    #[test]
    fn thought_level_option_hidden_when_disabled() {
        let thought = ThoughtLevelSettings {
            enabled: false,
            current: None,
            default_level: None,
            available: Vec::new(),
        };
        let options = settings_to_config_options(&settings_with(None, thought));
        assert_eq!(options.len(), 2);
        assert_eq!(options[0]["currentValue"], "");
    }

    #[test]
    fn log_file_name_is_mode_plus_local_date() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 22).expect("date");
        assert_eq!(log_file_name(date), "mode-2026-11-22.jsonl");
    }
}
