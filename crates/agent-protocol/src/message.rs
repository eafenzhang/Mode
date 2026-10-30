//! `session/messages` 的消息 / 消息部件线格式镜像。
//!
//! 真源：`packages/shared/src/mode-protocol-legacy-types.ts` 的 `modeMessageInfoSchema`、
//! `modeMessagePartSchema`、`modeToolStateSchema`、`modeTokenUsageSchema` 与
//! `modeMessageSemanticsSchema`。规范见 `docs/specs/session-read-group.md`。
//!
//! 全部 schema 都是 `.strict()`。判别联合（`role` / `type` / `status`）在这里用
//! `#[serde(untagged)]` + 各分支 `deny_unknown_fields` 表达：serde 不支持「内部标记 +
//! deny_unknown_fields」，untagged 分支各自严格，因此未知键仍会被拒绝。
//!
//! `cost` 用 `serde_json::Number` 而不是 `f64`：线上既有整数也有小数，转成 `f64` 会把
//! `0` 写成 `0.0`，round-trip 就不再逐字节相等。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::session_read::deserialize_nullable;

type JsonObject = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenCacheUsage {
    pub read: u64,
    pub write: u64,
}

/// `modeTokenUsageSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache: TokenCacheUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageTime {
    pub created: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MessageOrigin {
    #[serde(rename = "real_user")]
    RealUser,
    #[serde(rename = "agent_runtime")]
    AgentRuntime,
    #[serde(rename = "system")]
    System,
    #[serde(rename = "migration")]
    Migration,
    #[serde(rename = "import")]
    Import,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MessageKind {
    #[serde(rename = "user_prompt")]
    UserPrompt,
    #[serde(rename = "slash_command")]
    SlashCommand,
    #[serde(rename = "system_reminder")]
    SystemReminder,
    #[serde(rename = "background_notification")]
    BackgroundNotification,
    #[serde(rename = "subagent_notification")]
    SubagentNotification,
    #[serde(rename = "todo_reminder")]
    TodoReminder,
    #[serde(rename = "rewind_notice")]
    RewindNotice,
    #[serde(rename = "fork_notice")]
    ForkNotice,
    #[serde(rename = "timeline_event")]
    TimelineEvent,
    #[serde(rename = "compact_summary")]
    CompactSummary,
    #[serde(rename = "shared_context")]
    SharedContext,
    #[serde(rename = "assistant_response")]
    AssistantResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum UiVisibility {
    #[serde(rename = "visible")]
    Visible,
    #[serde(rename = "hidden")]
    Hidden,
    #[serde(rename = "debug")]
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ProviderVisibility {
    #[serde(rename = "visible")]
    Visible,
    #[serde(rename = "hidden")]
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TranscriptVisibility {
    #[serde(rename = "visible")]
    Visible,
    #[serde(rename = "hidden")]
    Hidden,
}

/// `modeMessageSemanticsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageSemantics {
    pub origin: MessageOrigin,
    pub kind: MessageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_name: Option<String>,
    pub ui_visibility: UiVisibility,
    pub provider_visibility: ProviderVisibility,
    pub transcript_visibility: TranscriptVisibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SyntheticUserMessageSource {
    #[serde(rename = "background_task")]
    BackgroundTask,
    #[serde(rename = "fork")]
    Fork,
    #[serde(rename = "goal_state_change")]
    GoalStateChange,
    #[serde(rename = "goal-continuation")]
    GoalContinuation,
    #[serde(rename = "plugin_reference")]
    PluginReference,
    #[serde(rename = "rewind")]
    Rewind,
    #[serde(rename = "selection_side_chat")]
    SelectionSideChat,
    #[serde(rename = "subagent")]
    Subagent,
    #[serde(rename = "subagent_message")]
    SubagentMessage,
    #[serde(rename = "todo_reminder")]
    TodoReminder,
    #[serde(rename = "workflow_launch")]
    WorkflowLaunch,
    #[serde(rename = "shared_context")]
    SharedContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MessageVisibility {
    #[serde(rename = "user-visible")]
    UserVisible,
    #[serde(rename = "model-only")]
    ModelOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum UserRole {
    #[serde(rename = "user")]
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AssistantRole {
    #[serde(rename = "assistant")]
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageModel {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ModelOptions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessagePath {
    pub cwd: String,
    pub root: String,
}

/// `modeUserMessageInfoSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserMessageInfo {
    pub message_id: String,
    pub session_id: String,
    pub role: UserRole,
    pub time: MessageTime,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<MessageModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SyntheticUserMessageSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<MessageVisibility>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantics: Option<MessageSemantics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

/// `modeAssistantMessageInfoSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistantMessageInfo {
    pub message_id: String,
    pub session_id: String,
    pub role: AssistantRole,
    pub time: MessageTime,
    pub parent_message_id: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<MessageModel>,
    pub path: MessagePath,
    /// 线上既有整数也有小数；用 `Number` 才能逐字节写回。
    pub cost: Number,
    pub tokens: TokenUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantics: Option<MessageSemantics>,
    /// `z.unknown().optional()`：任意 JSON；显式 null 与省略分开保留。
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub structured: Option<Option<Value>>,
}

/// `modeMessageInfoSchema`：按 `role` 的判别联合。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageInfo {
    User(UserMessageInfo),
    Assistant(AssistantMessageInfo),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToolStatusPending {
    #[serde(rename = "pending")]
    Pending,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToolStatusRunning {
    #[serde(rename = "running")]
    Running,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToolStatusCompleted {
    #[serde(rename = "completed")]
    Completed,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToolStatusError {
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolStatePending {
    pub status: ToolStatusPending,
    pub input: JsonObject,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolStateRunning {
    pub status: ToolStatusRunning,
    pub input: JsonObject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    pub started_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolStateCompleted {
    pub status: ToolStatusCompleted,
    pub input: JsonObject,
    pub output: String,
    pub title: String,
    pub metadata: JsonObject,
    pub started_at: u64,
    pub completed_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolStateError {
    pub status: ToolStatusError,
    pub input: JsonObject,
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    pub started_at: u64,
    pub completed_at: u64,
}

/// `modeToolStateSchema`：按 `status` 的判别联合。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolState {
    Pending(ToolStatePending),
    Running(ToolStateRunning),
    Completed(ToolStateCompleted),
    Error(ToolStateError),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeText {
    #[serde(rename = "text")]
    Text,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeReasoning {
    #[serde(rename = "reasoning")]
    Reasoning,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeFile {
    #[serde(rename = "file")]
    File,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeTool {
    #[serde(rename = "tool")]
    Tool,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeStepStart {
    #[serde(rename = "step-start")]
    StepStart,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeStepFinish {
    #[serde(rename = "step-finish")]
    StepFinish,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeSnapshot {
    #[serde(rename = "snapshot")]
    Snapshot,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypePatch {
    #[serde(rename = "patch")]
    Patch,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeCompaction {
    #[serde(rename = "compaction")]
    Compaction,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeTimeline {
    #[serde(rename = "timeline")]
    Timeline,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeSubagent {
    #[serde(rename = "subagent")]
    Subagent,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeAgent {
    #[serde(rename = "agent")]
    Agent,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PartTypeRetry {
    #[serde(rename = "retry")]
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TimelineType {
    #[serde(rename = "context_compaction")]
    ContextCompaction,
    #[serde(rename = "goal_verification")]
    GoalVerification,
    #[serde(rename = "session_fork")]
    SessionFork,
    #[serde(rename = "model_change")]
    ModelChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TimelineDisplay {
    #[serde(rename = "separator")]
    Separator,
    #[serde(rename = "worklog")]
    Worklog,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TimelineTrigger {
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "partial")]
    Partial,
    #[serde(rename = "reactive")]
    Reactive,
    #[serde(rename = "session_memory")]
    SessionMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TimelinePhase {
    #[serde(rename = "standalone_turn")]
    StandaloneTurn,
    #[serde(rename = "pre_request")]
    PreRequest,
    #[serde(rename = "mid_turn")]
    MidTurn,
    #[serde(rename = "reactive")]
    Reactive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelinePartTime {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelineVerification {
    pub passed: bool,
    pub reason: String,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub next_action: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelineFromModel {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ModelOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelineToModel {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<ModelOptions>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelinePart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeTimeline,
    pub timeline_type: TimelineType,
    pub display: TimelineDisplay,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<TimelinePartTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TimelineTrigger>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<TimelinePhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_compact_token_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_compact_token_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub true_post_compact_token_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_iteration: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<TimelineVerification>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_checkpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_file_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_model: Option<TimelineFromModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_model: Option<TimelineToModel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeText,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignored: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReasoningPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeReasoning,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilePart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeFile,
    pub mime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// 允许空串（按路径引用的附件线上就是空串）。
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeTool,
    pub call_id: String,
    pub tool: String,
    pub state: ToolState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepStartPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeStepStart,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepFinishPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeStepFinish,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    /// 与 `cost` 一样保留 JSON 数字原样。
    pub cost: Number,
    pub tokens: TokenUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeSnapshot,
    pub snapshot: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypePatch,
    pub hash: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompactionPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeCompaction,
    pub auto: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubagentPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeSubagent,
    pub prompt: String,
    pub description: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<MessageModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeAgent,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryPart {
    pub part_id: String,
    pub session_id: String,
    pub message_id: String,
    #[serde(rename = "type")]
    pub part_type: PartTypeRetry,
    pub attempt: u64,
    pub error: JsonObject,
}

/// `modeMessagePartSchema`：按 `type` 的判别联合（13 个分支）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessagePart {
    Text(TextPart),
    Reasoning(ReasoningPart),
    File(FilePart),
    Tool(ToolPart),
    StepStart(StepStartPart),
    StepFinish(StepFinishPart),
    Snapshot(SnapshotPart),
    Patch(PatchPart),
    Compaction(CompactionPart),
    Timeline(Box<TimelinePart>),
    Subagent(SubagentPart),
    Agent(AgentPart),
    Retry(RetryPart),
}

/// `modeMessageWithPartsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageWithParts {
    pub info: MessageInfo,
    pub parts: Vec<MessagePart>,
}
