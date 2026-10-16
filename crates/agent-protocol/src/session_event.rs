//! 会话事件：`session/event` 通知的 `params`，也就是运行时的会话事件信封与载荷。
//!
//! 线上形状（实测，`docs/specs/tauri-agent-runtime.md` §1.2）：
//!
//! ```text
//! {"method":"session/event","params":{"eventId":…,"sessionId":…,"turnId":?,"seq":…,
//!   "traceId":?,"timestamp":…,"deliveryKind":?,"type":"tool.updated","payload":?}}
//! ```
//!
//! `type` 是判别键；`payload` 的类型由 `type` 决定，所以载荷**不在信封里**统一建模，
//! 而是用 [`SessionEvent::payload_as`] 按类型取。
//!
//! 严格度（crate 文档的规则）：
//!
//! - 信封（eventId/sessionId/turnId/seq/traceId/timestamp/deliveryKind）是封闭契约 →
//!   [`SessionEvent`] 带 `deny_unknown_fields`，多一个键就拒绝；
//! - 判别键（`type`、`tool.updated` 的 `kind`、`model.streaming` 的 `kind`）是**分类依据** →
//!   取值为字面量白名单，未知取值报错（比较器按它们分类/排序，不能静默当已知）；
//! - 载荷内部里会长的那部分 → 已知字段声明为类型，其余落进 `extra`，多一个可选字段不会
//!   让整条事件解析失败，但镜像取到的字段类型不符仍然立刻报错。
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::frame::ExtraFields;

/// 会话事件的判别键取值（`ModeSessionEventType` 的镜像）。
///
/// 取值一律写成字面量，不用 `rename_all` 推导：线上是 `turn.started` / `session.titleUpdated`
/// 这类带点和连续大写的取值，命名转换规则会在 `OAuthCallback` → `o-auth-callback` 这种地方
/// 悄悄改字面（M1 踩过一次，见 `crates/protocol/src/lib.rs` 的 `DeepLinkEventKind` 注释）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionEventType {
    #[serde(rename = "session.created")]
    SessionCreated,
    #[serde(rename = "session.resumed")]
    SessionResumed,
    #[serde(rename = "session.updated")]
    SessionUpdated,
    #[serde(rename = "session.titleUpdated")]
    SessionTitleUpdated,
    #[serde(rename = "session.closed")]
    SessionClosed,
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "turn.steerQueued")]
    TurnSteerQueued,
    #[serde(rename = "turn.steerDrained")]
    TurnSteerDrained,
    #[serde(rename = "turn.completed")]
    TurnCompleted,
    #[serde(rename = "turn.failed")]
    TurnFailed,
    #[serde(rename = "message.upserted")]
    MessageUpserted,
    #[serde(rename = "message.removed")]
    MessageRemoved,
    #[serde(rename = "part.started")]
    PartStarted,
    #[serde(rename = "part.delta")]
    PartDelta,
    #[serde(rename = "part.upserted")]
    PartUpserted,
    #[serde(rename = "part.removed")]
    PartRemoved,
    #[serde(rename = "model.streaming")]
    ModelStreaming,
    #[serde(rename = "tool.updated")]
    ToolUpdated,
    #[serde(rename = "permission.requested")]
    PermissionRequested,
    #[serde(rename = "permission.resolved")]
    PermissionResolved,
    #[serde(rename = "userInput.requested")]
    UserInputRequested,
    #[serde(rename = "userInput.resolved")]
    UserInputResolved,
    #[serde(rename = "checkpoint.created")]
    CheckpointCreated,
    #[serde(rename = "rewind.triggered")]
    RewindTriggered,
    #[serde(rename = "streamRecovery.updated")]
    StreamRecoveryUpdated,
}

impl SessionEventType {
    /// 线上取值（日志与报错里用，避免把 Debug 输出当成线上串）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionCreated => "session.created",
            Self::SessionResumed => "session.resumed",
            Self::SessionUpdated => "session.updated",
            Self::SessionTitleUpdated => "session.titleUpdated",
            Self::SessionClosed => "session.closed",
            Self::TurnStarted => "turn.started",
            Self::TurnSteerQueued => "turn.steerQueued",
            Self::TurnSteerDrained => "turn.steerDrained",
            Self::TurnCompleted => "turn.completed",
            Self::TurnFailed => "turn.failed",
            Self::MessageUpserted => "message.upserted",
            Self::MessageRemoved => "message.removed",
            Self::PartStarted => "part.started",
            Self::PartDelta => "part.delta",
            Self::PartUpserted => "part.upserted",
            Self::PartRemoved => "part.removed",
            Self::ModelStreaming => "model.streaming",
            Self::ToolUpdated => "tool.updated",
            Self::PermissionRequested => "permission.requested",
            Self::PermissionResolved => "permission.resolved",
            Self::UserInputRequested => "userInput.requested",
            Self::UserInputResolved => "userInput.resolved",
            Self::CheckpointCreated => "checkpoint.created",
            Self::RewindTriggered => "rewind.triggered",
            Self::StreamRecoveryUpdated => "streamRecovery.updated",
        }
    }
}

