//! `session/read` 的状态快照镜像（`modeSessionStateSnapshotSchema`）。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的 `modeSessionStateSnapshotSchema`
//! 及其子 schema（settings / projection / runtime / goalStats / todos / slashCommands），
//! 以及 `mode-protocol-legacy-types.ts` 的 runtime/context 形状。规范见
//! `docs/specs/session-read-group.md`。
//!
//! 所有 schema 都是 `.strict()`；判别联合同样用 `#[serde(untagged)]` + 分支严格。
//! 浮点字段（`amount` / hit rate）用 `serde_json::Number`，避免整数被写成 `0.0`。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::message::{MessageModel, MessageWithParts};
use crate::session_read::{deserialize_nullable, SessionGoal, SessionInfo, SessionMode, SessionStatus};

type JsonObject = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelInputFormat {
    pub supports_text: bool,
    pub supports_image: bool,
    pub supports_video: bool,
    pub supports_audio: bool,
    pub supports_pdf: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelOutputFormat {
    pub supports_text: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelProperties {
    pub input_format: ModelInputFormat,
    pub output_format: ModelOutputFormat,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThoughtLevelOption {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelReasoningOptions {
    pub levels: Vec<ThoughtLevelOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_level: Option<String>,
}

/// `modeModelOptionSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelOption {
    #[serde(rename = "ref")]
    pub model: MessageModel,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ModelReasoningOptions>,
    pub properties: ModelProperties,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionModelSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<MessageModel>,
    pub available: Vec<ModelOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<MessageModel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThoughtLevelSettings {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_level: Option<String>,
    pub available: Vec<ThoughtLevelOption>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionModeSettings {
    pub current: SessionMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SessionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSettings {
    pub model: SessionModelSettings,
    pub thought_level: ThoughtLevelSettings,
    pub mode: SessionModeSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<PermissionSettings>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionRuleValue {
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PermissionRuleBehavior {
    #[serde(rename = "allow")]
    Allow,
    #[serde(rename = "deny")]
    Deny,
    #[serde(rename = "ask")]
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PermissionUpdateType {
    #[serde(rename = "addRules")]
    AddRules,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionUpdate {
    #[serde(rename = "type")]
    pub update_type: PermissionUpdateType,
    pub behavior: PermissionRuleBehavior,
    pub rules: Vec<PermissionRuleValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PermissionDecision {
    #[serde(rename = "allow")]
    Allow,
    #[serde(rename = "deny")]
    Deny,
    #[serde(rename = "escalate")]
    Escalate,
    #[serde(rename = "modify")]
    Modify,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionResponse {
    pub decision: PermissionDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub modified_input: Option<Option<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_updates: Option<Vec<PermissionUpdate>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionOption {
    pub option_id: String,
    /// 协议里是开放字符串（不是判别键），保持原文。
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub response: PermissionResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PermissionRiskLevel {
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "high")]
    High,
    #[serde(rename = "critical")]
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SubagentOriginKind {
    #[serde(rename = "subagent")]
    Subagent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InteractionOrigin {
    pub kind: SubagentOriginKind,
    pub agent_id: String,
    pub agent_type: String,
    pub child_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parent_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingPermission {
    pub request_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub reason: String,
    pub risk_level: PermissionRiskLevel,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub input: Option<Option<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<InteractionOrigin>,
    pub options: Vec<PermissionOption>,
    pub requested_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ActiveToolCallStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "denied")]
    Denied,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ActiveToolCallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ErrorPhase {
    #[serde(rename = "prepare")]
    Prepare,
    #[serde(rename = "configuration")]
    Configuration,
    #[serde(rename = "connect")]
    Connect,
    #[serde(rename = "response")]
    Response,
    #[serde(rename = "stream")]
    Stream,
    #[serde(rename = "parse")]
    Parse,
    #[serde(rename = "validation")]
    Validation,
    #[serde(rename = "unhandled")]
    Unhandled,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ExceptionKind {
    #[serde(rename = "api_call")]
    ApiCall,
    #[serde(rename = "generic")]
    Generic,
    #[serde(rename = "protocol")]
    Protocol,
    #[serde(rename = "provider_business")]
    ProviderBusiness,
    #[serde(rename = "transport")]
    Transport,
    #[serde(rename = "type_error")]
    TypeError,
    #[serde(rename = "validation")]
    Validation,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ErrorTransport {
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "sse")]
    Sse,
    #[serde(rename = "websocket")]
    Websocket,
}

/// `errorAttributionSchema`（全部可选，无 null）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorAttribution {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ErrorSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_phase: Option<ErrorPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception_kind: Option<ExceptionKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<ErrorTransport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionLastError {
    #[serde(rename = "type")]
    pub error_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<ErrorAttribution>,
}

/// `modeSessionProjectionSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionProjection {
    pub session_id: String,
    pub status: SessionStatus,
    pub mode: SessionMode,
    pub turn_count: u64,
    pub total_token_count: u64,
    pub context_used: u64,
    pub context_window: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn_id: Option<String>,
    pub pending_permissions: Vec<PendingPermission>,
    pub active_tool_calls: Vec<ActiveToolCall>,
    pub background_jobs: Vec<JsonObject>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub target: Option<Option<SessionGoal>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<SessionLastError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ApiRetryKind {
    #[serde(rename = "api_retry")]
    ApiRetry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiRetryStatus {
    pub kind: ApiRetryKind,
    pub attempt: u64,
    pub max_retries: u64,
    pub retry_delay_ms: u64,
    /// 键必在，值可 null。
    pub error_status: Option<u64>,
    pub error: String,
}

/// `modeSessionContextCacheUsageSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextCacheUsage {
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub latest_hit_rate: Option<Option<Number>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_rate_request_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cache_write_tokens: Option<u64>,
    /// 键必在，值可 null。
    pub hit_rate: Option<Number>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BreakdownSource {
    #[serde(rename = "system_prompt")]
    SystemPrompt,
    #[serde(rename = "meta_user_context")]
    MetaUserContext,
    #[serde(rename = "skills")]
    Skills,
    #[serde(rename = "tool_prompt")]
    ToolPrompt,
    #[serde(rename = "system_tool_schemas")]
    SystemToolSchemas,
    #[serde(rename = "mcp_tool_schemas")]
    McpToolSchemas,
    #[serde(rename = "messages")]
    Messages,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BreakdownItem {
    pub source: BreakdownSource,
    pub chars: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextCost {
    pub amount: Number,
    pub currency: String,
}

/// `modeSessionContextUsageSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionContextUsage {
    pub used: u64,
    pub size: u64,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub cost: Option<Option<ContextCost>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<ContextCacheUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakdown: Option<Vec<BreakdownItem>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalVerification {
    pub passed: bool,
    pub reason: String,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub next_action: Option<Option<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GoalVerificationStatus {
    #[serde(rename = "started")]
    Started,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed_closed")]
    FailedClosed,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SyntheticKind {
    #[serde(rename = "synthetic")]
    Synthetic,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GoalVerificationType {
    #[serde(rename = "goal_verification")]
    GoalVerification,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SeparatorDisplay {
    #[serde(rename = "separator")]
    Separator,
}

/// `modeSessionGoalVerificationTimelineSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GoalVerificationTimeline {
    pub version: u64,
    pub kind: SyntheticKind,
    #[serde(rename = "type")]
    pub timeline_type: GoalVerificationType,
    pub display: SeparatorDisplay,
    pub target_id: String,
    pub verification_id: String,
    pub status: GoalVerificationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<GoalVerification>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_iteration: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_assistant_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ActiveTurnKind {
    #[serde(rename = "regular")]
    Regular,
    #[serde(rename = "compact")]
    Compact,
    #[serde(rename = "rewind")]
    Rewind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryKind {
    #[serde(rename = "desktop-continuous")]
    DesktopContinuous,
    #[serde(rename = "web-remote-replayable")]
    WebRemoteReplayable,
}

/// `modeSessionRuntimeStateSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionRuntimeState {
    pub event_seq: u64,
    pub state_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_kind: Option<DeliveryKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_turn_kind: Option<ActiveTurnKind>,
    pub pending_request_ids: Vec<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub api_retry: Option<Option<ApiRetryStatus>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<SessionContextUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_verifications: Option<Vec<GoalVerification>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_verification_timeline: Option<Vec<GoalVerificationTimeline>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionGoalStats {
    pub time_used_seconds: u64,
    pub tokens_used: u64,
    pub token_budget: Option<u64>,
    pub context_used: u64,
    pub context_window: u64,
    pub tool_call_count: u64,
    pub iteration_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TodoStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "in_progress")]
    InProgress,
    #[serde(rename = "completed")]
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TodoPriority {
    #[serde(rename = "high")]
    High,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "low")]
    Low,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TodoGroupSource {
    #[serde(rename = "goal_iteration")]
    GoalIteration,
    #[serde(rename = "session")]
    Session,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoGroup {
    pub id: String,
    pub source: TodoGroupSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_iteration: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    pub todos: Vec<TodoItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SlashCommandSource {
    #[serde(rename = "builtin")]
    Builtin,
    #[serde(rename = "custom")]
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SlashCommandSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ProtocolName {
    #[serde(rename = "Mode Protocol")]
    ModeProtocol,
    #[serde(rename = "ZCode Protocol")]
    ZCodeProtocol,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolInfo {
    pub name: ProtocolName,
    pub version: u64,
}

/// `modeSessionStateSnapshotSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionStateSnapshot {
    pub protocol: ProtocolInfo,
    pub session: SessionInfo,
    pub settings: SessionSettings,
    pub projection: SessionProjection,
    pub runtime: SessionRuntimeState,
    pub messages: Vec<MessageWithParts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_stats: Option<SessionGoalStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todos: Option<Vec<TodoItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo_groups: Option<Vec<TodoGroup>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slash_commands: Option<Vec<SlashCommand>>,
}
