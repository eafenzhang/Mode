//! `agent-core`：M2 会话运行时的 Rust 侧骨架。
//!
//! 规格：`docs/specs/tauri-agent-runtime.md`（oracle 契约、三层对拍口径、合法差异白名单）。
//! 纪律：`docs/specs/clean-room-policy.md`——本 crate 只按 spec、**允许输入的声明面**与
//! oracle 的黑盒观察取材，不读旧运行时的实现体。
//!
//! # 镜像的是哪个协议面
//!
//! **stdio 会话面**：方法名信封与 `session/event` 上承载的会话事件（`session/*`、
//! `interaction/*`、`startup/*`），即 Task 1 在 `crates/agent-protocol` 落下的那套镜像。
//! **不是** `packages/shared/src/mode-protocol-v4/**` 的主题帧——v4 是 host（web / 远端）面，
//! stdio 运行时不说它（依据见 `crates/agent-protocol/src/lib.rs` 的「镜像的是哪个协议面」一节）。
//! 事件类型（[`SessionEvent`] / [`SessionEventType`]）直接复用该镜像，本 crate 不另立一套。
//!
//! # 演进顺序
//!
//! 1. ~~Task 0 只落占位~~：占位（`SessionEventKind` / `Runtime` trait）已在 Task 2 删掉——
//!    Task 1 的协议镜像已落地，留着它就是同一个 crate 里两套并行的事件模型
//!    （AGENTS.md：「避免重复状态和多条写入路径」）。
//! 2. **Task 2（本步）**：接上工具注册表（[`tool`]）与最小会话循环的工具面（[`session`]）。
//! 3. Task 3–4 补写工具与 shell；Task 5 把会话循环做成流式并补双链路重放
//!    （`turn.completed` / `turn.failed` 的载荷需要模型层数字：`tokenCount`/`usage`/`cacheStats`，
//!    本任务没有这些数据，所以**不产出**终态事件，见 [`session`]）。
//! 4. Task 6 落权限引擎（按 `ModeToolFamily` 分档的规则表）。

pub mod approval;
pub mod cancel;
pub mod command_inbox;
pub mod compact;
pub mod context;
pub mod events;
pub mod goal;
pub mod permission;
pub mod plugin_tools;
pub mod provider;
pub mod recovery;
pub mod rule_store;
pub mod session;
pub mod stream;
pub mod tool;

pub use agent_protocol::session_event::{
    DeliveryKind, SessionEvent, SessionEventType, ToolBatchPayload, ToolCallIdentity,
    ToolErrorPayload, ToolResultObject, ToolScheduledPayload, ToolStartedPayload,
    ToolUpdatedPayload,
};
pub use agent_tools::{
    FileState, ModeToolFamily, TaskNotification, TaskRegistry, ToolContext, ToolError, ToolOutcome,
    ToolSpec,
};
pub use cancel::CancelToken;
pub use command_inbox::{Admission, CommandInbox, Completion};
pub use compact::{
    compact_prompt, summary_continuation, validate_compact_response, CompactResponseError,
    SUMMARY_TITLE,
};
pub use context::{
    build_model_request, build_system_prompt, estimate_history_tokens, trim_messages,
    ContextBudget, ContextCacheUsage, ContextCost, ContextInput, ContextUsage, MemorySection,
    TrimmedHistory,
};
pub use events::EventPublisher;
pub use goal::{
    continuation_prompt, continuation_stop_reason, parse_verifier_response, verifier_prompt,
    GoalStatus, GoalStopReason, GoalTarget, VerifierVerdict, CANCELLED_VERIFIER_REASON,
    CONTINUATION_MARKER, INVALID_VERIFIER_REASON, VERIFIER_MARKER,
};
pub use permission::{
    evaluate as evaluate_permission, ChannelPermissionHandler, Decision as PermissionDecision,
    FramePermissionHandler, PermissionHandler,
};
pub use plugin_tools::{PluginToolDeclaration, PluginToolError, PluginToolSource};
pub use provider::{
    ModelCapabilities, ModelDelta, ModelMessage, ModelProvider, ModelRequest, ModelResponse,
    ModelToolCall, ModelToolDefinition, ModelUsage, ProviderError, ProviderErrorKind, ScriptStep,
    ScriptToolCall, ScriptedProvider,
};
pub use recovery::RetryPolicy;
pub use rule_store::{scope_id_for, RuleStore, SqliteRuleStore};
pub use session::TurnTerminalFacts;
pub use stream::{EventSink, TurnError, TurnProgress};