/// 事件投递链路：`desktop-continuous`（桌面实时）与 `web-remote-replayable`（远端可重放）。
///
/// 两条链路的语义不同（spec §1.2 与 AGENTS.md 的边界），所以它是事件信封的一等字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeliveryKind {
    #[serde(rename = "desktop-continuous")]
    DesktopContinuous,
    #[serde(rename = "web-remote-replayable")]
    WebRemoteReplayable,
}

/// 会话事件信封（封闭契约）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionEvent {
    pub event_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// 毫秒时间戳（线上是整数；协议声明为非负，这里只能表达成 `i64`）。
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_kind: Option<DeliveryKind>,
    #[serde(rename = "type")]
    pub event_type: SessionEventType,
    /// 载荷按 `type` 解释；缺省表示该类型没有载荷。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

impl SessionEvent {
    /// 按已知类型严格解码载荷（字段名/类型不符即报错）。
    ///
    /// `Ok(None)` = 该事件没有载荷（信封里 `payload` 可缺省）。
    pub fn payload_as<T: serde::de::DeserializeOwned>(
        &self,
    ) -> Result<Option<T>, serde_json::Error> {
        match &self.payload {
            None | Some(Value::Null) => Ok(None),
            Some(payload) => serde_json::from_value(payload.clone()).map(Some),
        }
    }
}

/// `session.titleUpdated` 的 `payload.source`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TitleSource {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "first_input")]
    FirstInput,
    #[serde(rename = "generated")]
    Generated,
    #[serde(rename = "custom")]
    Custom,
}

/// `session.titleUpdated` 的载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTitleUpdatedPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub previous_title: String,
    pub source: TitleSource,
    pub title: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `turn.started` 的载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedPayload {
    /// 线上是**小数**毫秒（实测 `1792124347201.429`），所以是 `f64` 而不是整数时间戳。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_started_at: Option<f64>,
    pub turn_number: u64,
    /// 本轮的用户输入原文。
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_execution_id: Option<String>,
    /// 未建模（M2 不读）：`inputSource` / `inputVisibility` / `executionKind` / `intent` /
    /// `originMeta` / `attachments` / `backgroundSource` 等落在这里。
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `turn.completed` 的 `payload.resultType`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TurnResultType {
    #[serde(rename = "success")]
    Success,
    /// 用户主动中断属于正常结束，复用 `turn.completed` 上报（协议注释如此声明）。
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "error_max_turns")]
    ErrorMaxTurns,
    #[serde(rename = "error_max_budget")]
    ErrorMaxBudget,
    #[serde(rename = "error_during_execution")]
    ErrorDuringExecution,
    #[serde(rename = "error_max_tool_calls")]
    ErrorMaxToolCalls,
}

/// `turn.completed` 的 `payload.cacheStats`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheStats {
    pub total_messages: u64,
    pub cached_messages: u64,
    pub last_cache_hit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `turn.completed` 的载荷（一轮对话的正常终态）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedPayload {
    pub response: String,
    pub token_count: u64,
    /// 未建模：协议的 `z.unknown()`，按原样保留。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    pub tool_call_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_round_count: Option<u64>,
    /// 毫秒（线上是整数；协议声明为非负）。
    pub duration: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_stats: Option<CacheStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    pub result_type: TurnResultType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_subagent_result_consumed: Option<bool>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// 协议里的错误详情（`turn.failed` 与 `tool.updated` 的 error 分支共用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolErrorDetail {
    /// 线上的键是 `type`（Rust 关键字），所以字段名另取、用 `rename` 对齐。
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlying_error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlying_error_detail: Option<String>,
    /// 未建模（M2 不读）：归属信息，原样保留。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `turn.failed` 的载荷（一轮对话的失败终态）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnFailedPayload {
    pub error: ProtocolErrorDetail,
    pub turn_phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_subagent_result_consumed: Option<bool>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `model.streaming` 的 `payload.kind`（**第一层工具入参的所在**）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelStreamingKind {
    #[serde(rename = "start")]
    Start,
    #[serde(rename = "finish")]
    Finish,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "text_start")]
    TextStart,
    #[serde(rename = "text_delta")]
    TextDelta,
    #[serde(rename = "text_end")]
    TextEnd,
    #[serde(rename = "reasoning_start")]
    ReasoningStart,
    #[serde(rename = "reasoning_delta")]
    ReasoningDelta,
    #[serde(rename = "reasoning_end")]
    ReasoningEnd,
    #[serde(rename = "tool_input_start")]
    ToolInputStart,
    #[serde(rename = "tool_input_delta")]
    ToolInputDelta,
    #[serde(rename = "tool_input_end")]
    ToolInputEnd,
    #[serde(rename = "tool_call")]
    ToolCall,
}

