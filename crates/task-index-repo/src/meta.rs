//! `ModeTaskMeta` 的 Rust 投影与 `modeTaskMetaSchema` 的等价解析。
//!
//! 行为真源：`packages/shared/src/validation.ts`（`modeTaskMetaSchema`）与
//! `packages/shared/src/mode-task-types-core.ts`（`ModeTaskMeta`）。
//!
//! Zod 的 `z.object` 默认丢弃未知字段、`z.string().trim().min(1)` 会改动值，
//! `.optional()` 不接受 `null`，嵌套 `summaryTitle` 缺省补 `null`。这些语义都在
//! `parse_mode_task_meta` 里显式复刻，避免“差不多”的近似。

use serde::Serialize;
use serde_json::{Map, Value};

use crate::js::{js_trim, utf16_len};

pub const MODE_AGENT_PROVIDER: &str = "glm";
pub const CRON_DEFAULT_GROUP_ID: &str = "mode-default-group-cron";
pub const OFF_PEAK_DEFAULT_GROUP_ID: &str = "mode-default-group-off-peak";

const MODE_VALUES: &[&str] = &["yolo", "plan", "edit", "auto", "autoEdit", "build"];
const STATUS_VALUES: &[&str] = &["running", "completed", "error"];
const MIGRATION_SOURCE_VALUES: &[&str] = &["claudeCode"];
const WORKSPACE_PURPOSE_VALUES: &[&str] = &["project", "conversation"];
const GOAL_STATUS_VALUES: &[&str] = &["active", "paused", "budget_limited", "complete"];
const ATTRIBUTION_KEYS: &[&str] = &[
    "source",
    "reason",
    "errorPhase",
    "exceptionKind",
    "providerId",
    "modelId",
    "providerKind",
    "transport",
    "statusCode",
    "providerErrorCode",
    "retryable",
];

type Parse<T> = std::result::Result<T, ()>;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeTaskMeta {
    pub task_id: String,
    pub trace_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_overridden: Option<bool>,
    pub workspace_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_purpose: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_epoch: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron_automation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub off_peak_task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<ModeTaskLastError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_summary: Option<ModeTaskChangeSummary>,
    /// 三态：`None` = 字段缺失；`Some(None)` = JSON `null`；`Some(Some(_))` = goal。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Option<ModeTaskGoal>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeTaskGoal {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "targetID")]
    pub target_id: String,
    pub objective: String,
    /// 恒输出；`None` 序列化为 `null`（schema 缺省值）。
    pub summary_title: Option<String>,
    pub status: String,
    /// 恒输出；`None` 序列化为 `null`。
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_input_id: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_run_started_at_ms: Option<Option<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_run_last_seen_at_ms: Option<Option<i64>>,
    pub time: ModeTaskGoalTime,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModeTaskGoalTime {
    pub created: i64,
    pub updated: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeTaskLastError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribution: Option<ErrorAttribution>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorAttribution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exception_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeTaskChangeSummary {
    pub file_count: i64,
    pub added: i64,
    pub removed: i64,
    pub files: Vec<ModeTaskChangedFile>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeTaskChangedFile {
    pub path: String,
    pub added: i64,
    pub removed: i64,
    pub write_count: i64,
    pub last_turn_index: i64,
}

pub(crate) fn is_terminal_status(status: Option<&str>) -> bool {
    matches!(status, Some("completed") | Some("error"))
}

fn as_object(value: &Value) -> Parse<&Map<String, Value>> {
    value.as_object().ok_or(())
}

fn int_value(value: &Value) -> Option<i64> {
    if let Some(integer) = value.as_i64() {
        return Some(integer);
    }
    // Zod 的 `z.number().int()` 接受整值浮点（JSON 里写成 1.0）。
    let float = value.as_f64()?;
    if float.fract() == 0.0 && float >= i64::MIN as f64 && float <= i64::MAX as f64 {
        Some(float as i64)
    } else {
        None
    }
}

fn required_non_empty(object: &Map<String, Value>, key: &str) -> Parse<String> {
    match object.get(key) {
        Some(Value::String(value)) => {
            let trimmed = js_trim(value);
            if trimmed.is_empty() {
                Err(())
            } else {
                Ok(trimmed.to_owned())
            }
        }
        _ => Err(()),
    }
}

fn optional_non_empty(object: &Map<String, Value>, key: &str) -> Parse<Option<String>> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => {
            let trimmed = js_trim(value);
            if trimmed.is_empty() {
                Err(())
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Some(_) => Err(()),
    }
}

fn required_string(object: &Map<String, Value>, key: &str) -> Parse<String> {
    match object.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(()),
    }
}

fn optional_string(object: &Map<String, Value>, key: &str) -> Parse<Option<String>> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(()),
    }
}

