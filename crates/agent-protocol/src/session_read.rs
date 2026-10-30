//! `session/list` 的严格线格式镜像（会话只读组第一片）。
//!
//! 真源：`packages/shared/src/mode-protocol-legacy-types.ts` 的 `modeSessionInfoSchema` /
//! `modeWorkspaceRefSchema` / `modeSessionGoalSchema`，以及
//! `packages/shared/src/mode-protocol/index.ts` 的 `modeSessionListParamsSchema` /
//! `modeSessionListResultSchema`。规范见 `docs/specs/session-read-group.md`。
//!
//! 这些 schema 都是 `.strict()`，所以这里全部 `deny_unknown_fields`：多一个键就拒绝。
//! `summaryTitle` / `activeInputId` / `activeRun*` 是 `nullable().optional()`，镜像用
//! `Option<Option<T>>` 保留「省略」与「显式 null」的区别，round-trip 才能逐字节回到线上。

use serde::{Deserialize, Deserializer, Serialize};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
