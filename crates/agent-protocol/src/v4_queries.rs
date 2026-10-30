//! v4 只读查询：`conversationRowsRange` 与 `conversationPlans`。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/transport.ts`。规范见
//! `docs/specs/session-write-group.md` §「v4 只读查询」。
//!
//! 两者都是「只读、无状态、超时重发安全」的查询，刻意不是 v4 command：没有 ACK、
//! 没有幂等表，调用方按陈旧读防护自行丢弃结果。

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::v4_rows::{ConversationRow, ToolCallRow};
use crate::v4_transport::V4ClientMode;

pub const V4_CONVERSATION_ROWS_RANGE_METHOD: &str = "v4/conversation/rowsRange";
pub const V4_CONVERSATION_PLANS_METHOD: &str = "v4/conversation/plans";
/// 协议限制 `1..=rowsRangeMaxLimit`；由 Agent 侧 schema 拒绝，客户端不改写。
pub const ROWS_RANGE_MAX_LIMIT: u64 = 200;

/// `v4ConversationRowsRangeParamsSchema`：zod 未加 `.strict()`，未知键按 strip 忽略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsRangeParams {
    pub session_id: String,
    /// Host attachment 注入的可信位；renderer 侧调用缺省省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_mode: Option<V4ClientMode>,
    /// 取 `rowId < beforeRowId` 的行；缺省从当前尾部向前。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_row_id: Option<Number>,
    pub limit: Number,
}

/// `v4ConversationRowsRangeResultSchema`：`rows` 按 `rowId` 升序，未知键忽略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsRangeResult {
    pub rows: Vec<ConversationRow>,
    pub at_seq: Number,
    pub at_revision: Number,
    pub at_log_epoch: String,
    pub has_more: bool,
}

/// `v4ConversationPlansParamsSchema`：严格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlansParams {
    pub session_id: String,
}

/// `v4ConversationPlansResultSchema`：严格；`plans` 为当前有效分支的终态 ExitPlanMode，
/// `rowId` 降序。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlansResult {
    pub plans: Vec<ToolCallRow>,
    pub at_seq: Number,
    pub at_log_epoch: String,
}