fn optional_bool(object: &Map<String, Value>, key: &str) -> Parse<Option<bool>> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(()),
    }
}

fn required_non_negative_int(object: &Map<String, Value>, key: &str) -> Parse<i64> {
    match object.get(key).and_then(int_value) {
        Some(value) if value >= 0 => Ok(value),
        _ => Err(()),
    }
}

fn optional_non_negative_int(object: &Map<String, Value>, key: &str) -> Parse<Option<i64>> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => match int_value(value) {
            Some(integer) if integer >= 0 => Ok(Some(integer)),
            _ => Err(()),
        },
    }
}

fn optional_enum(
    object: &Map<String, Value>,
    key: &str,
    allowed: &[&str],
) -> Parse<Option<String>> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(value)) if allowed.contains(&value.as_str()) => Ok(Some(value.clone())),
        Some(_) => Err(()),
    }
}

fn parse_nullable_envelope<T>(
    object: &Map<String, Value>,
    key: &str,
    parse_value: impl FnOnce(&Value) -> Parse<T>,
) -> Parse<Option<Option<T>>> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(value) => Ok(Some(Some(parse_value(value)?))),
    }
}

fn parse_error_attribution(value: &Value) -> Parse<ErrorAttribution> {
    let object = as_object(value)?;
    // `errorAttributionSchema` 是 `.strict()`：未知字段直接失败。
    for key in object.keys() {
        if !ATTRIBUTION_KEYS.contains(&key.as_str()) {
            return Err(());
        }
    }
    let source = optional_enum(object, "source", &["provider", "runtime", "tool", "network"])?;
    let reason = match object.get("reason") {
        None => None,
        Some(Value::String(value)) if (1..=160).contains(&utf16_len(value)) => {
            Some(value.clone())
        }
        Some(_) => return Err(()),
    };
    let error_phase = optional_enum(
        object,
        "errorPhase",
        &[
            "prepare",
            "configuration",
            "connect",
            "response",
            "stream",
            "parse",
            "validation",
            "unhandled",
        ],
    )?;
    let exception_kind = optional_enum(
        object,
        "exceptionKind",
        &[
            "api_call",
            "generic",
            "protocol",
            "provider_business",
            "transport",
            "type_error",
            "validation",
        ],
    )?;
    let bounded = |key: &str| -> Parse<Option<String>> {
        match object.get(key) {
            None => Ok(None),
            Some(Value::String(value)) if (1..=160).contains(&utf16_len(value)) => {
                Ok(Some(value.clone()))
            }
            Some(_) => Err(()),
        }
    };
    let provider_id = bounded("providerId")?;
    let model_id = bounded("modelId")?;
    let provider_kind = bounded("providerKind")?;
    let transport = optional_enum(object, "transport", &["http", "sse", "websocket"])?;
    let status_code = match object.get("statusCode") {
        None => None,
        Some(value) => match int_value(value) {
            Some(code) if (100..=599).contains(&code) => Some(code),
            _ => return Err(()),
        },
    };
    let provider_error_code = bounded("providerErrorCode")?;
    let retryable = optional_bool(object, "retryable")?;
    Ok(ErrorAttribution {
        source,
        reason,
        error_phase,
        exception_kind,
        provider_id,
        model_id,
        provider_kind,
        transport,
        status_code,
        provider_error_code,
        retryable,
    })
}

fn parse_last_error(value: &Value) -> Parse<ModeTaskLastError> {
    let object = as_object(value)?;
    let code = optional_string(object, "code")?;
    let detail = optional_string(object, "detail")?;
    let message = match object.get("message") {
        Some(Value::String(value)) if !value.is_empty() => value.clone(),
        _ => return Err(()),
    };
    let trace_id = optional_non_empty(object, "traceId")?;
    let task_id = optional_non_empty(object, "taskId")?;
    let attribution = match object.get("attribution") {
        None => None,
        Some(value) => Some(parse_error_attribution(value)?),
    };
    Ok(ModeTaskLastError {
        code,
        detail,
        message,
        trace_id,
        task_id,
        attribution,
    })
}