/// `model.streaming` 的载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStreamingPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_message_id: Option<String>,
    /// 文本增量；`kind` 是 `tool_input_delta` 时这里是工具入参的 JSON 文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<bool>,
    /// 未建模：协议的 `z.unknown()`。**`kind=tool_call` 时这里是本轮工具调用的入参**，
    /// 第一层（工具调用序列）的入参只能从这里取（调度事件故意不带 input）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    pub kind: ModelStreamingKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_executed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `tool.updated` 各 `kind` 共有的调用身份字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallIdentity {
    pub tool_call_id: String,
    /// 注意：`toolName` **不在**这里——调度事件把它声明为必填、其余 kind 是选填，
    /// 所以它按 kind 各自声明（两组 `flatten` 抢同一个键会变成不确定行为）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ToolCallSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// 工具调用的来源（子代理镜像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolCallSource {
    #[serde(rename = "subagent")]
    Subagent,
}

/// 工具自带的输出对象（协议声明为 `z.record`，**开放对象**）。
///
/// 内部形状是工具自己的事（`Read` 给 content/perf/truncated…，`Bash` 给别的），所以只把比较器
/// 要读的两个字段声明出来，剩下的原样保留。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultObject {
    /// `ok` 的判据（`result.success === true`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    /// 工具输出的正文（第一层「工具输出」层的取值处）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `tool.updated` / `kind=scheduled` 的载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolScheduledPayload {
    #[serde(flatten)]
    pub call: ToolCallIdentity,
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_byte_length: Option<u64>,
    /// 调度事件**故意不带入参**：入参在 `model.streaming/tool_call` 里（spec §1.6）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_omitted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_group_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_run_parallel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<ToolSchedule>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// 调度计划（协议声明为 `z.record`，开放对象）。
///
/// 这两个容器是**行为**：分组数与顺序长度逐字比，元素身份（id）不是（spec §3 的规范化规则）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSchedule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_groups: Option<Vec<Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_order: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `tool.updated` / `kind=started` 的载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStartedPayload {
    #[serde(flatten)]
    pub call: ToolCallIdentity,
    pub started_at: ProtocolInstant,
    /// 线上字段，但 v1 共享 schema 还没声明它（TS 侧的滞后表记着同一件事）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// 同 `readOnly`：线上有、v1 schema 未声明；取值集合尚未观测完整，所以是字符串。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_effect_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `tool.updated` / `kind=result` 的载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultPayload {
    #[serde(flatten)]
    pub call: ToolCallIdentity,
    pub result: ToolResultObject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// 毫秒（线上是整数；协议声明为非负）。
    pub duration: f64,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `tool.updated` / `kind=error` 的载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolErrorPayload {
    #[serde(flatten)]
    pub call: ToolCallIdentity,
    pub error: ProtocolErrorDetail,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `tool.updated` / `kind=batch` 的载荷（一个并行批次的完成汇总）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolBatchPayload {
    pub tool_call_ids: Vec<String>,
    pub success_count: u64,
    pub error_count: u64,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

/// `tool.updated` 的载荷：按 `kind` 判别的联合。
///
/// 只建模 M2 只读切片实际会产生的 5 个 kind（scheduled/started/result/error/batch，实测覆盖）；
/// 协议里还有 `progress`（长跑命令的进度）与 `raw`，它们出现时应当**解析失败**，由后续任务
/// 按夹具补齐——静默接受一个没有镜像的 kind，等于把「未覆盖」伪装成「已覆盖」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ToolUpdatedPayload {
    #[serde(rename = "scheduled")]
    Scheduled(ToolScheduledPayload),
    #[serde(rename = "started")]
    Started(ToolStartedPayload),
    #[serde(rename = "result")]
    Result(ToolResultPayload),
    #[serde(rename = "error")]
    Error(ToolErrorPayload),
    #[serde(rename = "batch")]
    Batch(ToolBatchPayload),
}

/// 协议里的「时间点」：既可能是毫秒数（含小数），也可能是 ISO 串。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProtocolInstant {
    Ms(f64),
    Iso(String),
}
