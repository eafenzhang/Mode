//! 频道投影类型（`modeAutomationProtocolSchema` / `modeAutomationRun`）。

use automation_repo::{Automation, AutomationRun};
use automation_schedule::ScheduleRule;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeAutomation {
    pub automation_id: String,
    pub title: String,
    pub cron_expr: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<model_selection::ModelSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_task_id: Option<String>,
    pub enabled: bool,
    pub lifecycle_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<i64>,
    pub run_count: i64,
    pub recurring: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_rule: Option<ScheduleRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeAutomationRun {
    pub run_id: String,
    pub automation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<i64>,
    pub trigger: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<model_selection::ModelSelection>,
    pub dispatch_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&Automation> for ModeAutomation {
    fn from(source: &Automation) -> Self {
        Self {
            automation_id: source.automation_id.clone(),
            title: source.title.clone(),
            cron_expr: source.cron_expr.clone(),
            prompt: source.prompt.clone(),
            model_selection: source.model_selection.clone(),
            mode: source.mode.clone(),
            target_task_id: source.target_task_id.clone(),
            enabled: source.enabled,
            lifecycle_status: source.lifecycle_status.clone(),
            next_run_at: source.next_run_at,
            last_run_at: source.last_run_at,
            run_count: source.run_count,
            recurring: source.recurring,
            max_runs: source.max_runs,
            schedule_rule: parse_schedule_rule(source.schedule_rule.as_ref()),
        }
    }
}

impl From<&AutomationRun> for ModeAutomationRun {
    fn from(source: &AutomationRun) -> Self {
        Self {
            run_id: source.run_id.clone(),
            automation_id: source.automation_id.clone(),
            scheduled_at: source.scheduled_at,
            trigger: source.trigger.clone(),
            model_selection: source.model_selection.clone(),
            dispatch_status: source.dispatch_status.clone(),
            outcome: source.outcome.clone(),
            session_id: source.session_id.clone(),
            error: source.error.clone(),
            created_at: source.created_at,
            updated_at: source.updated_at,
        }
    }
}

pub(crate) fn parse_schedule_rule(value: Option<&serde_json::Value>) -> Option<ScheduleRule> {
    serde_json::from_value(value?.clone()).ok()
}

/// 立即运行结果：`{automation, run}`。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeAutomationRunNow {
    pub automation: ModeAutomation,
    pub run: ModeAutomationRun,
}
