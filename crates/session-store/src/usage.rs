//! `model_usage` / `turn_usage` / `tool_usage` 写入。
//!
//! C1 只负责把调用方测得的事实落库；不在这里聚合、重算或补默认业务值。除主键与
//! `NOT NULL` 文本列外，其余字段用 `Default` 填 0/`None`，由调用方按需覆盖。
//! JSON 列（`raw_usage_json` / `provider_metadata_json`）按原始文本保存。

use rusqlite::params;

use crate::error::StoreError;
use crate::SessionStore;

const MODEL_USAGE_COLUMNS: &str = "id, session_id, logical_request_id, provider_id, model_id, \
query_source, started_at, status, agent, assistant_message_id, attempt_index, \
cache_creation_input_tokens, cache_read_input_tokens, cancelled_by_user, completed_at, \
computed_total_tokens, context_exceeded, duration_ms, error_code, error_message, error_type, \
finish_reason, first_token_at, input_tokens, mode, output_tokens, parent_user_message_id, \
provider_metadata_json, provider_total_tokens, raw_usage_json, reasoning_tokens, retry_count, \
retryable, span_id, task_type, time_to_first_token_ms, tool_call_count, trace_id, turn_id, variant";

const TURN_USAGE_COLUMNS: &str = "session_id, turn_id, started_at, status, \
cache_creation_input_tokens, cache_read_input_tokens, cancelled_by_user, completed_at, \
computed_total_tokens, context_exceeded, duration_ms, error_code, error_type, first_model_start_at, \
first_token_at, input_tokens, model_request_count, model_retry_count, output_tokens, \
reasoning_tokens, retryable, time_to_first_token_ms, tool_call_count, tool_error_count, trace_id, \
user_message_id";

const TOOL_USAGE_COLUMNS: &str = "id, session_id, tool_call_id, tool_name, started_at, status, \
approval_status, cancelled_by_user, completed_at, destructive, duration_ms, error_code, \
error_message, error_type, exit_code, first_output_at, output_bytes, read_only, retry_count, \
retryable, side_effect_scope, stderr_bytes, stdout_bytes, time_to_first_output_ms, trace_id, \
truncated, turn_id";

/// `model_usage` 一行。必填列不带默认值；其余列由调用方按观察值填写。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelUsageRecord {
    pub id: String,
    pub session_id: String,
    pub logical_request_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub query_source: String,
    pub started_at: i64,
    pub status: String,
    pub agent: Option<String>,
    pub assistant_message_id: Option<String>,
    pub attempt_index: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cancelled_by_user: bool,
    pub completed_at: Option<i64>,
    pub computed_total_tokens: i64,
    pub context_exceeded: bool,
    pub duration_ms: Option<i64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub error_type: Option<String>,
    pub finish_reason: Option<String>,
    pub first_token_at: Option<i64>,
    pub input_tokens: i64,
    pub mode: Option<String>,
    pub output_tokens: i64,
    pub parent_user_message_id: Option<String>,
    pub provider_metadata_json: Option<String>,
    pub provider_total_tokens: Option<i64>,
    pub raw_usage_json: Option<String>,
    pub reasoning_tokens: i64,
    pub retry_count: i64,
    pub retryable: bool,
    pub span_id: Option<String>,
    pub task_type: Option<String>,
    pub time_to_first_token_ms: Option<i64>,
    pub tool_call_count: i64,
    pub trace_id: Option<String>,
    pub turn_id: Option<String>,
    pub variant: Option<String>,
}

/// `turn_usage` 一行；主键是 `(session_id, turn_id)`。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnUsageRecord {
    pub session_id: String,
    pub turn_id: String,
    pub started_at: i64,
    pub status: String,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cancelled_by_user: bool,
    pub completed_at: Option<i64>,
    pub computed_total_tokens: i64,
    pub context_exceeded: bool,
    pub duration_ms: Option<i64>,
    pub error_code: Option<String>,
    pub error_type: Option<String>,
    pub first_model_start_at: Option<i64>,
    pub first_token_at: Option<i64>,
    pub input_tokens: i64,
    pub model_request_count: i64,
    pub model_retry_count: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub retryable: bool,
    pub time_to_first_token_ms: Option<i64>,
    pub tool_call_count: i64,
    pub tool_error_count: i64,
    pub trace_id: Option<String>,
    pub user_message_id: Option<String>,
}

/// `tool_usage` 一行；`(session_id, tool_call_id)` 有唯一索引。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolUsageRecord {
    pub id: String,
    pub session_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub started_at: i64,
    pub status: String,
    pub approval_status: Option<String>,
    pub cancelled_by_user: bool,
    pub completed_at: Option<i64>,
    pub destructive: Option<bool>,
    pub duration_ms: Option<i64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub error_type: Option<String>,
    pub exit_code: Option<i64>,
    pub first_output_at: Option<i64>,
    pub output_bytes: i64,
    pub read_only: Option<bool>,
    pub retry_count: i64,
    pub retryable: bool,
    pub side_effect_scope: Option<String>,
    pub stderr_bytes: i64,
    pub stdout_bytes: i64,
    pub time_to_first_output_ms: Option<i64>,
    pub trace_id: Option<String>,
    pub truncated: bool,
    pub turn_id: Option<String>,
}