fn parse_change_summary(value: &Value) -> Parse<ModeTaskChangeSummary> {
    let object = as_object(value)?;
    let file_count = required_non_negative_int(object, "fileCount")?;
    let added = required_non_negative_int(object, "added")?;
    let removed = required_non_negative_int(object, "removed")?;
    let files_value = object.get("files").ok_or(())?;
    let files_array = files_value.as_array().ok_or(())?;
    let mut files = Vec::with_capacity(files_array.len());
    for file in files_array {
        let file_object = as_object(file)?;
        let path = required_string(file_object, "path")?;
        let added = required_non_negative_int(file_object, "added")?;
        let removed = required_non_negative_int(file_object, "removed")?;
        let write_count = match file_object.get("writeCount").and_then(int_value) {
            Some(value) if value > 0 => value,
            _ => return Err(()),
        };
        let last_turn_index = required_non_negative_int(file_object, "lastTurnIndex")?;
        files.push(ModeTaskChangedFile {
            path,
            added,
            removed,
            write_count,
            last_turn_index,
        });
    }
    Ok(ModeTaskChangeSummary {
        file_count,
        added,
        removed,
        files,
    })
}

fn parse_goal(value: &Value) -> Parse<ModeTaskGoal> {
    let object = as_object(value)?;
    let session_id = required_non_empty(object, "sessionID")?;
    let target_id = required_non_empty(object, "targetID")?;
    let objective = required_non_empty(object, "objective")?;
    let summary_title = match object.get("summaryTitle") {
        // schema 的 `.default(null)`：缺失补 null。
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(_) => return Err(()),
    };
    let status = match object.get("status") {
        Some(Value::String(value)) if GOAL_STATUS_VALUES.contains(&value.as_str()) => {
            value.clone()
        }
        _ => return Err(()),
    };
    let token_budget = match object.get("tokenBudget") {
        Some(Value::Null) => None,
        Some(value) => match int_value(value) {
            Some(budget) if budget > 0 => Some(budget),
            _ => return Err(()),
        },
        None => return Err(()),
    };
    let tokens_used = required_non_negative_int(object, "tokensUsed")?;
    let time_used_seconds = required_non_negative_int(object, "timeUsedSeconds")?;
    let active_input_id = nullable_non_empty(object, "activeInputId")?;
    let active_run_started_at_ms = nullable_non_negative_int(object, "activeRunStartedAtMs")?;
    let active_run_last_seen_at_ms = nullable_non_negative_int(object, "activeRunLastSeenAtMs")?;
    let time_object = as_object(object.get("time").ok_or(())?)?;
    let time = ModeTaskGoalTime {
        created: required_non_negative_int(time_object, "created")?,
        updated: required_non_negative_int(time_object, "updated")?,
    };
    Ok(ModeTaskGoal {
        session_id,
        target_id,
        objective,
        summary_title,
        status,
        token_budget,
        tokens_used,
        time_used_seconds,
        active_input_id,
        active_run_started_at_ms,
        active_run_last_seen_at_ms,
        time,
    })
}

fn nullable_non_empty(object: &Map<String, Value>, key: &str) -> Parse<Option<Option<String>>> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(value)) => {
            let trimmed = js_trim(value);
            if trimmed.is_empty() {
                Err(())
            } else {
                Ok(Some(Some(trimmed.to_owned())))
            }
        }
        Some(_) => Err(()),
    }
}

fn nullable_non_negative_int(object: &Map<String, Value>, key: &str) -> Parse<Option<Option<i64>>> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(value) => match int_value(value) {
            Some(integer) if integer >= 0 => Ok(Some(Some(integer))),
            _ => Err(()),
        },
    }
}

/// 解析单个 `lastError` 值（`modeTaskMetaSchema` 的嵌套 schema）。
pub fn parse_last_error_value(value: &Value) -> Option<ModeTaskLastError> {
    parse_last_error(value).ok()
}

/// 解析单个 `target` goal 值（`modeTaskGoalSchema`）。
pub fn parse_task_goal_value(value: &Value) -> Option<ModeTaskGoal> {
    parse_goal(value).ok()
}

/// `modeTaskMetaSchema.safeParse(JSON.parse(meta_json))` 的等价实现。
///
/// 返回 `None` 表示解析失败（调用方回落到列投影）。
pub fn parse_mode_task_meta(value: &Value) -> Option<ModeTaskMeta> {
    parse_mode_task_meta_inner(value).ok()
}

