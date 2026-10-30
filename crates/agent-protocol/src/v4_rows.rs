//! v4 会话行（第一批）：`RowBase` 与 `turnHeader` / `userInput` / `assistantText` / `reasoning`。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/rows.ts`。注意这些 row schema 是
//! `z.object({...})` **不带 `.strict()`**：zod 默认剥离未知键，所以 Rust 侧同样忽略未知键
//! （不用 `deny_unknown_fields`），并用 `flatten` 复用公共字段。
//!
//! 规范见 `docs/specs/session-write-group.md` §「v4 会话行」。后续切片再补
//! `toolCall` / `artifact` / `subagent` / `hookInvocation` / `timelineMarker` 与行联合。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::workflow::LiteralTrue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditDisposition {
    #[serde(rename = "rewind")]
    Rewind,
    #[serde(rename = "fork")]
    Fork,
}

/// `rowActionsSchema`：只下发为 `true` 的键。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowActions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_fork: Option<LiteralTrue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_edit: Option<LiteralTrue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_retry: Option<LiteralTrue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_rewind_files: Option<LiteralTrue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_disposition: Option<EditDisposition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowVisibility {
    #[serde(rename = "visible")]
    Visible,
}

/// `rowBaseFields`：所有行共有的字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowBase {
    /// session 内单调、永不复用。
    pub row_id: Number,
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<RowVisibility>,
    pub created_at: Number,
    pub created_at_seq: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<RowActions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnWorkSegment {
    pub segment_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_entity_id: Option<String>,
    pub started_at: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_ms: Option<Number>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnFileChanges {
    pub additions: Number,
    pub deletions: Number,
    pub files: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<TurnFileChangeState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnFileChangeState {
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "reverted")]
    Reverted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackgroundSource {
    #[serde(rename = "bash")]
    Bash,
    #[serde(rename = "subagent")]
    Subagent,
    #[serde(rename = "workflow")]
    Workflow,
}

/// `backgroundResultOriginMetaSchema`；`workflowNotification` 暂按原始 JSON 透传
/// （其判别联合在后续切片建模）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundResultOriginMeta {
    pub background_source: BackgroundSource,
    pub work_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_notification: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowLaunchScope {
    #[serde(rename = "project")]
    Project,
    #[serde(rename = "global")]
    Global,
}

/// `workflowLaunchMetaSchema`；`display` 与 `amend` 暂按原始 JSON 透传。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowLaunchMeta {
    pub run_id: String,
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<WorkflowLaunchScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amend: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnOrigin {
    #[serde(rename = "userInput")]
    UserInput,
    #[serde(rename = "backgroundResult")]
    BackgroundResult,
    #[serde(rename = "goalContinuation")]
    GoalContinuation,
    #[serde(rename = "editRerun")]
    EditRerun,
    #[serde(rename = "workflowLaunch")]
    WorkflowLaunch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionKind {
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "controlOnly")]
    ControlOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnState {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completedSuccess")]
    CompletedSuccess,
    #[serde(rename = "completedInterrupted")]
    CompletedInterrupted,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnHeaderKind {
    #[serde(rename = "turnHeader")]
    TurnHeader,
}

/// `turnHeaderRowSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnHeaderRow {
    #[serde(flatten)]
    pub base: RowBase,
    pub kind: TurnHeaderKind,
    pub origin: TurnOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_kind: Option<ExecutionKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_round_count: Option<u64>,
    pub state: TurnState,
    pub started_at: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_ms: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_segments: Option<Vec<TurnWorkSegment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_meta: Option<BackgroundResultOriginMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_launch: Option<WorkflowLaunchMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_changes: Option<TurnFileChanges>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserInputOrigin {
    #[serde(rename = "realUser")]
    RealUser,
    #[serde(rename = "backgroundResult")]
    BackgroundResult,
    #[serde(rename = "goalContinuation")]
    GoalContinuation,
    #[serde(rename = "mailbox")]
    Mailbox,
    #[serde(rename = "synthetic")]
    Synthetic,
    #[serde(rename = "workflowLaunch")]
    WorkflowLaunch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputAttachment {
    pub r#ref: String,
    pub file_name: String,
    pub mime: String,
    pub bytes: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserInputKind {
    #[serde(rename = "userInput")]
    UserInput,
}

/// `userInputRowSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputRow {
    #[serde(flatten)]
    pub base: RowBase,
    pub kind: UserInputKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epilogue_start: Option<u64>,
    pub origin: UserInputOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_meta: Option<UserInputOriginMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_launch: Option<WorkflowLaunchMeta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guided: Option<LiteralTrue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_source_command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<UserInputAttachment>>,
}

/// `userInput.originMeta`（与 `backgroundResultOriginMeta` 同一组取值，但形状更窄）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputOriginMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_source: Option<BackgroundSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssistantTextKind {
    #[serde(rename = "assistantText")]
    AssistantText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssistantTextState {
    #[serde(rename = "streaming")]
    Streaming,
    #[serde(rename = "complete")]
    Complete,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Feedback {
    #[serde(rename = "like")]
    Like,
    #[serde(rename = "dislike")]
    Dislike,
}

/// `assistantTextRowSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantTextRow {
    #[serde(flatten)]
    pub base: RowBase,
    pub kind: AssistantTextKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_response_id: Option<String>,
    pub text: String,
    pub state: AssistantTextState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<Feedback>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasoningKind {
    #[serde(rename = "reasoning")]
    Reasoning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasoningState {
    #[serde(rename = "streaming")]
    Streaming,
    #[serde(rename = "complete")]
    Complete,
    #[serde(rename = "interrupted")]
    Interrupted,
}

/// `reasoningRowSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningRow {
    #[serde(flatten)]
    pub base: RowBase,
    pub kind: ReasoningKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_response_id: Option<String>,
    pub text: String,
    pub state: ReasoningState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<Number>,
}