impl SessionStore {
    /// 追加一条 model 请求用量记录。
    pub fn record_model_usage(&self, record: ModelUsageRecord) -> Result<(), StoreError> {
        self.with_write_transaction(|transaction| Self::record_model_usage_tx(transaction, record))
    }

    /// [`Self::record_model_usage`] 的**调用方事务内**变体（compact 落库合并写）。
    pub fn record_model_usage_tx(
        transaction: &rusqlite::Transaction<'_>,
        record: ModelUsageRecord,
    ) -> Result<(), StoreError> {
        let sql = insert_sql("model_usage", MODEL_USAGE_COLUMNS, 40);
        {
            transaction.execute(
                &sql,
                params![
                    record.id,
                    record.session_id,
                    record.logical_request_id,
                    record.provider_id,
                    record.model_id,
                    record.query_source,
                    record.started_at,
                    record.status,
                    record.agent,
                    record.assistant_message_id,
                    record.attempt_index,
                    record.cache_creation_input_tokens,
                    record.cache_read_input_tokens,
                    record.cancelled_by_user,
                    record.completed_at,
                    record.computed_total_tokens,
                    record.context_exceeded,
                    record.duration_ms,
                    record.error_code,
                    record.error_message,
                    record.error_type,
                    record.finish_reason,
                    record.first_token_at,
                    record.input_tokens,
                    record.mode,
                    record.output_tokens,
                    record.parent_user_message_id,
                    record.provider_metadata_json,
                    record.provider_total_tokens,
                    record.raw_usage_json,
                    record.reasoning_tokens,
                    record.retry_count,
                    record.retryable,
                    record.span_id,
                    record.task_type,
                    record.time_to_first_token_ms,
                    record.tool_call_count,
                    record.trace_id,
                    record.turn_id,
                    record.variant,
                ],
            )?;
            Ok(())
        }
    }

    /// 追加一条 turn 用量记录；同一 `(session_id, turn_id)` 重复写入会报主键错误。
    pub fn record_turn_usage(&self, record: TurnUsageRecord) -> Result<(), StoreError> {
        self.with_write_transaction(|transaction| Self::record_turn_usage_tx(transaction, record))
    }

    /// [`Self::record_turn_usage`] 的**调用方事务内**变体（compact 落库合并写）。
    pub fn record_turn_usage_tx(
        transaction: &rusqlite::Transaction<'_>,
        record: TurnUsageRecord,
    ) -> Result<(), StoreError> {
        let sql = insert_sql("turn_usage", TURN_USAGE_COLUMNS, 26);
        {
            transaction.execute(
                &sql,
                params![
                    record.session_id,
                    record.turn_id,
                    record.started_at,
                    record.status,
                    record.cache_creation_input_tokens,
                    record.cache_read_input_tokens,
                    record.cancelled_by_user,
                    record.completed_at,
                    record.computed_total_tokens,
                    record.context_exceeded,
                    record.duration_ms,
                    record.error_code,
                    record.error_type,
                    record.first_model_start_at,
                    record.first_token_at,
                    record.input_tokens,
                    record.model_request_count,
                    record.model_retry_count,
                    record.output_tokens,
                    record.reasoning_tokens,
                    record.retryable,
                    record.time_to_first_token_ms,
                    record.tool_call_count,
                    record.tool_error_count,
                    record.trace_id,
                    record.user_message_id,
                ],
            )?;
            Ok(())
        }
    }

    /// 会话已持久化的 turn id（升序；`turn_usage` 主键的一部分）。
    ///
    /// 冷物化用它恢复 turn 计数器：resume / 重启后新 turn 不能从 `turn_0` 重来，
    /// 否则 `(session_id, turn_id)` 主键冲突（最终评审 C3）。
    pub fn turn_usage_turn_ids(&self, session_id: &str) -> Result<Vec<String>, StoreError> {
        let connection = self.connection();
        let mut statement = connection
            .prepare("SELECT turn_id FROM turn_usage WHERE session_id = ?1 ORDER BY turn_id")?;
        let ids = statement
            .query_map([session_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    /// 追加一条工具调用用量记录；同一 `(session_id, tool_call_id)` 重复写入会报唯一索引错误。
    pub fn record_tool_usage(&self, record: ToolUsageRecord) -> Result<(), StoreError> {
        let sql = insert_sql("tool_usage", TOOL_USAGE_COLUMNS, 27);
        self.with_write_transaction(|transaction| {
            transaction.execute(
                &sql,
                params![
                    record.id,
                    record.session_id,
                    record.tool_call_id,
                    record.tool_name,
                    record.started_at,
                    record.status,
                    record.approval_status,
                    record.cancelled_by_user,
                    record.completed_at,
                    record.destructive,
                    record.duration_ms,
                    record.error_code,
                    record.error_message,
                    record.error_type,
                    record.exit_code,
                    record.first_output_at,
                    record.output_bytes,
                    record.read_only,
                    record.retry_count,
                    record.retryable,
                    record.side_effect_scope,
                    record.stderr_bytes,
                    record.stdout_bytes,
                    record.time_to_first_output_ms,
                    record.trace_id,
                    record.truncated,
                    record.turn_id,
                ],
            )?;
            Ok(())
        })
    }
}

fn insert_sql(table: &str, columns: &str, column_count: usize) -> String {
    let placeholders = (1..=column_count)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("INSERT INTO {table} ({columns}) VALUES ({placeholders})")
}
