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


#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutputTruncated {
    pub total_bytes: Number,
    pub r#ref: String,
}

/// `toolOutputSchema`；`display` 在 TS 里是 `.catch(undefined)`，这里保持原始 JSON。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutput {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<ToolOutputTruncated>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolProgress {
    pub bytes: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_line: Option<String>,
    pub updated_at: Number,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionOutputPreview {
    pub text: String,
    pub full_text: String,
    pub total_lines: u64,
    pub total_bytes: u64,
    pub lines_estimated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CuaAppIdentity {
    pub pid: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCallStatus {
    #[serde(rename = "inputStreaming")]
    InputStreaming,
    #[serde(rename = "pendingApproval")]
    PendingApproval,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCallKind {
    #[serde(rename = "toolCall")]
    ToolCall,
}

/// `toolCallRowSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRow {
    #[serde(flatten)]
    pub base: RowBase,
    pub kind: ToolCallKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_response_id: Option<String>,
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolCallStatus,
    pub input_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cua_app: Option<CuaAppIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<ToolOutput>,
    /// TS 的 `toolCallDisplaySchema.optional().catch(undefined)`：解析失败只丢这张卡。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolCallError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ToolProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<ExecutionOutputPreview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_interaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backgrounded: Option<LiteralTrue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Number>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversationArtifactType {
    #[serde(rename = "pdf")]
    Pdf,
    #[serde(rename = "pptx")]
    Pptx,
    #[serde(rename = "docx")]
    Docx,
    #[serde(rename = "xlsx")]
    Xlsx,
    #[serde(rename = "image")]
    Image,
    #[serde(rename = "html")]
    Html,
    #[serde(rename = "md")]
    Md,
    #[serde(rename = "text")]
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactRowKind {
    #[serde(rename = "artifact")]
    Artifact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactState {
    #[serde(rename = "current")]
    Current,
}

/// `artifactRowSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRow {
    #[serde(flatten)]
    pub base: RowBase,
    pub kind: ArtifactRowKind,
    pub artifact_version_id: String,
    pub logical_artifact_key: String,
    pub display_name: String,
    pub artifact_type: ConversationArtifactType,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub r#ref: String,
    pub state: ArtifactState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentStatus {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentRowKind {
    #[serde(rename = "subagent")]
    Subagent,
}

/// `subagentRowSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRow {
    #[serde(flatten)]
    pub base: RowBase,
    pub kind: SubagentRowKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
    pub subagent_type: String,
    pub status: SubagentStatus,
    pub summary_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backgrounded: Option<LiteralTrue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Number>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookProjectionSourceKind {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "plugin")]
    Plugin,
    #[serde(rename = "project")]
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookExecutionState {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookExecutionOutcome {
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "blocked")]
    Blocked,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "timed_out")]
    TimedOut,
}

/// `hookExecutionProjectionSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookExecutionProjection {
    pub hook_run_id: String,
    pub hook_index: u64,
    pub did_execute: bool,
    pub state: HookExecutionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<HookExecutionOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_reason: Option<String>,
    pub started_at: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<Number>,
    pub display_name: String,
    pub source_kind: HookProjectionSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookEventName {
    #[serde(rename = "SessionStart")]
    SessionStart,
    #[serde(rename = "UserPromptSubmit")]
    UserPromptSubmit,
    #[serde(rename = "PreToolUse")]
    PreToolUse,
    #[serde(rename = "PermissionRequest")]
    PermissionRequest,
    #[serde(rename = "PostToolUse")]
    PostToolUse,
    #[serde(rename = "PostToolUseFailure")]
    PostToolUseFailure,
    #[serde(rename = "Stop")]
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookLane {
    #[serde(rename = "assistantWork")]
    AssistantWork,
    #[serde(rename = "toolBefore")]
    ToolBefore,
    #[serde(rename = "toolAfter")]
    ToolAfter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookInvocationRowKind {
    #[serde(rename = "hookInvocation")]
    HookInvocation,
}

/// `hookInvocationRowSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInvocationRow {
    #[serde(flatten)]
    pub base: RowBase,
    pub kind: HookInvocationRowKind,
    pub hook_invocation_id: String,
    pub hook_event_name: HookEventName,
    pub hook_count: u64,
    pub state: HookExecutionState,
    pub started_at: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<Number>,
    pub lane: HookLane,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_tool_call_id: Option<String>,
    pub executions: Vec<HookExecutionProjection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineMarkerLane {
    #[serde(rename = "assistantWork")]
    AssistantWork,
    #[serde(rename = "turnTailBoundary")]
    TurnTailBoundary,
    #[serde(rename = "lightBoundary")]
    LightBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactOrigin {
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "auto")]
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactStatus {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "noop")]
    Noop,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactType {
    #[serde(rename = "compact")]
    Compact,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactMarker {
    #[serde(rename = "type")]
    pub marker_type: CompactType,
    pub origin: CompactOrigin,
    pub status: CompactStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_after: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForkNoticeType {
    #[serde(rename = "forkNotice")]
    ForkNotice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkNoticeMarker {
    #[serde(rename = "type")]
    pub marker_type: ForkNoticeType,
    pub parent_session_id: String,
    pub parent_row_id: Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForkCreatedType {
    #[serde(rename = "forkCreated")]
    ForkCreated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkCreatedMarker {
    #[serde(rename = "type")]
    pub marker_type: ForkCreatedType,
    pub child_session_id: String,
    pub at_row_id: Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelChangeType {
    #[serde(rename = "modelChange")]
    ModelChange,
}

/// `modelChange` 的「有来源」形态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChangeWithFromMarker {
    #[serde(rename = "type")]
    pub marker_type: ModelChangeType,
    pub from_provider: String,
    pub from_model: String,
    pub to_provider: String,
    pub to_model: String,
    pub to_thought: String,
}

/// `modelChange` 的「显式 ∅→X」形态（无 from 字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChangeToOnlyMarker {
    #[serde(rename = "type")]
    pub marker_type: ModelChangeType,
    pub to_provider: String,
    pub to_model: String,
    pub to_thought: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalSetType {
    #[serde(rename = "goalSet")]
    GoalSet,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSetMarker {
    #[serde(rename = "type")]
    pub marker_type: GoalSetType,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_objective: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalVerifyOutcome {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "pass")]
    Pass,
    #[serde(rename = "notSatisfied")]
    NotSatisfied,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalVerifyType {
    #[serde(rename = "goalVerify")]
    GoalVerify,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalVerifyMarker {
    #[serde(rename = "type")]
    pub marker_type: GoalVerifyType,
    pub iteration: Number,
    pub outcome: GoalVerifyOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetryNoticeType {
    #[serde(rename = "retryNotice")]
    RetryNotice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryNoticeMarker {
    #[serde(rename = "type")]
    pub marker_type: RetryNoticeType,
    pub attempt: Number,
    pub reason_code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckpointRestoredType {
    #[serde(rename = "checkpointRestored")]
    CheckpointRestored,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRestoredMarker {
    #[serde(rename = "type")]
    pub marker_type: CheckpointRestoredType,
    pub checkpoint_id: String,
}

/// `timelineMarkerPayloadSchema`（按 `type` 的判别联合）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TimelineMarkerPayload {
    Compact(CompactMarker),
    ForkNotice(ForkNoticeMarker),
    ForkCreated(ForkCreatedMarker),
    ModelChangeWithFrom(ModelChangeWithFromMarker),
    ModelChangeToOnly(ModelChangeToOnlyMarker),
    GoalSet(GoalSetMarker),
    GoalVerify(GoalVerifyMarker),
    RetryNotice(RetryNoticeMarker),
    CheckpointRestored(CheckpointRestoredMarker),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineMarkerRowKind {
    #[serde(rename = "timelineMarker")]
    TimelineMarker,
}

/// `timelineMarkerRowSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineMarkerRow {
    #[serde(flatten)]
    pub base: RowBase,
    pub kind: TimelineMarkerRowKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<TimelineMarkerLane>,
    pub marker: TimelineMarkerPayload,
}

/// `conversationRowSchema`：按 `kind` 的判别联合。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConversationRow {
    TurnHeader(TurnHeaderRow),
    UserInput(UserInputRow),
    AssistantText(AssistantTextRow),
    Reasoning(ReasoningRow),
    ToolCall(ToolCallRow),
    Artifact(ArtifactRow),
    Subagent(SubagentRow),
    HookInvocation(HookInvocationRow),
    TimelineMarker(TimelineMarkerRow),
}
