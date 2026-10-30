//! v4 `conversationSnapshot`（A 区 + rows 窗口）。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/snapshot.ts` 与 `session-config.ts` /
//! `input-intent.ts` / `shared-context-import.ts` / `workspace-hook-review.ts`。
//!
//! 范围：
//! - A 区：control / availability / inputRouting / meta / sharedContextImport / config /
//!   modelTransition / usage / queue / pendingInteractions / pendingCommands /
//!   backgroundWorks / subagents / goal / plan / workspaceHookAdmission；
//! - B 区：`rows` 窗口（[`crate::v4_rows::ConversationRow`]）。
//!
//! **未做**：`workflowRuns` 暂按原始 JSON 透传（`workflow-runs.ts` 体量大，独立切片）。
//! 规范见 `docs/specs/session-write-group.md` §「v4 会话快照」。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::message::MessageModel;
use crate::session_read::deserialize_nullable;
use crate::snapshot::{
    BreakdownItem, ContextCacheUsage, ErrorAttribution, InteractionOrigin, PermissionResponse,
};
use crate::v4_rows::ConversationRow;
use crate::workflow::{LiteralFalse, LiteralTrue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionPhase {
    #[serde(rename = "draft")]
    Draft,
    #[serde(rename = "prewarming")]
    Prewarming,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completedSuccess")]
    CompletedSuccess,
    #[serde(rename = "completedInterrupted")]
    CompletedInterrupted,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopTargetKind {
    #[serde(rename = "assistant")]
    Assistant,
    #[serde(rename = "tool")]
    Tool,
    #[serde(rename = "subagent")]
    Subagent,
    #[serde(rename = "compact")]
    Compact,
    #[serde(rename = "goalVerifier")]
    GoalVerifier,
    #[serde(rename = "goalContinuation")]
    GoalContinuation,
    #[serde(rename = "turnSteer")]
    TurnSteer,
    #[serde(rename = "mixed")]
    Mixed,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActiveWorkKind {
    #[serde(rename = "primaryTurn")]
    PrimaryTurn,
    #[serde(rename = "foregroundSubagent")]
    ForegroundSubagent,
    #[serde(rename = "compact")]
    Compact,
    #[serde(rename = "goalVerifier")]
    GoalVerifier,
    #[serde(rename = "goalContinuation")]
    GoalContinuation,
    #[serde(rename = "turnSteer")]
    TurnSteer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveWorkSummary {
    pub kind: ActiveWorkKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_execution_id: Option<String>,
    pub started_at: Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorSource {
    #[serde(rename = "provider")]
    Provider,
    #[serde(rename = "runtime")]
    Runtime,
    #[serde(rename = "tool")]
    Tool,
    #[serde(rename = "network")]
    Network,
}

/// `sessionErrorInfoSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionErrorInfo {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
    pub at: Number,
    pub source: ErrorSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlying_error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlying_error_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<ErrorAttribution>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiRetryState {
    pub attempt: Number,
    pub max_attempts: Number,
    pub next_retry_at: Number,
    pub reason_code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopState {
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "stoppable")]
    Stoppable,
    #[serde(rename = "stopping")]
    Stopping,
}

/// `sessionControlSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionControl {
    pub phase: SessionPhase,
    pub session_ended: bool,
    pub can_stop: bool,
    pub stop_state: StopState,
    pub stop_target_kind: StopTargetKind,
    pub active_works: Vec<ActiveWorkSummary>,
    pub last_error: Option<SessionErrorInfo>,
    pub api_retry: Option<ApiRetryState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowedAction {
    pub allowed: LiteralTrue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeniedAction {
    pub allowed: LiteralFalse,
    pub reason_code: String,
}

/// `actionAvailabilitySchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActionAvailability {
    Allowed(AllowedAction),
    Denied(DeniedAction),
}

/// `sessionActionAvailabilitySchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionAvailability {
    pub fork: ActionAvailability,
    pub compact: ActionAvailability,
    pub switch_model_config: ActionAvailability,
    pub set_followup_mode: ActionAvailability,
    pub queue_edit: ActionAvailability,
    pub send_queued_now: ActionAvailability,
    pub pause_goal: ActionAvailability,
    pub resume_goal: ActionAvailability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputRoutingMode {
    #[serde(rename = "startNow")]
    StartNow,
    #[serde(rename = "enqueue")]
    Enqueue,
    #[serde(rename = "guide")]
    Guide,
    #[serde(rename = "reject")]
    Reject,
    #[serde(rename = "choice")]
    Choice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputRouting {
    pub mode: InputRoutingMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TitleSource {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "generated")]
    Generated,
    #[serde(rename = "custom")]
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetaState {
    pub title: String,
    pub title_source: TitleSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SharedContextImportStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "reserved")]
    Reserved,
    #[serde(rename = "attached")]
    Attached,
    #[serde(rename = "discarded")]
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedContextImportV2 {
    pub context_id: String,
    pub title: String,
    pub share_url: String,
    pub status: SharedContextImportStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySharedContextImport {
    pub title: String,
}

/// `sharedContextImportStateSchema`（V2 或 legacy 形状）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SharedContextImportState {
    V2(SharedContextImportV2),
    Legacy(LegacySharedContextImport),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FollowupMode {
    #[serde(rename = "queue")]
    Queue,
    #[serde(rename = "guide")]
    Guide,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionGrant {
    pub interaction_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTransition {
    pub tool_call_id: String,
    pub plan_enabled: bool,
}

/// `sessionConfigStateSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionConfigState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<MessageModel>,
    pub provider: String,
    pub model: String,
    pub thought: String,
    #[serde(default)]
    pub thought_levels: Vec<String>,
    pub followup_mode: FollowupMode,
    #[serde(default = "default_session_mode")]
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_grant: Option<PermissionGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_transition: Option<PlanTransition>,
}

fn default_session_mode() -> String {
    "build".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTransitionEndpoint {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelTransitionOrigin {
    #[serde(rename = "registryFallback")]
    RegistryFallback,
}

/// `sessionModelTransitionSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionModelTransition {
    pub event_id: String,
    pub origin: ModelTransitionOrigin,
    pub from: ModelTransitionEndpoint,
    pub to: ModelTransitionEndpoint,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWindowUsage {
    pub used_tokens: Number,
    pub max_tokens: Number,
    /// `nullable()`：键必在。
    pub auto_compact_threshold_tokens: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<ContextCacheUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakdown: Option<Vec<BreakdownItem>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CumulativeUsage {
    pub input_tokens: Number,
    pub output_tokens: Number,
    pub cache_read_tokens: Number,
    pub cache_write_tokens: Number,
}

/// `sessionUsageStateSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsageState {
    pub context_window: Option<ContextWindowUsage>,
    pub cumulative: CumulativeUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueAttachmentRef {
    pub r#ref: String,
    pub file_name: String,
    pub mime: String,
    pub bytes: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubmissionMode {
    #[serde(rename = "build")]
    Build,
    #[serde(rename = "edit")]
    Edit,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "yolo")]
    Yolo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedContextRef {
    pub kind: SharedContextRefKind,
    pub context_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SharedContextRefKind {
    #[serde(rename = "shared_context_import")]
    SharedContextImport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputDeliveryRequested {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "startNow")]
    StartNow,
    #[serde(rename = "queue")]
    Queue,
    #[serde(rename = "guide")]
    Guide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputDeliveryAdmitted {
    #[serde(rename = "startNow")]
    StartNow,
    #[serde(rename = "queue")]
    Queue,
    #[serde(rename = "guide")]
    Guide,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationInputDelivery {
    pub requested: InputDeliveryRequested,
    pub admitted: InputDeliveryAdmitted,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationInputOrder {
    pub admission_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SteerState {
    #[serde(rename = "notRequested")]
    NotRequested,
    #[serde(rename = "submitting")]
    Submitting,
    #[serde(rename = "steering")]
    Steering,
    #[serde(rename = "guided")]
    Guided,
    #[serde(rename = "fellBack")]
    FellBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationInputSteer {
    pub state: SteerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DispatchState {
    #[serde(rename = "admitted")]
    Admitted,
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "reserved")]
    Reserved,
    #[serde(rename = "promoting")]
    Promoting,
    #[serde(rename = "drained")]
    Drained,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationInputDispatch {
    pub state: DispatchState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputProvenance {
    pub source_command_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentKind {
    #[serde(rename = "sendText")]
    SendText,
    #[serde(rename = "sendGoalCommand")]
    SendGoalCommand,
    #[serde(rename = "compact")]
    Compact,
}

/// `conversationInputIntentSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationInputIntent {
    pub source_command_id: String,
    pub queue_item_id: String,
    pub client_id: String,
    pub kind: IntentKind,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<QueueAttachmentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<MessageModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SubmissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_context_refs: Option<Vec<SharedContextRef>>,
    pub delivery: ConversationInputDelivery,
    pub order: ConversationInputOrder,
    pub steer: ConversationInputSteer,
    pub dispatch: ConversationInputDispatch,
    pub admitted_at: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<InputProvenance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueDispatchState {
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "reserved")]
    Reserved,
    #[serde(rename = "promoting")]
    Promoting,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueDispatch {
    pub state: QueueDispatchState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation_id: Option<String>,
}

/// `queueItemSchema`：intent 的全部字段 + 队列态 `dispatch` + 可选 denylist。
///
/// 不使用 `flatten`：intent 自己也有 `dispatch` 字段，扁平化会写出重复键。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub source_command_id: String,
    pub queue_item_id: String,
    pub client_id: String,
    pub kind: IntentKind,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<QueueAttachmentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<MessageModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SubmissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_context_refs: Option<Vec<SharedContextRef>>,
    pub delivery: ConversationInputDelivery,
    pub order: ConversationInputOrder,
    pub steer: ConversationInputSteer,
    pub dispatch: QueueDispatch,
    pub admitted_at: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<InputProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_disallowlist: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueuePauseReason {
    #[serde(rename = "stopped")]
    Stopped,
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueState {
    pub items: Vec<QueueItem>,
    pub auto_drain: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<QueuePauseReason>,
}

// ---- pendingInteractions ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum V4PermissionOptionKind {
    #[serde(rename = "allowOnce")]
    AllowOnce,
    #[serde(rename = "allowAlways")]
    AllowAlways,
    #[serde(rename = "deny")]
    Deny,
    #[serde(rename = "custom")]
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct V4PermissionOption {
    pub option_id: String,
    pub label: String,
    pub kind: V4PermissionOptionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<PermissionResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionRequestKind {
    #[serde(rename = "permission")]
    Permission,
}

/// `permissionRequestPayloadSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequestPayload {
    pub kind: PermissionRequestKind,
    pub tool_call_id: String,
    pub tool_name: String,
    pub summary: String,
    pub detail: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_text: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<InteractionOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_access_option: Option<V4PermissionOption>,
    pub options: Vec<V4PermissionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputOptionPayload {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputQuestionPayload {
    pub question: String,
    pub header: String,
    pub options: Vec<UserInputOptionPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_select: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputSimpleOption {
    pub option_id: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserInputRequestKind {
    #[serde(rename = "userInput")]
    UserInput,
}

/// `userInputRequestPayloadSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInputRequestPayload {
    pub kind: UserInputRequestKind,
    pub prompt: String,
    pub free_text: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<UserInputSimpleOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub questions: Option<Vec<UserInputQuestionPayload>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_question_index: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_drafts: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<InteractionOrigin>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookReviewEvent {
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
pub enum HookReviewExecutionType {
    #[serde(rename = "command")]
    Command,
    #[serde(rename = "process")]
    Process,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookReviewExecutionMode {
    #[serde(rename = "foreground")]
    Foreground,
    #[serde(rename = "background")]
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookReviewTrustState {
    #[serde(rename = "not_applicable")]
    NotApplicable,
    #[serde(rename = "pending_trust")]
    PendingTrust,
    #[serde(rename = "trusted_persistent")]
    TrustedPersistent,
    #[serde(rename = "blocked_untrusted")]
    BlockedUntrusted,
    #[serde(rename = "blocked_policy")]
    BlockedPolicy,
    #[serde(rename = "revoked")]
    Revoked,
    #[serde(rename = "stale_digest")]
    StaleDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookReviewSourceFile {
    pub path: String,
    pub display_path: String,
    pub editable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookReviewSummary {
    pub event_count: u64,
    pub hook_count: u64,
    pub pending_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookReviewItem {
    pub review_item_id: String,
    pub event: HookReviewEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    #[serde(rename = "type")]
    pub item_type: HookReviewExecutionType,
    pub display_name: String,
    pub display_command: String,
    pub source_path: String,
    pub resolved_timeout_ms: u64,
    pub resolved_max_output_bytes: u64,
    pub execution_mode: HookReviewExecutionMode,
    pub configured_enabled: bool,
    pub editable: bool,
    pub trust_state: HookReviewTrustState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceHookReviewKind {
    #[serde(rename = "workspaceHookReview")]
    WorkspaceHookReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookReviewWarningCode {
    #[serde(rename = "workspace_hooks_execute_code")]
    WorkspaceHooksExecuteCode,
}

/// `workspaceHookReviewRequestPayloadSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceHookReviewRequestPayload {
    pub kind: WorkspaceHookReviewKind,
    pub review_flow_id: String,
    pub generation: u64,
    pub interaction_id: String,
    pub session_id: String,
    pub task_id: String,
    pub run_id: String,
    pub workspace_identity: String,
    pub workspace_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_session_id: Option<String>,
    pub bundle_digest: String,
    pub created_at: Number,
    pub deadline_at: Number,
    pub source_files: Vec<HookReviewSourceFile>,
    pub summary: HookReviewSummary,
    pub items: Vec<HookReviewItem>,
    pub warning_code: HookReviewWarningCode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PendingInteractionPayload {
    Permission(PermissionRequestPayload),
    UserInput(UserInputRequestPayload),
    WorkspaceHookReview(WorkspaceHookReviewRequestPayload),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PendingInteractionKind {
    #[serde(rename = "permission")]
    Permission,
    #[serde(rename = "userInput")]
    UserInput,
    #[serde(rename = "workspaceHookReview")]
    WorkspaceHookReview,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HiddenGraceAutoResolution {
    pub state: AutoResolutionState,
    pub started_at: Number,
    pub visible_at: Number,
    pub deadline_at: Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoResolutionState {
    #[serde(rename = "hiddenGrace")]
    HiddenGrace,
    #[serde(rename = "visibleCountdown")]
    VisibleCountdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnoozedState {
    #[serde(rename = "snoozed")]
    Snoozed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozedAutoResolution {
    pub state: SnoozedState,
    pub started_at: Number,
    pub snoozed_at: Number,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InteractionAutoResolution {
    Grace(HiddenGraceAutoResolution),
    Snoozed(SnoozedAutoResolution),
}

/// `pendingInteractionSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingInteraction {
    pub interaction_id: String,
    pub kind: PendingInteractionKind,
    /// `nullable()`：键必在。
    pub anchor_row_id: Option<Number>,
    pub created_at: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resolution: Option<InteractionAutoResolution>,
    pub payload: PendingInteractionPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandStateSummary {
    pub command_id: String,
    pub client_id: String,
    #[serde(rename = "type")]
    pub command_type: String,
    pub state: CommandSummaryState,
    pub at: Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandSummaryState {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "executing")]
    Executing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackgroundWorkKind {
    #[serde(rename = "bash")]
    Bash,
    #[serde(rename = "subagent")]
    Subagent,
    #[serde(rename = "workflow")]
    Workflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackgroundWorkStatus {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "resultPending")]
    ResultPending,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundWorkSummary {
    pub work_id: String,
    pub kind: BackgroundWorkKind,
    pub title: String,
    pub status: BackgroundWorkStatus,
    pub started_at: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
    pub anchor_row_id: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningSubagentSummary {
    pub child_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub subagent_type: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub status: RunningSubagentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Number>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentProjectionState {
    pub revision: u64,
    pub child_session_ids: Vec<String>,
    pub running: Vec<RunningSubagentSummary>,
    pub ended_total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanItemStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "inProgress")]
    InProgress,
    #[serde(rename = "completed")]
    Completed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: String,
    pub content: String,
    pub status: PlanItemStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalIterationState {
    pub iteration: u64,
    pub items: Vec<PlanItem>,
    pub updated_at: Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalStatus {
    #[serde(rename = "active")]
    Active,
    #[serde(rename = "paused")]
    Paused,
    #[serde(rename = "verifying")]
    Verifying,
    #[serde(rename = "verified")]
    Verified,
    #[serde(rename = "notSatisfied")]
    NotSatisfied,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalVerificationOutcome {
    #[serde(rename = "pass")]
    Pass,
    #[serde(rename = "notSatisfied")]
    NotSatisfied,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalVerificationState {
    pub iteration: Number,
    pub outcome: GoalVerificationOutcome,
    pub at: Number,
    pub anchor_row_id: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
}

/// `goalStateSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalState {
    #[serde(default)]
    pub target_id: String,
    pub objective: String,
    #[serde(default, deserialize_with = "deserialize_nullable")]
    pub summary_title: Option<Option<String>>,
    #[serde(default)]
    pub time_used_seconds: u64,
    #[serde(default)]
    pub active_run_started_at_ms: Option<u64>,
    pub status: GoalStatus,
    pub iteration: Number,
    pub verifications: Vec<GoalVerificationState>,
    #[serde(default)]
    pub iterations: Vec<GoalIterationState>,
}

/// `planStateSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanState {
    pub items: Vec<PlanItem>,
    pub updated_at: Number,
}

/// `workspaceHookAdmissionStateSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceHookAdmissionState {
    pub pending_count: Number,
    pub bundle_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
}

/// `rowsWindowSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsWindow {
    /// 尾部窗口，rowId 升序。
    pub window: Vec<ConversationRow>,
    pub total_count: Number,
    /// `nullable()`：键必在。
    pub first_row_id: Option<Number>,
}

/// `conversationSnapshotSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSnapshot {
    pub protocol_version: u64,
    pub session_id: String,
    pub log_epoch: String,
    pub seq: Number,
    pub revision: Number,
    pub control: SessionControl,
    pub availability: SessionActionAvailability,
    pub input_routing: InputRouting,
    #[serde(default)]
    pub meta: SessionMetaState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_context_import: Option<SharedContextImportState>,
    pub config: SessionConfigState,
    #[serde(default, deserialize_with = "deserialize_nullable", skip_serializing_if = "Option::is_none")]
    pub model_transition: Option<Option<SessionModelTransition>>,
    pub usage: SessionUsageState,
    pub queue: QueueState,
    pub pending_interactions: Vec<PendingInteraction>,
    pub pending_commands: Vec<CommandStateSummary>,
    pub background_works: Vec<BackgroundWorkSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<SubagentProjectionState>,
    /// `workflowRunsStateSchema`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_runs: Option<crate::v4_workflow_runs::WorkflowRunsState>,
    pub goal: Option<GoalState>,
    pub plan: Option<PlanState>,
    #[serde(default, deserialize_with = "deserialize_nullable", skip_serializing_if = "Option::is_none")]
    pub workspace_hook_admission: Option<Option<WorkspaceHookAdmissionState>>,
    pub rows: RowsWindow,
}

impl Default for SessionMetaState {
    fn default() -> Self {
        Self {
            title: String::new(),
            title_source: TitleSource::Default,
        }
    }
}