fn parse_mode_task_meta_inner(value: &Value) -> Parse<ModeTaskMeta> {
    let object = as_object(value)?;
    let task_id = required_non_empty(object, "taskId")?;
    let trace_id = required_non_empty(object, "traceId")?;
    let title = required_string(object, "title")?;
    let title_overridden = optional_bool(object, "titleOverridden")?;
    let workspace_path = required_non_empty(object, "workspacePath")?;
    let workspace_identity = optional_non_empty(object, "workspaceIdentity")?;
    let workspace_purpose =
        optional_enum(object, "workspacePurpose", WORKSPACE_PURPOSE_VALUES)?;
    let created_at = required_non_negative_int(object, "createdAt")?;
    let updated_at = required_non_negative_int(object, "updatedAt")?;
    let mode = match object.get("mode") {
        Some(Value::String(value)) if MODE_VALUES.contains(&value.as_str()) => value.clone(),
        _ => return Err(()),
    };
    let model = optional_string(object, "model")?;
    let thought_level = optional_non_empty(object, "thoughtLevel")?;
    let runtime_epoch = optional_non_negative_int(object, "runtimeEpoch")?;
    let provider = match object.get("provider") {
        None => None,
        Some(Value::String(value)) if value == MODE_AGENT_PROVIDER => Some(value.clone()),
        Some(_) => return Err(()),
    };
    let migration_source = optional_enum(object, "migrationSource", MIGRATION_SOURCE_VALUES)?;
    let forked_from_task_id = optional_non_empty(object, "forkedFromTaskId")?;
    let cron_automation_id = optional_non_empty(object, "cronAutomationId")?;
    let off_peak_task_id = optional_non_empty(object, "offPeakTaskId")?;
    let unread_at = optional_non_negative_int(object, "unreadAt")?;
    let status = optional_enum(object, "status", STATUS_VALUES)?;
    let last_error = match object.get("lastError") {
        None => None,
        Some(value) => Some(parse_last_error(value)?),
    };
    let change_summary = match object.get("changeSummary") {
        None => None,
        Some(value) => Some(parse_change_summary(value)?),
    };
    let target = parse_nullable_envelope(object, "target", parse_goal)?;
    Ok(ModeTaskMeta {
        task_id,
        trace_id,
        title,
        title_overridden,
        workspace_path,
        workspace_identity,
        workspace_purpose,
        created_at,
        updated_at,
        mode,
        model,
        thought_level,
        runtime_epoch,
        provider,
        migration_source,
        forked_from_task_id,
        cron_automation_id,
        off_peak_task_id,
        unread_at,
        status,
        last_error,
        change_summary,
        target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_meta() -> Value {
        json!({
            "taskId": "t1",
            "traceId": "trace",
            "title": "hello",
            "workspacePath": "/w",
            "createdAt": 1,
            "updatedAt": 2,
            "mode": "build"
        })
    }

    #[test]
    fn parses_and_trims_non_empty_strings() {
        let parsed = parse_mode_task_meta(&json!({
            "taskId": " t1 ",
            "traceId": "  trace  ",
            "title": "hello",
            "workspacePath": " /w ",
            "createdAt": 1,
            "updatedAt": 2,
            "mode": "auto"
        }))
        .expect("合法 meta");
        assert_eq!(parsed.task_id, "t1");
        assert_eq!(parsed.trace_id, "trace");
        assert_eq!(parsed.workspace_path, "/w");
    }

    #[test]
    fn rejects_null_for_optional_fields() {
        let mut value = base_meta();
        value["model"] = Value::Null;
        assert!(parse_mode_task_meta(&value).is_none());
    }

    #[test]
    fn strips_unknown_fields() {
        let mut value = base_meta();
        value["unknown"] = json!("x");
        let parsed = parse_mode_task_meta(&value).expect("未知字段应被丢弃");
        let serialized = serde_json::to_value(parsed).expect("可序列化");
        assert!(serialized.get("unknown").is_none());
    }

    #[test]
    fn target_defaults_summary_title_and_preserves_null() {
        let mut value = base_meta();
        value["target"] = json!({
            "sessionID": "s",
            "targetID": "g",
            "objective": "o",
            "status": "active",
            "tokenBudget": null,
            "tokensUsed": 0,
            "timeUsedSeconds": 0,
            "time": { "created": 1, "updated": 1 }
        });
        let parsed = parse_mode_task_meta(&value).expect("合法 target");
        let goal = parsed.target.clone().unwrap().unwrap();
        assert_eq!(goal.summary_title, None);
        let serialized = serde_json::to_value(parsed).expect("可序列化");
        assert_eq!(serialized["target"]["summaryTitle"], Value::Null);

        value["target"] = Value::Null;
        let parsed = parse_mode_task_meta(&value).expect("target 允许 null");
        assert_eq!(parsed.target, Some(None));
    }

    #[test]
    fn attribution_is_strict() {
        let mut value = base_meta();
        value["lastError"] = json!({ "message": "boom", "attribution": { "extra": 1 } });
        assert!(parse_mode_task_meta(&value).is_none());
    }
}
