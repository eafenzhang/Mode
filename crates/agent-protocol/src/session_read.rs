//! 会话只读组的严格线格式镜像（`session/list`、`session/events`）。
//!
//! 真源：`packages/shared/src/mode-protocol-legacy-types.ts` 的 `modeSessionInfoSchema` /
//! `modeWorkspaceRefSchema` / `modeSessionGoalSchema`，以及
//! `packages/shared/src/mode-protocol/index.ts` 的 `modeSessionListParamsSchema` /
//! `modeSessionListResultSchema` / `modeSessionEventsParamsSchema` /
//! `modeSessionEventsResultSchema`。规范见 `docs/specs/session-read-group.md`。
//!
//! 这些 schema 都是 `.strict()`，所以这里全部 `deny_unknown_fields`：多一个键就拒绝。
//! `summaryTitle` / `activeInputId` / `activeRun*` 是 `nullable().optional()`，镜像用
//! `Option<Option<T>>` 保留「省略」与「显式 null」的区别，round-trip 才能逐字节回到线上。
//!
//! `session/events` 复用 `session_event::SessionEvent` 信封镜像：信封严格、载荷保持
//! `Value`。载荷按类型逐个 strict 是 TS zod 的行为，但镜像只对 M2 用到的类型做了强类型，
//! 其余载荷原样交给消费方（见 `docs/specs/session-read-group.md` 的差异一节）。

use serde::{Deserialize, Deserializer, Serialize};

use crate::session_event::SessionEvent;

/// `nullable().optional()` 的三态解码：键缺失走 `default`，显式 `null` 得到 `Some(None)`。
///
/// 直接写 `Option<Option<T>>` 不够：serde 解 `null` 时外层 `Option` 直接得到 `None`，
/// 「省略」与「显式 null」会被合并，round-trip 就写不回原样。
fn deserialize_nullable<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

pub const SESSION_LIST_METHOD: &str = "session/list";
pub const SESSION_EVENTS_METHOD: &str = "session/events";
pub const SESSION_SUBAGENTS_METHOD: &str = "session/subagents";

/// 结果里的 workspace ref（`modeWorkspaceRefSchema`，严格四键）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionWorkspaceRef {
    pub workspace_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_session_id: Option<String>,
    pub workspace_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionModelOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionModel {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<SessionModelOptions>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionKind {
    #[serde(rename = "interactive")]
    Interactive,
    #[serde(rename = "fork")]
    Fork,
    #[serde(rename = "selection_side_chat")]
    SelectionSideChat,
    #[serde(rename = "workflow_parent")]
    WorkflowParent,
    #[serde(rename = "workflow_child")]
    WorkflowChild,
    #[serde(rename = "subagent_child")]
    SubagentChild,
    #[serde(rename = "nested_workflow_child")]
    NestedWorkflowChild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMode {
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "build")]
    Build,
    #[serde(rename = "edit")]
    Edit,
    #[serde(rename = "yolo")]
    Yolo,
    #[serde(rename = "auto")]
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionStatus {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "paused")]
    Paused,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionTitleSource {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "first_input")]
    FirstInput,
    #[serde(rename = "generated")]
    Generated,
    #[serde(rename = "custom")]
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionGoalStatus {
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "paused")]
    Paused,
    #[serde(rename = "budget_limited")]
    BudgetLimited,
    #[serde(rename = "complete")]
    Complete,
}

/// `modeSessionGoalSchema`（`.strict()`，`summaryTitle` 默认 `null`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionGoal {
    pub session_id: String,
    pub target_id: String,
    pub objective: String,
    /// `nullable().default(null)`：省略写省略，null 写 null。
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub summary_title: Option<Option<String>>,
    pub status: SessionGoalStatus,
    /// `nullable()` 且键必在。
    pub token_budget: Option<u64>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub active_input_id: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub active_run_started_at_ms: Option<Option<u64>>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub active_run_last_seen_at_ms: Option<Option<u64>>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// `modeSessionInfoSchema`（`.strict()`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionInfo {
    pub session_id: String,
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    pub session_kind: SessionKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_source: Option<SessionTitleSource>,
    pub mode: SessionMode,
    pub status: SessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<SessionModel>,
    /// `nullable().optional()`。
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub target: Option<Option<SessionGoal>>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<u64>,
}

/// `session/list` 的 params（服务层构造；`limit` / `sessionIds` 缺省时省略键）。
///
/// 字段顺序与 TS 请求字面量一致：`workspace`、`includeArchived`、`limit`，`sessionIds`
/// 由 spread 追加在最后。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<SessionWorkspaceRef>,
    pub include_archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ids: Option<Vec<String>>,
}

/// `modeSessionListResultSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionListResult {
    pub sessions: Vec<SessionInfo>,
}

/// `session/events` 的 params（`modeSessionEventsParamsSchema`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionEventsParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// `modeSessionEventsResultSchema`：信封严格，载荷按事件类型解释。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionEventsResult {
    pub events: Vec<SessionEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunningSubagentStatus {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "blocked")]
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndedSubagentStatus {
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "lost")]
    Lost,
}

/// `modeSessionRunningSubagentSchema`（base + `status`，全部 `.strict()`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunningSubagent {
    pub child_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub subagent_type: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    pub status: RunningSubagentStatus,
}

/// `modeSessionEndedSubagentSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndedSubagent {
    pub child_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub subagent_type: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    pub status: EndedSubagentStatus,
}

/// `session/subagents` 的 params；`endedLimit` 恒发（服务层缺省 20）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSubagentsParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_cursor: Option<String>,
    pub ended_limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSubagentsEnded {
    pub total: u64,
    pub items: Vec<EndedSubagent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// `modeSessionSubagentsResultSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSubagentsResult {
    pub revision: u64,
    pub child_session_ids: Vec<String>,
    pub running: Vec<RunningSubagent>,
    pub ended: SessionSubagentsEnded,
}


