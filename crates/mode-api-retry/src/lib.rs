//! 网络重试状态的归一化与事件投影。
//!
//! 真源：`packages/shared/src/mode-api-retry-status.ts`。
//! 规范见 `docs/specs/mode-api-retry.md`。

mod session;

pub use session::{DeferredDraftRegistry, SessionApiRetryTracker, SessionTarget};

use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiRetryStatus {
    pub kind: String,
    pub attempt: i64,
    pub max_retries: i64,
    pub retry_delay_ms: i64,
    pub error_status: Option<i64>,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiRetryOutcome {
    /// 输入是 `undefined` 或空对象，保持「未知/不修改」。
    Undefined,
    /// 输入是 `null` 或明确的失败终止事件。
    Null,
    Status(ApiRetryStatus),
}

fn record(value: Option<&Value>) -> &Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    match value {
        Some(Value::Object(map)) => map,
        _ => EMPTY.get_or_init(Map::new),
    }
}

fn string_value(value: Option<&Value>) -> Option<&str> {
    value?.as_str()
}

fn integer_value(value: Option<&Value>, positive: bool) -> Option<i64> {
    let number = value?.as_f64()?;
    if !number.is_finite() || number.fract() != 0.0 {
        return None;
    }
    let value = number as i64;
    if positive && value <= 0 {
        return None;
    }
    if !positive && value < 0 {
        return None;
    }
    Some(value)
}

/// `normalizeModeApiRetryStatus`。
pub fn normalize_mode_api_retry_status(value: Option<&Value>) -> ApiRetryOutcome {
    match value {
        None | Some(Value::Null) => {
            if value.is_none() {
                ApiRetryOutcome::Undefined
            } else {
                ApiRetryOutcome::Null
            }
        }
        Some(Value::Object(map)) if map.is_empty() => ApiRetryOutcome::Undefined,
        Some(Value::Object(map)) => {
            let attempt = integer_value(map.get("attempt"), true).unwrap_or_else(|| {
                ((integer_value(map.get("nextAttempt"), true).unwrap_or(2)) - 1).max(1)
            });
            let max_retries = (integer_value(map.get("maxRetries"), false).unwrap_or_else(|| {
                (integer_value(map.get("maxAttempts"), true).unwrap_or(attempt + 1)) - 1
            }))
            .max(attempt);
            let retry_delay_ms = integer_value(map.get("retryDelayMs"), false)
                .or_else(|| integer_value(map.get("delayMs"), false))
                .unwrap_or(0);
            let error_status = integer_value(map.get("errorStatus"), false)
                .or_else(|| integer_value(map.get("statusCode"), false));
            let error = string_value(map.get("error"))
                .or_else(|| string_value(map.get("message")))
                .or_else(|| string_value(map.get("reason")))
                .unwrap_or("Model retry scheduled")
                .to_string();
            ApiRetryOutcome::Status(ApiRetryStatus {
                kind: "api_retry".to_string(),
                attempt,
                max_retries,
                retry_delay_ms,
                error_status,
                error,
            })
        }
        // 数组/标量在 TS 的 asRecord 下等价空对象 → undefined。
        Some(_) => ApiRetryOutcome::Undefined,
    }
}

/// `modeApiRetryFromStreamRecoveryPayload`。
pub fn mode_api_retry_from_stream_recovery_payload(value: Option<&Value>) -> ApiRetryOutcome {
    let map = record(value);
    let Some(attempt) = integer_value(map.get("retryNumber"), true) else {
        return ApiRetryOutcome::Undefined;
    };
    let max_retries = integer_value(map.get("maxRetries"), false)
        .unwrap_or(attempt)
        .max(attempt);
    ApiRetryOutcome::Status(ApiRetryStatus {
        kind: "api_retry".to_string(),
        attempt,
        max_retries,
        retry_delay_ms: 0,
        error_status: None,
        error: string_value(map.get("message"))
            .unwrap_or("Model stream recovery retry started")
            .to_string(),
    })
}

/// `modeApiRetryFromModelNetworkStatusPayload`。
pub fn mode_api_retry_from_model_network_status_payload(
    payload: &Map<String, Value>,
) -> ApiRetryOutcome {
    match string_value(payload.get("type")) {
        Some("model_retry_scheduled") => {
            normalize_mode_api_retry_status(Some(&Value::Object(payload.clone())))
        }
        Some("model_request_started") => {
            let stream_retry =
                mode_api_retry_from_stream_recovery_payload(payload.get("streamRecovery"));
            if stream_retry != ApiRetryOutcome::Undefined {
                return stream_retry;
            }
            if integer_value(payload.get("attempt"), true).unwrap_or(1) <= 1 {
                return ApiRetryOutcome::Null;
            }
            ApiRetryOutcome::Undefined
        }
        Some("model_request_completed") => ApiRetryOutcome::Null,
        Some("model_request_failed") if payload.get("retryable") != Some(&Value::Bool(true)) => {
            ApiRetryOutcome::Null
        }
        _ => ApiRetryOutcome::Undefined,
    }
}

/// `isModeModelRetryRecoveryProgressPayload`。
pub fn is_mode_model_retry_recovery_progress_payload(payload: &Map<String, Value>) -> bool {
    let kind = string_value(payload.get("kind"));
    if matches!(kind, Some("text_delta" | "reasoning_delta")) {
        return string_value(payload.get("delta")).is_some_and(|delta| !delta.is_empty());
    }
    if string_value(payload.get("toolCallId")).is_none_or(|tool_call_id| tool_call_id.is_empty()) {
        return false;
    }
    if matches!(
        kind,
        Some("tool_input_start" | "tool_input_end" | "tool_call")
    ) {
        return true;
    }
    if kind == Some("tool_input_delta") {
        return string_value(payload.get("delta")).is_some_and(|delta| !delta.is_empty());
    }
    false
}
