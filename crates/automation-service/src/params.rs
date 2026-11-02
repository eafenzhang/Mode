//! create/update 入参（`ModeAutomationCreateParams` / `ModeAutomationUpdateParams`）。
//!
//! modelSelection 与 botDeliveryTarget 在 `automation-repo` 里只实现 Serialize；
//! 这里用本地 Deserialize 形状接收，再转换成 repo 类型。

use automation_schedule::{IntervalUnit, ScheduleRule};
use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelectionInput {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default)]
    pub options: Option<SelectionOptionsInput>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionOptionsInput {
    #[serde(default)]
    pub reasoning_level: Option<String>,
}

impl ModelSelectionInput {
    pub fn to_repo(&self) -> model_selection::ModelSelection {
        model_selection::ModelSelection {
            provider_id: self.provider_id.clone(),
            model_id: self.model_id.clone(),
            options: self
                .options
                .as_ref()
                .map(|options| model_selection::SelectionOptions {
                    reasoning_level: options.reasoning_level.clone(),
                }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotDeliveryTargetInput {
    pub provider: String,
    pub bot_id: String,
    pub provider_user_id: String,
    pub chat_type: String,
}

impl BotDeliveryTargetInput {
    pub fn to_repo(&self) -> automation_repo::AutomationBotDeliveryTarget {
        automation_repo::AutomationBotDeliveryTarget {
            provider: self.provider.clone(),
            bot_id: self.bot_id.clone(),
            provider_user_id: self.provider_user_id.clone(),
            chat_type: self.chat_type.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationCreateParams {
    pub title: String,
    pub cron_expr: String,
    #[serde(default)]
    pub relative_delay_minutes: Option<f64>,
    pub prompt: String,
    #[serde(default)]
    pub model_selection: Option<ModelSelectionInput>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub target_task_id: Option<String>,
    #[serde(default)]
    pub bot_delivery_target: Option<BotDeliveryTargetInput>,
    #[serde(default)]
    pub recurring: Option<bool>,
    #[serde(default)]
    pub max_runs: Option<i64>,
    #[serde(default)]
    pub end_at: Option<i64>,
    #[serde(default)]
    pub interval_unit: Option<IntervalUnit>,
    #[serde(default)]
    pub interval: Option<f64>,
    #[serde(default)]
    pub schedule_rule: Option<ScheduleRule>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationUpdateParams {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub cron_expr: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub model_selection: Option<Option<ModelSelectionInput>>,
    #[serde(default, deserialize_with = "double_option")]
    pub mode: Option<Option<String>>,
    #[serde(default)]
    pub recurring: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    pub max_runs: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub end_at: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub schedule_rule: Option<Option<ScheduleRule>>,
    #[serde(default)]
    pub interval_unit: Option<IntervalUnit>,
    #[serde(default)]
    pub interval: Option<f64>,
    #[serde(default)]
    pub schedule_edited_by_user: Option<bool>,
}

/// 区分「键缺失」与「显式 null」。
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}
