//! `session/debug` 的严格线格式镜像。
//!
//! 真源：`packages/shared/src/session-debug.ts` 的 `sessionDebugParamsSchema` /
//! `sessionDebugSnapshotSchema`。规范见 `docs/specs/session-read-group.md`。
//!
//! `count` 是 `z.number().finite().nonnegative()`——允许小数，所以数值字段一律用
//! `serde_json::Number` 保留原样；`nullable()` 的键必在（`Option<T>` 不做 skip），
//! `nullable().optional()` 用三态解码。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::session_read::deserialize_nullable;

pub const SESSION_DEBUG_METHOD: &str = "session/debug";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDebugParams {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_write_input_tokens: Option<Number>,
}

/// `sessionDebugRoundSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugRound {
    pub event_key: String,
    pub request_id: String,
    pub request_index: Number,
    pub recorded_at: Number,
    pub usage: DebugUsage,
    /// 键必在，值可 null。
    pub hit_rate: Option<Number>,
    pub generation_duration_ms: Option<Number>,
    pub tokens_per_second: Option<Number>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub prefill_duration_ms: Option<Option<Number>>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub prefill_tokens_per_second: Option<Option<Number>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkStatusType {
    #[serde(rename = "model_request_started")]
    Started,
    #[serde(rename = "model_request_completed")]
    Completed,
    #[serde(rename = "model_request_failed")]
    Failed,
    #[serde(rename = "model_retry_scheduled")]
    RetryScheduled,
    #[serde(rename = "model_stream_stalled")]
    StreamStalled,
}

/// `sessionDebugNetworkEntrySchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugNetworkEntry {
    pub event_key: String,
    pub trace_id: String,
    pub recorded_at: Number,
    pub status_type: NetworkStatusType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(default, rename = "baseURL", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_attempt: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_ms: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub request_headers: BTreeMap<String, String>,
    pub response_headers: BTreeMap<String, String>,
    pub request_header_count: Number,
    pub response_header_count: Number,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugCacheStats {
    pub hit_rate_request_count: Number,
    pub total_input_tokens: Number,
    pub total_cache_read_tokens: Number,
    pub hit_rate: Option<Number>,
}

/// `sessionDebugSnapshotSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionDebugSnapshot {
    pub session_id: String,
    pub rounds: Vec<DebugRound>,
    pub network_entries: Vec<DebugNetworkEntry>,
    /// 键必在，值可 null。
    pub cache: Option<DebugCacheStats>,
}
