//! 会话运行时（LT-1 C1 Task 6/7/8）：per-session 的 turn 串行执行、事件总线与持久化。
//!
//! # 所有权与事件顺序
//!
//! ```text
//! submit() ──(std Mutex<Live>：幂等/revision/CommandInbox 准入)──> UnboundedSender
//!                                                                    │
//! per-session worker（tokio task，持有 Session/Arc<SessionStore>/Arc<dyn ProviderSource>）
//!     ├─ 物化后发一次 session/requestRuntimePreferences（Task 8，应答/回退缓存进 Shared）
//!     └─ 逐条消费队列 ──(读 Shared.config：mode/model/能力)──> spawn_blocking + block_on：
//!         Session::run_turn（审批 handler 的反向请求等待在阻塞线程上，不占 tokio worker）
//!                                                   └─ TurnRecorder: EventSink
//!                                                       ├─ 先观察落库再 publish（Task 8 起倒转）：
//!                                                       │   Shared::publish：内存事件日志（有界）
//!                                                       │   + broadcast 总线（同一临界区内）
//!                                                       └─ 观察事件 → session-store 落库
//!                                                           （turn.started → user message+part；
//!                                                            tool batch → assistant 消息+parts+tool_usage；
//!                                                            终态 → turn_usage + touch session）
//! ```
//!
//! - **串行**：一个 worker 只有一个；turn 运行期间 `submit` 只入队（`CommandInbox`
//!   记录 running/queued，`Accepted.admission` 供调用方观察）。
//! - **事件**：`seq` 由 `agent-core::Session` 单调分配；publish 与 subscribe 共用同一把
//!   `std Mutex`，先读日志再建 broadcast receiver，因此 replay→live 无重复、无缺口。
//! - **持久化**（控制器裁决 3/5）：只在**完成边界**写行——user message 在
//!   `turn.started` 原子写入（message+part 同一事务），assistant 消息在其步骤完成
//!   （`tool.updated/batch` / `turn.completed`）时原子写入；失败/取消不写半成品行，
//!   已提交的行保留。`session_input` 在 v1 immediate 路径**不写行**（Task 0 观察，
//!   控制器裁决 5）。
//! - **终态**：成功/取消的终态由 `run_turn` 发（`turn.completed`，取消为
//!   `resultType:"cancelled"`）；provider 错误的 `turn.failed` 由 worker 补发（
//!   [`Session::emit_turn_failed`]，seq 归 Session 分配）。每个 turn 恰好一条终态。
//! - **配置失败**（Task 7 控制器裁决 7）：会话没有可用模型（或 provider 解析失败）时，
//!   turn 以 `turn.failed`（`CONFIGURATION_ERROR`）结束，**不用默认模型兜底**。
//! - **取消**（控制器裁决 4）：`cancel_turn()` 只作用于**当前** turn 的
//!   [`CancelToken`]；排队中的输入照常执行（含 close 后的收尾 drain——已接受的
//!   输入由 CommandInbox 串行准入，不中途丢弃）。
//! - **阻塞 IO 与审批等待**（Task 8）：`SessionStore` 内部是 `Mutex<Connection>` +
//!   短 `BEGIN IMMEDIATE` 事务（控制器裁决 2 的「短临界区」路线）。turn 跑在
//!   `spawn_blocking` 线程上：同步 [`agent_core::PermissionHandler`] 的反向请求等待
//!   会阻塞当前线程，不能占 tokio worker（否则与 stdio 单写者互等死锁）；provider
//!   解析（账号型凭据钩子）同理。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::command_inbox::{CommandInbox, Completion};
use agent_core::compact::{compact_prompt, summary_continuation, validate_compact_response};
use agent_core::context::{ContextCacheUsage, ContextUsage};
use agent_core::goal::{
    continuation_prompt, continuation_stop_reason, parse_verifier_response, verifier_prompt,
    GoalStatus, GoalTarget, CANCELLED_VERIFIER_REASON,
};
use agent_core::provider::{ModelDelta, ModelMessage, ProviderError};
use agent_core::session::Session;
use agent_core::session::TurnTerminalFacts;
use agent_core::{
    estimate_history_tokens, CancelToken, EventSink, ModelProvider, ModelUsage, SessionEvent,
    SessionEventType, SUMMARY_TITLE,
};
use agent_protocol::session_event::{
    CacheStats, DeliveryKind, ModelStreamingKind, ModelStreamingPayload, ProtocolErrorDetail,
    ToolUpdatedPayload, TurnCompletedPayload, TurnFailedPayload, TurnResultType,
};
use agent_protocol::session_read::{SessionGoal, SessionGoalStatus};
use agent_protocol::session_write::{SessionCompactParams, SessionGoalAction, SessionGoalParams};
use agent_protocol::snapshot::{
    GoalVerification, GoalVerificationStatus, GoalVerificationTimeline, GoalVerificationType,
    SeparatorDisplay, SessionGoalStats, SessionStateSnapshot, SyntheticKind,
};
use agent_protocol::ExtraFields;
use serde_json::{json, Value};
use session_store::{
    MessageRecord, MessageWithParts, ModelUsageRecord, PartRecord, SessionEntryRecord,
    SessionRecord, SessionStore, TargetRecord, ToolUsageRecord, TurnUsageRecord,
};

use crate::reverse::{RuntimePreferences, RuntimeWiring};
use crate::state::{
    build_snapshot, fresh_id, Accepted, LiveView, RuntimeConfig, RuntimeError, RuntimeStatus,
    SessionInput, TurnKind,
};

/// provider 装配的会话上下文（Task 8）：账号型 provider 的宿主钩子
/// （`interaction/requestProviderRuntimeHeaders`）需要会话与工作区事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRequestContext {
    pub session_id: String,
    pub workspace_path: String,
    /// workspace identity（`WorkspaceRef` 的 identity 字段；缺省 `None`）。
    pub workspace_identity: Option<String>,
}

/// provider 装配源：按会话当前的模型事实解析出可执行的 [`ModelProvider`]。
///
/// 生产实现走 `ProviderConfigHub`（builtin/personal/account 三层合并 →
/// `OpenAiChatProvider`）；测试注入脚本化 fake。错误文案只带 provider/model 与
/// 缺失字段名（`ProviderConfigError` 的掩码纪律），不含凭据。
///
/// `context` 缺省 `None`：账号型 provider 在没有会话上下文时不发起宿主钩子，
/// 直接按「缺 `access.apiKey`」显式失败。**可能阻塞**（等待宿主反向请求应答）：
/// worker 把解析放在 `spawn_blocking` 上跑。
pub trait ProviderSource: Send + Sync {
    fn resolve(
        &self,
        provider_id: &str,
        model_id: &str,
        context: Option<&ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String>;

    /// 装配源配置代际（provider 缓存键的一部分）。默认空串 = 配置不会变（测试注入的
    /// 固定 provider）；生产实现返回 hub revision + 配置文件指纹（最终评审 I6）。
    fn cache_key(&self) -> String {
        String::new()
    }
}

/// worker 队列的一项：已接受、等待串行执行的输入。
struct QueuedInput {
    input: SessionInput,
    input_id: String,
    /// `Some` = compact 任务（`input.content` 是 `"/compact …"` 展示串；
    /// prompt 任务恒 `None`）。
    compact: Option<CompactJob>,
    /// `Some` = goal 任务（控制 turn / 续跑循环 / 仅事件）。
    goal: Option<GoalJob>,
    /// 非空 = 运行时生成的后台通知 turn（C3 T3）。这类 turn 不进 `CommandInbox`，
    /// 其 `input` 是已拼好的 `<task-notification>` 文本（多条空行分隔）。
    notifications: Vec<agent_core::TaskNotification>,
}

/// 一个已受理的 goal 任务（进入 worker 队列的事实）。
#[derive(Debug, Clone)]
struct GoalJob {
    /// 生效 inputId（权限/幂等/run_started.activeInputId）。
    input_id: String,
}

/// 目标续跑的热状态。唯一事实源是 `session_target` 行；这里的字段是同一把
/// `Live` 锁下的同步副本，用于快照与判定，每次变更都先落库。
#[derive(Debug, Clone, Default)]
struct GoalState {
    target: Option<GoalTarget>,
    /// 完整验证时间线（`runtime.goalVerificationTimeline`）。
    timeline: Vec<GoalVerificationTimeline>,
    /// 最近验证窗口（观察 `length:2`）。
    verifications: Vec<GoalVerification>,
    /// 达到 verifier 的轮次（每目标从 1 起；pre-request 取消不增加）。
    iteration: u64,
    /// 会话级 goal-continuation 尝试序号（`runtime_command_<n>`，跨目标累计）。
    continuation_ordinal: u64,
    /// 控制 turn 计数（0 起；`turn.started.turnNumber` 的基数）。
    control_turns: u64,
    /// 正在进行/已受理的 goal 任务（运行中限制的依据）。
    job_active: bool,
    /// pause/stop 已预分配、等待 worker 在终态/verifier 取消前发布的 status 事件。
    /// 队列而不是单槽：连续 pause/stop 也要保持 seq 单调且不丢事件。
    pending_cancel_events: std::collections::VecDeque<SessionEvent>,
    /// 无 goal / 首次 set 的 titleUpdated 已发过。
    title_emitted: bool,
}

impl GoalState {
    /// 冷恢复：`active` 目标转 `paused`，并清空 active run 字段（观察 §1.8）。
    fn restore(target: GoalTarget, timeline: Vec<GoalVerificationTimeline>) -> Self {
        let mut target = target;
        let clear_active = target.status != GoalStatus::Complete
            && (target.active_input_id.is_some()
                || target.active_run_started_at.is_some()
                || target.active_run_last_seen_at.is_some());
        if target.status == GoalStatus::Active {
            target.status = GoalStatus::Paused;
        }
        if target.status != GoalStatus::Active || clear_active {
            target.active_input_id = None;
            target.active_run_started_at = None;
            target.active_run_last_seen_at = None;
        }
        let verifications = recent_verifications(&timeline);
        Self {
            target: Some(target),
            timeline,
            verifications,
            iteration: 0,
            continuation_ordinal: 0,
            control_turns: 0,
            job_active: false,
            pending_cancel_events: std::collections::VecDeque::new(),
            title_emitted: false,
        }
    }
}

/// `session/goal` 的受理结果（应答三键）。
pub(crate) struct GoalOutcome {
    pub response: String,
    pub snapshot: SessionStateSnapshot,
    pub started_turn: bool,
}

/// `session/goal` 的显式拒绝面（JSON-RPC 错误，零事件零落库）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GoalReject {
    RevisionConflict {
        expected: u64,
        current: u64,
    },
    /// `-32010`：有 prompt/continuation 在跑（`pause` 例外）。
    PromptRunning,
    Store(String),
    RuntimeClosed,
}

/// 一个已受理的 compact 任务（进入 worker 队列的事实）。
#[derive(Debug, Clone)]
struct CompactJob {
    /// 生效 inputId（回显 / `turn.started.inputId` / `sourceCommandId`）。
    input_id: String,
    /// 用户附加指令（`None` → `turn.started.input` 是 `"/compact"`）。
    instructions: Option<String>,
    /// 受理时的已结算 compact 数：执行时该值更小 → 排队期间已有 compact 结算 →
    /// 本次按观察到的 `skipped` 形状执行（`agent-c2-contract.md` §1.6 trial 4）。
    settled_at_enqueue: u64,
}

/// compact 协调器的执行相位（per-session 单飞的状态面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CompactPhase {
    /// 没有已受理的 compact。
    #[default]
    Idle,
    /// 已受理、尚未进入执行（可能有多条排队）。
    Queued,
    /// 正在执行（此时第二个请求返回 `already_running`）。
    Running,
}

/// per-session compact 协调器：单飞受理 + 两种观察形状的状态机。
///
/// 状态迁移（`agent-c2-contract.md` §1.6；时序依赖见契约「Rust C2 现状」注记）：
///
/// ```text
/// Idle ──accept──> Queued ──run_compact 入口──> Running ──终态──> Queued(还有排队) / Idle
///   ↑                  ↑
///   └── already_running 只在 Running；accept 只在 Idle/Queued（排队中的第二单受理后
///       最终以 skipped 终态执行——两种形状都来自这一个状态机）
/// ```
#[derive(Debug, Default)]
struct CompactCoordinator {
    phase: CompactPhase,
    /// 已受理但尚未进入执行的任务数（`Running` 期间 accept 被拒，不增）。
    pending: usize,
    /// 已结算的 compact 数（成功 / skipped 计数；失败与配置失败不计——失败不改变
    /// 上下文，排队中的下一单仍应完整执行）。
    settled_ok: u64,
    /// 执行中的 compact turn id（快照 `activeTurnId` 的事实源）。
    running_turn_id: Option<String>,
}

impl CompactCoordinator {
    /// 受理一单（只在 `Idle`/`Queued` 调用）。
    fn on_accept(&mut self) {
        self.pending += 1;
        self.phase = CompactPhase::Queued;
    }

    /// worker 取到任务（尚未进入执行；provider 装配窗口内相位仍是 `Queued`）。
    fn on_pickup(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }

    /// `run_compact` 入口：进入执行（`already_running` 的判定点）。
    fn on_started(&mut self, turn_id: &str) {
        self.phase = CompactPhase::Running;
        self.running_turn_id = Some(turn_id.to_string());
    }

    /// 任务终态：`settled=true`（成功 / skipped）时结算计数；相位回 `Queued`/`Idle`。
    fn on_settled(&mut self, settled: bool) {
        if settled {
            self.settled_ok += 1;
        }
        self.running_turn_id = None;
        self.phase = if self.pending > 0 {
            CompactPhase::Queued
        } else {
            CompactPhase::Idle
        };
    }

    fn is_running(&self) -> bool {
        self.phase == CompactPhase::Running
    }

    /// 排队期间是否已有 compact 结算（→ 本次执行走 `skipped` 形状）。
    fn skip_due(&self, settled_at_enqueue: u64) -> bool {
        settled_at_enqueue < self.settled_ok
    }
}

/// `session/compact` 受理结果（应答 `compact.state` 的两支）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompactAccept {
    /// 已受理：进入 worker 队列，异步执行（新 revision 由应答快照带出）。
    Accepted { input_id: String },
    /// 已有 compact 在执行：不入队、不动 revision（观察：trial 1–3）。
    AlreadyRunning { input_id: String },
}

/// `session/compact` 的显式拒绝面（JSON-RPC 错误，零事件零落库）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompactReject {
    /// `-32009` revision 冲突（`data` 带 `actualRevision`/`expectedRevision`）。
    RevisionConflict { expected: u64, current: u64 },
    /// `-32010` 有 prompt turn 正在执行。
    PromptRunning,
    /// worker 队列已关闭。
    RuntimeClosed,
}

/// `"/compact"` 展示串（`turn.started.input` 的观察形状，契约 §1.1）。
fn compact_display(instructions: Option<&str>) -> String {
    match instructions {
        Some(instructions) => format!("/compact {instructions}"),
        None => "/compact".to_string(),
    }
}

/// 一次模型请求的用量快照（`used`/`cache` 的事实面）。
///
/// `total_tokens` 以 provider 的 total 为准；从 DB 恢复时直接取消息行
/// `tokens.total`（oracle 的合成 timeline 消息没有这个键，不能算作请求）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct RequestUsage {
    input_tokens: u64,
    total_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
}

impl RequestUsage {
    fn from_model_usage(usage: &ModelUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            total_tokens: usage.input_tokens + usage.output_tokens,
            cache_read_tokens: usage.cache_read_tokens,
            cache_write_tokens: usage.cache_write_tokens,
        }
    }
}

/// 会话跨 turn 的模型请求用量累计（`runtime.contextUsage` 的唯一事实源）。
///
/// 运行期由 [`TurnRecorder::model_usage`] 逐个请求写入；冷物化时从已持久化的
/// assistant 消息 `tokens` 重建（T0：resume 后投影计数归 0，但 contextUsage 保留）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct UsageAccounting {
    last_request: Option<RequestUsage>,
    request_count: u64,
    total_input_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_write_tokens: u64,
}

impl UsageAccounting {
    fn observe(&mut self, usage: &RequestUsage) {
        self.last_request = Some(*usage);
        self.request_count += 1;
        self.total_input_tokens += usage.input_tokens;
        self.total_cache_read_tokens += usage.cache_read_tokens;
        self.total_cache_write_tokens += usage.cache_write_tokens;
    }

    /// 从 `message` 行恢复：每条带 `tokens.total` 的 assistant 消息对应一次完成的
    /// 模型请求（C1 的 `persist_assistant_step` 逐请求写 tokens）。
    fn from_messages(messages: &[MessageWithParts]) -> Self {
        let mut accounting = Self::default();
        for row in messages {
            let data = &row.message.data;
            if data.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(tokens) = data.get("tokens") else {
                continue;
            };
            // 合成 timeline 消息的 tokens 全 0 且没有 total，不是模型请求。
            let Some(total) = tokens.get("total").and_then(Value::as_u64) else {
                continue;
            };
            accounting.observe(&RequestUsage {
                input_tokens: tokens.get("input").and_then(Value::as_u64).unwrap_or(0),
                total_tokens: total,
                cache_read_tokens: tokens
                    .get("cache")
                    .and_then(|cache| cache.get("read"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cache_write_tokens: tokens
                    .get("cache")
                    .and_then(|cache| cache.get("write"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            });
        }
        accounting
    }

    /// 投影成 `ContextUsage`；没有任何模型请求时为 `None`（create 快照没有该键）。
    ///
    /// `size` 未知时为 0（不编默认窗口）；`cost` 没有 provider 事实，保持 `None`。
    fn context_usage(&self, context_window: Option<u64>) -> Option<ContextUsage> {
        let last = self.last_request?;
        Some(ContextUsage {
            used: last.total_tokens,
            size: context_window.unwrap_or(0),
            cache: Some(ContextCacheUsage {
                input_tokens: last.input_tokens,
                cache_read_tokens: last.cache_read_tokens,
                cache_write_tokens: last.cache_write_tokens,
                hit_rate_request_count: self.request_count,
                total_input_tokens: self.total_input_tokens,
                total_cache_read_tokens: self.total_cache_read_tokens,
                total_cache_write_tokens: self.total_cache_write_tokens,
            }),
            cost: None,
        })
    }
}

/// 锁内的活跃状态。所有字段只有一个写入方（submit / worker），读方加锁短读。
struct Live {
    status: RuntimeStatus,
    /// 乐观并发版本：每次接受输入 +1，每个 turn 终态 +1，setter 成功 +1。
    revision: u64,
    updated_at: i64,
    last_error: Option<String>,
    turn_count: u64,
    total_token_count: u64,
    /// 本 runtime 生命周期内最近一次模型请求的 total（`projection.contextUsed`；
    /// T0 观察 22 且 resume 归 0，与 `totalTokenCount` 的累计口径是两个事实）。
    context_used: u64,
    /// 本 runtime 生命周期内最近一次已开始的 turn（`turn.started` 写入，终态不清；
    /// 观察的 read 视图在 turn 后仍带 `currentTurnId`）。
    current_turn_id: Option<String>,
    /// 下一条持久化 message 行的 `sequence`（冷启动时按库内已有行的最大序号 +1 初始化，
    /// Task 7 控制器裁决 6：恢复历史时不得与旧行撞序号）。
    next_message_sequence: i64,
    /// 已接受的 inputId → 回执（幂等依据；进程内有效）。
    seen_inputs: HashMap<String, Accepted>,
    /// running / FIFO 队列的准入账本（`agent-core::CommandInbox`，key = 会话 id）。
    inbox: CommandInbox,
    /// 当前运行中 turn 的取消令牌；`None` = 空闲。
    current_cancel: Option<CancelToken>,
    /// worker 正在执行的输入种类（`None` = 取到输入前/终态后的窗口）。
    current_kind: Option<TurnKind>,
    /// per-session compact 协调器（C2 T2 单飞受理与 `already_running`/`skipped` 状态机）。
    compact: CompactCoordinator,
    /// `runtime.contextUsage` 的累计事实（最近一次请求 + 会话请求总数/缓存合计）。
    usage: UsageAccounting,
    /// goal 热状态（唯一事实源是 `session_target` 行）。
    goal: GoalState,
    /// 当前 goal 任务的取消令牌（pause/stop/close 命中；队列等待期也有效）。
    goal_cancel: Option<CancelToken>,
}

/// 一个 turn 内重试事件的追踪 id（宿主生成；夹具折叠为 `<id>`）。
#[derive(Debug, Clone)]
struct RetryTrace {
    query_id: String,
    operation_id: String,
    logical_call_id: String,
    span_id: String,
    parent_span_id: String,
}

impl RetryTrace {
    fn new() -> Self {
        Self {
            query_id: uuid::Uuid::new_v4().to_string(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            logical_call_id: uuid::Uuid::new_v4().to_string(),
            span_id: uuid::Uuid::new_v4().to_string(),
            parent_span_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

/// 跨 submit/worker/snapshot 共享的状态 + 事件总线。
///
/// `config` 用 `RwLock`：setter（Task 7 方法层）原位更新，worker 在每个 turn 开始前
/// 读取（mode/能力进系统提示，模型事实决定 provider 装配），快照读取最新事实。
/// `preferences` 是宿主对 `session/requestRuntimePreferences` 的缓存应答（每运行时
/// 一次，Task 8 控制器裁决 3；取到前为 `None`）。`drained` 是 worker 的退出信号
/// （队列清空且发送端关闭后置 true）：`session/close` 的 closing 台账靠它感知
/// drain 完成（Task 7 复评裁决 2）。
struct Shared {
    live: Mutex<Live>,
    config: RwLock<RuntimeConfig>,
    /// 事件 seq 分配 + 日志/broadcast 的唯一所有者；与 `Session` 共享同一个 Arc。
    events: Arc<agent_core::EventPublisher>,
    drained: tokio::sync::watch::Sender<bool>,
    record: SessionRecord,
    /// 会话库句柄：goal 命令（方法层）与 worker 共用同一事实源。
    store: Arc<SessionStore>,
    preferences: RwLock<Option<RuntimePreferences>>,
    /// C3 T2：会话级 apiRetry 状态（`mode-api-retry` 是规则唯一所有者）。
    ///
    /// 由 [`Shared::publish`] 在提交 `session.updated` / `streamRecovery.updated`
    /// 时喂入；快照投影用 `with_api_retry_runtime`（不复制归一化规则）。
    api_retry: mode_api_retry::SessionApiRetryTracker,
    /// 每个 turn 的重试事件追踪 id（queryId/modelCall 的 operationId/logicalCallId）。
    /// 事件载荷由 agent-core 产出，宿主只补本层持有的会话/配置事实；同一 turn 的
    /// 多个重试事件共享这些 id（夹具把具体值折叠成 `<id>`）。
    retry_traces: Mutex<HashMap<String, RetryTrace>>,
    /// 最近一次 prompt turn 的项目记忆 fail-soft 原因（C2 T3 复核修复）。
    ///
    /// **诊断面，不是协议面**：T0 没有观察到 oracle 的记忆失败诊断（日志/事件），
    /// C2 不发明——失败路径在线上静默，原因只写这里供测试/宿主诊断读取（无日志
    /// 输出、不进 snapshot/协议）。覆盖写语义与 [`agent_core::Session::rule_store_error`]
    /// 同款：每个 prompt turn 的加载结果覆盖（fail-soft 失败 `Some(原因)`、成功或
    /// `memoryEnabled:false` 关闭 `None` 清空）；compact / 配置失败 turn 不写
    /// （没有记忆加载）。
    last_memory_error: RwLock<Option<String>>,
    /// 前台工具执行窗口（`tool.updated started` → `batch`，C3 T3）。
    ///
    /// `TaskRegistry` 的通知判据在任务完成线程上读它：窗口内完成的通知按 C1 观察
    /// 丢弃；窗口外（含模型流式等待）入队，等当前 turn 结束后合并投递。
    tool_execution: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Live> {
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn read_config(&self) -> RuntimeConfig {
        self.config
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 提交一条已分配事件（EventSink 在落库之后调用）。
    fn publish(&self, event: &SessionEvent) {
        if let Some(enriched) = self.enriched_retry_event(event) {
            self.track_api_retry(&enriched);
            self.events.commit(&enriched);
        } else {
            self.track_api_retry(event);
            self.events.commit(event);
        }
        // 终态后释放该 turn 的重试追踪 id（不随会话生命周期累积），并清空 apiRetry
        // （C3 T3）：goal 续跑失败后状态可能回到 idle，若只靠 completed/error 快照
        // 清空，上一 turn 的 retry 态会泄漏到下一次快照。
        if matches!(
            event.event_type,
            SessionEventType::TurnCompleted | SessionEventType::TurnFailed
        ) {
            self.api_retry
                .clear_session(&mode_api_retry::SessionTarget {
                    workspace_path: self.record.directory.clone(),
                    workspace_identity: self.record.workspace_id.clone(),
                    session_id: event.session_id.clone(),
                });
            if let Some(turn_id) = &event.turn_id {
                self.retry_traces
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(turn_id);
            }
        }
    }

    /// 一个 turn 的重试追踪 id（首次生成后复用）。
    fn retry_trace(&self, turn_key: &str) -> RetryTrace {
        let mut traces = self
            .retry_traces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        traces
            .entry(turn_key.to_string())
            .or_insert_with(RetryTrace::new)
            .clone()
    }

    /// C3 T2：用宿主事实补齐 agent-core 无法知道的重试事件字段。
    ///
    /// agent-core 只持有分类/计数；`providerId`/`modelId`/`sessionId`/`turnId`/
    /// `traceId`/`querySource`/`queryId` 与 `modelCall` 的形状属于宿主投影面。
    /// `providerKind`/`transport`/`baseURL`/header 统计仍是未接线事实，逐项登记在
    /// 契约 §2.7；这里不伪造这些值。
    fn enriched_retry_event(&self, event: &SessionEvent) -> Option<SessionEvent> {
        if event.event_type != SessionEventType::SessionUpdated {
            return None;
        }
        let payload = event.payload.as_ref().and_then(Value::as_object)?;
        let kind = payload.get("type").and_then(Value::as_str);
        if !matches!(
            kind,
            Some("model_retry_scheduled" | "model_request_completed" | "model_request_failed")
        ) {
            return None;
        }
        let config = self.read_config();
        let trace = self.retry_trace(event.turn_id.as_deref().unwrap_or_default());
        let mut enriched = payload.clone();
        if !config.provider_id.is_empty() {
            enriched
                .entry("providerId".to_string())
                .or_insert_with(|| Value::String(config.provider_id.clone()));
        }
        if !config.model_id.is_empty() {
            enriched
                .entry("modelId".to_string())
                .or_insert_with(|| Value::String(config.model_id.clone()));
        }
        enriched.insert(
            "sessionId".to_string(),
            Value::String(event.session_id.clone()),
        );
        if let Some(turn_id) = &event.turn_id {
            enriched.insert("turnId".to_string(), Value::String(turn_id.clone()));
        }
        if let Some(trace_id) = &self.record.trace_id {
            enriched
                .entry("traceId".to_string())
                .or_insert_with(|| Value::String(trace_id.clone()));
        }
        enriched.insert(
            "querySource".to_string(),
            Value::String("main_turn".to_string()),
        );
        enriched.insert("queryId".to_string(), Value::String(trace.query_id.clone()));
        enriched.insert(
            "requestId".to_string(),
            Value::String(uuid::Uuid::new_v4().to_string()),
        );
        enriched.insert("spanId".to_string(), Value::String(trace.span_id.clone()));
        enriched.insert(
            "parentSpanId".to_string(),
            Value::String(trace.parent_span_id.clone()),
        );
        enriched.insert(
            "modelRequestSessionType".to_string(),
            Value::String("main".to_string()),
        );
        enriched.insert("timestamp".to_string(), Value::from(event.timestamp));
        let mut model_call = serde_json::json!({
            "operation": "agent_step",
            "actorKind": "main",
            "operationId": trace.operation_id,
            "logicalCallId": trace.logical_call_id,
            "stepIndex": 0,
            "reasoning": {
                "capability": "unknown",
                "requestedControl": "provider_default",
                "requestedState": "provider_default",
                "requestedLevel": config
                    .reasoning_level
                    .clone()
                    .unwrap_or_else(|| "provider_default".to_string()),
                "effectiveControl": "unknown",
                "effectiveState": "unknown",
                "effectiveLevel": "unknown",
            },
        });
        if let Some(recovery) = enriched.get("streamRecovery") {
            model_call["callCause"] = Value::String("recovery".to_string());
            let retry_number = recovery.get("retryNumber").cloned().unwrap_or(Value::Null);
            model_call["attributes"] = serde_json::json!({
                "streamRecoveryNumber": retry_number,
            });
        }
        enriched.insert("modelCall".to_string(), model_call);
        Some(SessionEvent {
            payload: Some(Value::Object(enriched)),
            ..event.clone()
        })
    }

    /// 把重试相关事件喂给 `mode-api-retry` 的唯一归一化实现。
    ///
    /// - `session.updated` / `streamRecovery.updated`：apiRetry 状态/清除规则；
    /// - `model.streaming`：恢复进展（非空 text/reasoning/tool delta）按 crate 的
    ///   `is_mode_model_retry_recovery_progress_payload` 清除重试状态（§2.5）。
    fn track_api_retry(&self, event: &SessionEvent) {
        let target = mode_api_retry::SessionTarget {
            workspace_path: self.record.directory.clone(),
            workspace_identity: self.record.workspace_id.clone(),
            session_id: event.session_id.clone(),
        };
        if event.event_type == SessionEventType::ModelStreaming {
            if let Some(payload) = event.payload.as_ref() {
                self.api_retry
                    .track_model_streaming_progress(&target, payload);
            }
            return;
        }
        if !matches!(
            event.event_type,
            SessionEventType::SessionUpdated | SessionEventType::StreamRecoveryUpdated
        ) {
            return;
        }
        let Ok(inner) = serde_json::to_value(event) else {
            return;
        };
        let wrapper = json!({ "type": "session.event", "event": inner });
        let _ = self
            .api_retry
            .track_api_retry_from_session_event(&target, &wrapper);
    }

    /// 顺序提交一批已分配事件（goal 方法层的同步事件）。
    fn publish_many(&self, events: Vec<SessionEvent>) {
        for event in events {
            if let Some(enriched) = self.enriched_retry_event(&event) {
                self.events.commit(&enriched);
            } else {
                self.events.commit(&event);
            }
        }
    }
}

/// 方法层在共享发布器上分配一条事件（不提交；由 `publish_many` 或 worker 提交）。
fn make_event(
    events: &agent_core::EventPublisher,
    turn_id: Option<String>,
    event_type: SessionEventType,
    payload: Value,
) -> SessionEvent {
    events.allocate(
        turn_id,
        event_type,
        payload,
        DeliveryKind::DesktopContinuous,
    )
}

/// 一个会话的运行时：准入（`submit`）→ 串行 worker → 事件（`subscribe`/`snapshot`）。
///
/// Drop 本结构不会打断正在跑的 turn：worker 在队列清空且发送端关闭后自然退出
/// （`JoinHandle` 保留在此，供后续任务的显式 close 接线）。turn 的执行体运行在
/// `spawn_blocking` 线程上（Task 8）：同步的 [`PermissionHandler`] 审批等待
/// （反向请求）不能占 tokio worker 线程，否则 current-thread runtime 下与 stdio
/// 写者互等死锁。
pub struct SessionRuntime {
    shared: Arc<Shared>,
    queue: tokio::sync::mpsc::UnboundedSender<QueuedInput>,
    worker: tokio::task::JoinHandle<()>,
}

impl SessionRuntime {
    /// 构造运行时并启动 per-session worker。
    ///
    /// `wiring`（Task 8）：审批 handler / `allow_project` 规则库 / 偏好请求 hub。
    /// `rule_db_path` 与 store 同库（`local_setting` 行）；审批缺省走
    /// `agent-core` 的 `AutoAllow`（不经 stdio）。
    ///
    /// `next_message_sequence` 按库内已有 message 行的**最大序号 +1** 初始化（Task 7
    /// 控制器裁决 6）：在已有历史的会话上重开运行时（resume）时，新行的 `sequence`
    /// 接着旧行编号。用 `len()` 会在序号有洞（旧版本/迁移数据）时与历史撞号。
    pub fn new(
        record: SessionRecord,
        store: Arc<SessionStore>,
        source: Arc<dyn ProviderSource>,
        config: RuntimeConfig,
        wiring: RuntimeWiring,
    ) -> Result<Self, RuntimeError> {
        let existing = store
            .messages(&record.id, None, None)
            .map_err(|error| RuntimeError::Store(error.to_string()))?;
        let next_message_sequence = existing
            .iter()
            .map(|message| message.message.sequence.unwrap_or(-1))
            .max()
            .unwrap_or(-1)
            + 1;
        // 冷物化恢复（最终评审 C2/C3）：模型历史、FileState 与 turn 计数器都来自
        // 已持久化的 message/part/turn_usage。create-on-existing / resume / 重启共用
        // 本构造入口，因此三条路径都得到同一恢复语义。
        let turn_ids = store
            .turn_usage_turn_ids(&record.id)
            .map_err(|error| RuntimeError::Store(error.to_string()))?;
        // `contextUsage` 的冷恢复事实：逐请求的 tokens 已在 assistant 消息里持久化，
        // 不从 projection 计数器重算（T0：resume 后 projection 归 0，contextUsage 保留）。
        let usage = UsageAccounting::from_messages(&existing);
        let restored = crate::history::restore(&record.directory, &existing, &turn_ids);
        // C3 T1：目标从 `session_target` 冷恢复（active 转 paused、清 active 字段），
        // 验证时间线从 `session_entry` 重建；不自动续跑（无 idle turn）。
        let goal = load_goal_state(&store, &record.id)
            .map_err(|error| RuntimeError::Store(error.to_string()))?;

        let events = agent_core::EventPublisher::new(
            record.id.clone(),
            config.broadcast_capacity,
            config.event_log_capacity,
        );
        let (drained, _drained_receiver) = tokio::sync::watch::channel(false);
        let (queue, receiver) = tokio::sync::mpsc::unbounded_channel();
        let data_root = config.data_root.clone();
        let shared = Arc::new(Shared {
            live: Mutex::new(Live {
                status: RuntimeStatus::Idle,
                revision: 0,
                updated_at: record.time_updated,
                last_error: None,
                turn_count: 0,
                total_token_count: 0,
                // 冷物化：投影计数与 currentTurnId 都归 0/None（T0 观察 resume 后
                // projection 归 0），contextUsage 由 `usage` 单独恢复。
                context_used: 0,
                current_turn_id: None,
                next_message_sequence,
                seen_inputs: HashMap::new(),
                inbox: CommandInbox::new(),
                current_cancel: None,
                current_kind: None,
                compact: CompactCoordinator::default(),
                usage,
                goal,
                goal_cancel: None,
            }),
            config: RwLock::new(config),
            events: Arc::clone(&events),
            drained,
            store: Arc::clone(&store),
            preferences: RwLock::new(None),
            last_memory_error: RwLock::new(None),
            api_retry: mode_api_retry::SessionApiRetryTracker::new(),
            tool_execution: AtomicBool::new(false),
            retry_traces: Mutex::new(HashMap::new()),
            record,
        });

        let mut session = Session::new(shared.record.id.clone(), shared.record.directory.clone())
            .with_event_publisher(Arc::clone(&events))
            .with_mode(shared.read_config().mode)
            .with_model_capabilities(shared.read_config().capabilities)
            // C3 T2：重试/退避策略由 RuntimeConfig 注入（测试可置 0 延迟）。
            .with_retry_policy(shared.read_config().retry)
            .with_history(restored.messages)
            .with_file_state(restored.file_state)
            .with_turn_state(restored.next_turn_number, restored.used_turn_ids);
        if let Some(data_root) = data_root {
            session = session.with_data_root(data_root);
        }
        if let Some(handler) = wiring.permission_handler {
            session = session.with_permission_handler(handler);
        }
        if let Some(rule_db_path) = wiring.rule_db_path {
            session = session.with_sqlite_rules(rule_db_path);
        }
        // C3 T3：后台通知的准入判据 + 空闲唤醒。队列仍在 `TaskRegistry`；
        // 这里只提供「前台工具窗口丢弃」与「入队后叫醒 worker」两个钩子。
        let notification_signal = Arc::new(tokio::sync::Notify::new());
        {
            let shared_for_filter = Arc::clone(&shared);
            let signal = Arc::clone(&notification_signal);
            session.tasks().set_notification_hooks(
                Arc::new(move |_notification| {
                    !shared_for_filter.tool_execution.load(Ordering::SeqCst)
                }),
                Arc::new(move || signal.notify_one()),
            );
        }
        let worker = tokio::spawn(worker_loop(
            receiver,
            Arc::clone(&shared),
            session,
            store,
            source,
            wiring.reverse,
            notification_signal,
        ));
        Ok(Self {
            shared,
            queue,
            worker,
        })
    }

    /// 接受一条输入：幂等（同 `inputId` 返回首次回执）→ 乐观并发校验 → 准入入队。
    ///
    /// 立即返回，不等 turn 结束；`Accepted.admission` 区分 running / queued。
    pub fn submit(&self, input: SessionInput) -> Result<Accepted, RuntimeError> {
        let mut live = self.shared.lock();
        let provided = input
            .input_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let input_id = match provided {
            Some(value) => value.to_string(),
            None => fresh_id("in"),
        };

        // 幂等：同 inputId 的重复提交不再入队、不再动 revision。
        if let Some(existing) = live.seen_inputs.get(&input_id) {
            return Ok(existing.clone());
        }
        // 乐观并发：版本不一致时显式拒绝，不启动 turn。
        if let Some(expected) = input.expected_revision {
            if expected != live.revision {
                return Err(RuntimeError::RevisionConflict {
                    expected,
                    current: live.revision,
                });
            }
        }

        let admission = live.inbox.admit(&self.shared.record.id, input_id.clone());
        live.revision += 1;
        let accepted = Accepted {
            state_revision: live.revision,
            input_id: input_id.clone(),
            admission,
        };
        // 先确认入队成功再记幂等回执：队列入队失败时不能把「已接受」写进台账
        //（最终评审 Minor；失败后 runtime 已关闭，回执也不会再被查询）。
        self.queue
            .send(QueuedInput {
                input,
                input_id: input_id.clone(),
                compact: None,
                goal: None,
                notifications: Vec::new(),
            })
            .map_err(|_| RuntimeError::RuntimeClosed)?;
        live.seen_inputs.insert(input_id, accepted.clone());
        Ok(accepted)
    }

    /// 订阅事件流：`(replay, live)`。replay 来自内存日志（`seq > after_seq`，缺省
    /// 全部），live 是 broadcast receiver——两段拼起来无重复、无缺口；终态事件已
    /// 发布时进 replay，尚未发布时必进 live 缓冲。
    pub fn subscribe(
        &self,
        after_seq: Option<u64>,
    ) -> (
        Vec<SessionEvent>,
        tokio::sync::broadcast::Receiver<SessionEvent>,
    ) {
        let (replay, receiver, _effective) = self.subscribe_with_cursor(after_seq);
        (replay, receiver)
    }

    /// 订阅事件流并返回**生效的**游标（最终评审 I10）。
    ///
    /// `after_seq > event_seq` 说明客户端拿的是旧 runtime 的游标（close/resume 后
    /// 事件序号重新开始）。此时按「重置」处理：replay 不过滤，live 转发也不过滤；
    /// 否则新 runtime 的事件 seq 永远追不上旧游标，订阅者会静默丢掉所有新事件。
    /// 返回的游标是 live 转发应使用的过滤值（`None` = 不丢任何事件）。
    pub fn subscribe_with_cursor(
        &self,
        after_seq: Option<u64>,
    ) -> (
        Vec<SessionEvent>,
        tokio::sync::broadcast::Receiver<SessionEvent>,
        Option<u64>,
    ) {
        self.shared.events.subscribe(after_seq)
    }

    /// 取消当前运行中的 turn（裁决 4）。空闲时是幂等 no-op；排队中的输入不受影响。
    ///
    /// C3 T1：goal 续跑/verifier 也由同一个取消面收口——取消 goal 任务令牌并把目标
    /// 置为 `paused`（`session/stop` 的观察：target paused、turn cancelled）。
    /// `status_updated` 事件在锁内预分配（seq 立即进入快照），由 worker 在终态/
    /// verifier 取消前发布，保证 `status_updated → run_finished → turn.completed`
    /// 的观察顺序；`goal_state_change` 消息行在这里落库。
    pub fn cancel_turn(&self) -> Result<(), RuntimeError> {
        let mut live = self.shared.lock();
        if let Some(token) = &live.current_cancel {
            token.cancel();
        }
        if let Some(token) = &live.goal_cancel {
            token.cancel();
        }
        let mut publish_now = None;
        if let Some(mut target) = live.goal.target.clone() {
            if target.status == GoalStatus::Active {
                let previous = target.clone();
                target.set_status(GoalStatus::Paused, now_ms());
                target.time_updated = now_ms();
                self.shared
                    .store
                    .upsert_target(&target_record(&target))
                    .map_err(|error| RuntimeError::Store(error.to_string()))?;
                live.goal.target = Some(target.clone());
                live.revision += 1;
                let session_id = self.shared.record.id.clone();
                let event = make_event(
                    &self.shared.events,
                    None,
                    SessionEventType::SessionUpdated,
                    target_update_payload(
                        "status_updated",
                        "command",
                        Some(&previous),
                        Some(&target),
                    ),
                );
                // goal-continuation 有 before_terminal 钩子，事件要排队保证
                // status_updated → run_finished → turn.completed；普通 prompt/compact
                // 没有钩子，直接提交（否则 worker 已过 drain 点时会永久缺号）。
                let goal_turn = live.goal.job_active
                    && matches!(live.current_kind, Some(TurnKind::GoalContinuation));
                if goal_turn {
                    live.goal.pending_cancel_events.push_back(event);
                } else {
                    publish_now = Some(event);
                }
                let sequence = live.next_message_sequence;
                live.next_message_sequence += 1;
                let (message, part) = goal_message_rows(
                    &session_id,
                    "Goal paused",
                    "system_reminder",
                    Some("goal_state_change"),
                    sequence,
                    now_ms(),
                );
                let _ = self
                    .shared
                    .store
                    .append_message_with_parts(message, &[part]);
            }
        }
        drop(live);
        if let Some(event) = publish_now {
            self.shared.publish(&event);
        }
        Ok(())
    }

    /// 当前乐观并发版本（setter 的 `expectedRevision` 校验用）。
    pub fn state_revision(&self) -> u64 {
        self.shared.lock().revision
    }

    /// 当前事件 seq 高水位（`session/subscribe` 结果的 `eventSeq`）。
    pub fn event_seq(&self) -> u64 {
        self.shared.events.current_seq()
    }

    /// worker 退出（drain 完成）信号的订阅端（`session/close` 的 closing 台账用；
    /// `materialize` 直接 `borrow()` 判断 drain 是否已结束）。
    pub(crate) fn drain_watcher(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shared.drained.subscribe()
    }

    /// 应用一次 setter 的配置更新（Task 7 方法层）。
    ///
    /// `expected_revision` 与当前 `stateRevision` 不一致时报
    /// [`RuntimeError::RevisionConflict`]（**不改配置**）；成功时原位替换配置并
    /// 推进版本（状态面事实变了）。落库（`runtime/model_selection` 等 entry）由
    /// 方法层负责——本方法只管内存事实。
    pub fn update_config(
        &self,
        expected_revision: Option<u64>,
        apply: impl FnOnce(&mut RuntimeConfig),
    ) -> Result<u64, RuntimeError> {
        let mut live = self.shared.lock();
        if let Some(expected) = expected_revision {
            if expected != live.revision {
                return Err(RuntimeError::RevisionConflict {
                    expected,
                    current: live.revision,
                });
            }
        }
        {
            let mut config = self
                .shared
                .config
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            apply(&mut config);
        }
        live.revision += 1;
        live.updated_at = now_ms();
        Ok(live.revision)
    }

    /// worker 的 join 句柄（后续任务的显式 close 接线用；本结构 Drop 不打断在跑的 turn）。
    pub fn worker(&self) -> &tokio::task::JoinHandle<()> {
        &self.worker
    }

    /// 关闭队列并等待 worker 结束（测试/嵌入方的确定性收尾；生产 close 走方法层）。
    ///
    /// 返回后队列已空、worker 已退出、`drained` 为 true；比 `drop` + 轮询更确定。
    pub async fn shutdown(self) {
        let Self { queue, worker, .. } = self;
        drop(queue);
        let _ = worker.await;
    }

    /// 活跃快照：mode / model / status / turnCount / usage 反映活跃状态；
    /// `messages` 恒空（历史消息由 `session/read` / `session/resume` 从 store 投影）。
    /// settings / slashCommands / projection 的事实面按 C2 T6 对齐（见
    /// `state::build_snapshot` 与 `agent-c2-contract.md` §4）。
    pub fn snapshot(&self) -> agent_protocol::snapshot::SessionStateSnapshot {
        let live = self.shared.lock();
        self.snapshot_locked(&live)
    }

    /// 锁内构造快照（`session/compact` 应答与 [`Self::snapshot`] 共用）。
    ///
    /// compact 的应答快照必须在**受理的同一临界区**里取：受理后 worker 才可能进入
    /// 执行，因此 `accepted` 应答的 `activeTurnId` 恒缺席、`already_running` 应答的
    /// 恒存在（与夹具 §1.6/§1.7 的时序一致，不依赖调度竞态）。
    fn snapshot_locked(&self, live: &Live) -> agent_protocol::snapshot::SessionStateSnapshot {
        let config = self.shared.read_config();
        let view = LiveView {
            status: live.status,
            revision: live.revision,
            updated_at: live.updated_at,
            last_error: live.last_error.clone(),
            turn_count: live.turn_count,
            total_token_count: live.total_token_count,
            context_used: live.context_used,
            event_seq: self.shared.events.current_seq(),
            current_turn_id: live.current_turn_id.clone(),
            context_usage: live.usage.context_usage(config.capabilities.context_window),
            active_turn: live
                .compact
                .running_turn_id
                .clone()
                .map(|turn_id| (turn_id, TurnKind::Compact)),
            target: live.goal.target.as_ref().map(goal_target_protocol),
            goal_verifications: live.goal.verifications.clone(),
            goal_verification_timeline: live.goal.timeline.clone(),
        };
        let snapshot = build_snapshot(&self.shared.record, &config, &view);
        // C3 T2：`runtime.apiRetry` 由 `mode-api-retry` 的唯一投影规则生成。
        // 走 crate 自己的 snapshot API（序列化→投影→反序列化），不复制归一化逻辑。
        let Ok(value) = serde_json::to_value(&snapshot) else {
            return snapshot;
        };
        let projected = self.shared.api_retry.with_api_retry_runtime(&value);
        serde_json::from_value(projected).unwrap_or(snapshot)
    }

    /// `session/resume` 的 `goalStats` 投影（观察键：timeUsedSeconds/tokensUsed/
    /// tokenBudget/contextUsed/contextWindow/toolCallCount/iterationCount）。
    ///
    /// 冷恢复观察 `contextUsed:0`、`iterationCount:1`；toolCallCount 未观察到独立
    /// 事实，按 0（登记 §1.11）。
    pub(crate) fn goal_stats(&self) -> Option<SessionGoalStats> {
        let live = self.shared.lock();
        let target = live.goal.target.as_ref()?;
        let config = self.shared.read_config();
        Some(SessionGoalStats {
            time_used_seconds: target.time_used_seconds,
            tokens_used: target.tokens_used,
            token_budget: target.token_budget,
            context_used: live.context_used,
            context_window: config.capabilities.context_window.unwrap_or(0),
            tool_call_count: 0,
            iteration_count: live.goal.iteration.max(1),
        })
    }

    /// `session/compact` 的受理入口（C2 T2；契约 §1.1/§1.6）。
    ///
    /// 在**同一把 `Live` 锁**内完成：`expectedRevision` 校验 → 单飞判定
    /// （执行中 → `already_running`）→ prompt 执行中 → `-32010` → 受理入队并推进
    /// revision。冲突/拒绝路径零事件、零落库、revision 不动；受理路径先入队再改
    /// 状态（worker 取任务也要拿这把锁，两步之间不可能被插队）。
    ///
    /// 返回值带应答快照（见 [`Self::snapshot_locked`] 的时序说明）。
    pub(crate) fn request_compact(
        &self,
        params: &SessionCompactParams,
    ) -> Result<
        (
            CompactAccept,
            agent_protocol::snapshot::SessionStateSnapshot,
        ),
        CompactReject,
    > {
        let mut live = self.shared.lock();
        // 乐观并发：冲突先于一切状态判定（契约 §1.1：错误应答、零事件零落库）。
        if let Some(expected) = params.expected_revision {
            if expected != live.revision {
                return Err(CompactReject::RevisionConflict {
                    expected,
                    current: live.revision,
                });
            }
        }
        let input_id = params
            .input_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| fresh_id("in"));
        // 单飞：已有 compact 在执行 → already_running（不动 revision，不入队）。
        if live.compact.is_running() {
            let snapshot = self.snapshot_locked(&live);
            return Ok((CompactAccept::AlreadyRunning { input_id }, snapshot));
        }
        // prompt 正在执行 → -32010（compact turn 执行中不走这支：相位已是 Running）。
        if live.current_kind == Some(TurnKind::Prompt) {
            return Err(CompactReject::PromptRunning);
        }
        let settled_at_enqueue = live.compact.settled_ok;
        let job = CompactJob {
            input_id: input_id.clone(),
            instructions: params.instructions.clone(),
            settled_at_enqueue,
        };
        let display = compact_display(job.instructions.as_deref());
        let item = QueuedInput {
            input: SessionInput {
                input_id: Some(job.input_id.clone()),
                content: display,
                expected_revision: None,
            },
            input_id: job.input_id.clone(),
            compact: Some(job),
            goal: None,
            notifications: Vec::new(),
        };
        // 先入队、后改状态：worker 取任务需要 `live` 锁，本临界区内不会被插队；
        // 入队失败（worker 已退出）时不留下半截受理状态。
        if self.queue.send(item).is_err() {
            return Err(CompactReject::RuntimeClosed);
        }
        live.compact.on_accept();
        live.revision += 1;
        let snapshot = self.snapshot_locked(&live);
        Ok((CompactAccept::Accepted { input_id }, snapshot))
    }

    /// `session/goal` 的受理入口（C3 T1；契约 §1.1–§1.9）。
    ///
    /// 每个 action 在自己的 `Live` 临界区里做 revision/运行中判定并准备事件；
    /// 临界区释放后才发布事件（`publish_many` 需要同一把锁）与入队。
    pub(crate) fn request_goal(
        &self,
        params: &SessionGoalParams,
    ) -> Result<GoalOutcome, GoalReject> {
        match params.action {
            SessionGoalAction::Show => self.goal_show(params),
            SessionGoalAction::Pause => self.goal_pause(params),
            SessionGoalAction::Clear => self.goal_clear(params),
            SessionGoalAction::Resume => self.goal_resume(params),
            SessionGoalAction::Set | SessionGoalAction::Replace => self.goal_set(params),
        }
    }

    /// 取得 goal 命令的 `Live` 临界区并做 revision/运行中判定。
    fn goal_lock(
        &self,
        params: &SessionGoalParams,
        reject_running: bool,
    ) -> Result<MutexGuard<'_, Live>, GoalReject> {
        let live = self.shared.lock();
        if let Some(expected) = params.expected_revision {
            if expected != live.revision {
                return Err(GoalReject::RevisionConflict {
                    expected,
                    current: live.revision,
                });
            }
        }
        if reject_running
            && (live.goal.job_active
                || live
                    .current_kind
                    .map(TurnKind::is_prompt_running)
                    .unwrap_or(false))
        {
            return Err(GoalReject::PromptRunning);
        }
        Ok(live)
    }

    /// `show`：只读；运行中同样 `-32010`（观察：show 不是例外）。
    fn goal_show(&self, params: &SessionGoalParams) -> Result<GoalOutcome, GoalReject> {
        let live = self.goal_lock(params, true)?;
        let target = live.goal.target.as_ref();
        let response = match target {
            Some(target) => target.status_response(),
            None => "No goal is set. Use /goal <objective> to set one.".to_string(),
        };
        Ok(GoalOutcome {
            response,
            snapshot: self.snapshot_locked(&live),
            started_turn: false,
        })
    }

    fn goal_set(&self, params: &SessionGoalParams) -> Result<GoalOutcome, GoalReject> {
        let mut live = self.goal_lock(params, true)?;
        let Some(objective) = params
            .objective
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            let response = match params.action {
                SessionGoalAction::Replace => "Usage: /goal replace <objective>",
                _ => "Usage: /goal <objective>",
            };
            return Ok(GoalOutcome {
                response: response.to_string(),
                snapshot: self.snapshot_locked(&live),
                started_turn: false,
            });
        };
        let now = now_ms();
        let session_id = self.shared.record.id.clone();
        let previous = live.goal.target.clone();
        let target = GoalTarget::new(
            session_id.clone(),
            fresh_id("target"),
            objective.to_string(),
            now,
        );
        self.persist_goal_target(&target)?;

        let mut events = Vec::new();
        let emit_title = !live.goal.title_emitted && self.shared.record.title.is_empty();
        if emit_title {
            events.push(make_event(
                &self.shared.events,
                None,
                SessionEventType::SessionTitleUpdated,
                json!({
                    "previousTitle": "",
                    "source": "first_input",
                    "title": objective,
                }),
            ));
        }

        // 控制 turn：`turnNumber` 用控制 turn 自己的 0 起序号（观察：第二个目标
        // 的控制 turn 仍是 1，与上一次已开始的 continuation turn 序号重叠）。
        let control_number = live.goal.control_turns;
        let control_turn_id = fresh_id("turn_control");
        let control_started_at = now_ms();
        events.push(make_event(
            &self.shared.events,
            Some(control_turn_id.clone()),
            SessionEventType::TurnStarted,
            json!({
                "turnNumber": control_number,
                "input": objective,
                "messageId": uuid::Uuid::new_v4().to_string(),
                "executionKind": "controlOnly",
            }),
        ));
        events.push(make_event(
            &self.shared.events,
            Some(control_turn_id.clone()),
            SessionEventType::TurnCompleted,
            json!({
                "response": "",
                "tokenCount": 0,
                "toolCallCount": 0,
                "duration": (now_ms() - control_started_at).max(0) as f64,
                "resultType": "success",
            }),
        ));

        // summary_updated → set（事件 target 的 summaryTitle 为 null；落库行是 objective）。
        let mut event_target = target.clone();
        event_target.summary_title = None;
        events.push(make_event(
            &self.shared.events,
            None,
            SessionEventType::SessionUpdated,
            target_update_payload(
                "summary_updated",
                "runtime",
                Some(&event_target),
                Some(&target),
            ),
        ));
        events.push(make_event(
            &self.shared.events,
            None,
            SessionEventType::SessionUpdated,
            target_update_payload("set", "command", previous.as_ref(), Some(&event_target)),
        ));

        let old_goal = live.goal.clone();
        debug_assert!(
            old_goal.pending_cancel_events.is_empty(),
            "set 覆盖 GoalState 前仍有未发布的 pending 事件（会永久缺号）"
        );
        live.goal = GoalState {
            target: Some(target.clone()),
            timeline: Vec::new(),
            verifications: Vec::new(),
            iteration: 0,
            continuation_ordinal: old_goal.continuation_ordinal,
            control_turns: control_number + 1,
            job_active: true,
            pending_cancel_events: std::collections::VecDeque::new(),
            title_emitted: old_goal.title_emitted || emit_title,
        };
        live.current_turn_id = Some(control_turn_id);
        live.turn_count += 1;
        // 首次创建 +3（命令 + 控制 turn + 目标切换，夹具 2 → 5）；替换已有目标 +1
        // （夹具 set-over/replace 的第二目标只前进 1）。
        live.revision += if previous.is_some() { 1 } else { 3 };
        let objective_sequence = live.next_message_sequence;
        live.next_message_sequence += 1;
        let input_id = goal_input_id(params.input_id.as_deref());
        let cancel = CancelToken::new();
        live.goal_cancel = Some(cancel);
        live.inbox.admit(&session_id, input_id.clone());
        let snapshot = self.snapshot_locked(&live);
        // 「事件可见 ⇒ 行已提交」：先写 objective/标题行，再发布控制/set 事件。
        self.persist_goal_user_prompt_with_sequence(&session_id, objective, objective_sequence);
        if emit_title {
            let _ = self
                .shared
                .store
                .set_session_title(&session_id, objective, "first_input", now);
        }
        drop(live);
        self.shared.publish_many(events);
        let job = GoalJob { input_id };
        if let Err(error) = self.enqueue_goal_item(job) {
            let mut live = self.shared.lock();
            live.goal.job_active = false;
            live.goal_cancel = None;
            return Err(error);
        }
        Ok(GoalOutcome {
            response: target.status_response(),
            snapshot,
            started_turn: true,
        })
    }

    fn goal_resume(&self, params: &SessionGoalParams) -> Result<GoalOutcome, GoalReject> {
        let mut live = self.goal_lock(params, true)?;
        let Some(target) = live.goal.target.clone() else {
            return Ok(GoalOutcome {
                response: "No goal to resume.".to_string(),
                snapshot: self.snapshot_locked(&live),
                started_turn: false,
            });
        };
        if target.status != GoalStatus::Paused {
            return Ok(GoalOutcome {
                response: target.status_response(),
                snapshot: self.snapshot_locked(&live),
                started_turn: false,
            });
        }
        let now = now_ms();
        let session_id = self.shared.record.id.clone();
        let previous = target.clone();
        let mut resumed = target;
        resumed.set_status(GoalStatus::Active, now);
        resumed.active_input_id = None;
        resumed.active_run_started_at = None;
        resumed.active_run_last_seen_at = None;
        self.persist_goal_target(&resumed)?;
        live.goal.target = Some(resumed.clone());
        let status_event = make_event(
            &self.shared.events,
            None,
            SessionEventType::SessionUpdated,
            target_update_payload("status_updated", "runtime", Some(&previous), Some(&resumed)),
        );
        live.goal.job_active = true;
        debug_assert!(
            live.goal.pending_cancel_events.is_empty(),
            "resume 清理 pending 队列前仍有未发布事件（会永久缺号）"
        );
        live.goal.pending_cancel_events.clear();
        live.revision += 1;
        let message_sequence = live.next_message_sequence;
        live.next_message_sequence += 1;
        let input_id = goal_input_id(params.input_id.as_deref());
        live.goal_cancel = Some(CancelToken::new());
        live.inbox.admit(&session_id, input_id.clone());
        let snapshot = self.snapshot_locked(&live);
        self.persist_goal_state_message_with_sequence(
            &session_id,
            "Goal resumed",
            "goal_state_change",
            message_sequence,
        );
        drop(live);

        self.shared.publish_many(vec![status_event]);
        let job = GoalJob { input_id };
        if let Err(error) = self.enqueue_goal_item(job) {
            let mut live = self.shared.lock();
            live.goal.job_active = false;
            live.goal_cancel = None;
            return Err(error);
        }
        Ok(GoalOutcome {
            response: goal_resumed_response(&resumed),
            snapshot,
            started_turn: true,
        })
    }

    fn goal_pause(&self, params: &SessionGoalParams) -> Result<GoalOutcome, GoalReject> {
        let mut live = self.goal_lock(params, false)?;
        let session_id = self.shared.record.id.clone();
        let Some(target) = live.goal.target.clone() else {
            // 无 goal 的副作用：首个 mutating action 发 titleUpdated（观察 §1.1）；
            // 不写任何 message 行。
            let emit_title = !live.goal.title_emitted && self.shared.record.title.is_empty();
            live.goal.title_emitted = true;
            let mut events = Vec::new();
            if emit_title {
                events.push(make_event(
                    &self.shared.events,
                    None,
                    SessionEventType::SessionTitleUpdated,
                    json!({
                        "previousTitle": "",
                        "source": "first_input",
                        "title": "/goal paused",
                    }),
                ));
            }
            live.revision += 1;
            let snapshot = self.snapshot_locked(&live);
            drop(live);
            self.shared.publish_many(events);
            if emit_title {
                let _ = self.shared.store.set_session_title(
                    &session_id,
                    "/goal paused",
                    "first_input",
                    now_ms(),
                );
            }
            return Ok(GoalOutcome {
                response: "No goal to pause.".to_string(),
                snapshot,
                started_turn: false,
            });
        };
        let now = now_ms();
        let previous = target.clone();
        let mut paused = target;
        if paused.status == GoalStatus::Active {
            paused.set_status(GoalStatus::Paused, now);
            self.persist_goal_target(&paused)?;
            live.goal.target = Some(paused.clone());
        }
        let cancel_event = make_event(
            &self.shared.events,
            None,
            SessionEventType::SessionUpdated,
            target_update_payload("status_updated", "command", Some(&previous), Some(&paused)),
        );
        // 只有 goal-continuation 有 before_terminal 钩子；普通 prompt/compact 中
        // pause 只能直接提交（否则 pending 事件会卡住后续 prompt 的日志/广播）。
        let running =
            live.goal.job_active && matches!(live.current_kind, Some(TurnKind::GoalContinuation));
        let mut publish_now = None;
        if running {
            // 事件 seq 已包含在应答快照里；实际发布由 worker 在终态/verifier 取消前
            // 完成，保证 status_updated → run_finished → turn.completed 的顺序。
            live.goal.pending_cancel_events.push_back(cancel_event);
            if let Some(cancel) = &live.goal_cancel {
                cancel.cancel();
            }
        } else {
            publish_now = Some(cancel_event);
        }
        let message_sequence = live.next_message_sequence;
        live.next_message_sequence += 1;
        live.revision += 1;
        let snapshot = self.snapshot_locked(&live);
        // 先落 message 行，再发布（idle pause 的 publish_now）。
        self.persist_goal_state_message_with_sequence(
            &session_id,
            "Goal paused",
            "goal_state_change",
            message_sequence,
        );
        drop(live);

        if let Some(event) = publish_now {
            self.shared.publish_many(vec![event]);
        }
        Ok(GoalOutcome {
            response: String::new(),
            snapshot,
            started_turn: false,
        })
    }

    fn goal_clear(&self, params: &SessionGoalParams) -> Result<GoalOutcome, GoalReject> {
        let mut live = self.goal_lock(params, true)?;
        let session_id = self.shared.record.id.clone();
        let previous = live.goal.target.clone();
        let mut events = Vec::new();
        let mut message_sequence = None;
        if previous.is_some() {
            self.shared
                .store
                .delete_target(&session_id)
                .map_err(|error| GoalReject::Store(error.to_string()))?;
            live.goal.target = None;
            live.goal.iteration = 0;
            message_sequence = Some(live.next_message_sequence);
            live.next_message_sequence += 1;
        }
        // 观察：clear 保留 runtime 的 goalVerifications / timeline。
        events.push(make_event(
            &self.shared.events,
            None,
            SessionEventType::SessionUpdated,
            target_update_payload("cleared", "command", previous.as_ref(), None),
        ));
        live.revision += 1;
        let snapshot = self.snapshot_locked(&live);
        if let Some(sequence) = message_sequence {
            self.persist_goal_state_message_with_sequence(
                &session_id,
                "Goal cleared",
                "goal_state_change",
                sequence,
            );
        }
        drop(live);

        self.shared.publish_many(events);
        Ok(GoalOutcome {
            response: if previous.is_some() {
                "Goal cleared.".to_string()
            } else {
                "No goal to clear.".to_string()
            },
            snapshot,
            started_turn: false,
        })
    }

    fn enqueue_goal_item(&self, job: GoalJob) -> Result<(), GoalReject> {
        let item = QueuedInput {
            input: SessionInput {
                input_id: Some(job.input_id.clone()),
                content: String::new(),
                expected_revision: None,
            },
            input_id: job.input_id.clone(),
            compact: None,
            goal: Some(job),
            notifications: Vec::new(),
        };
        self.queue.send(item).map_err(|_| GoalReject::RuntimeClosed)
    }

    fn persist_goal_target(&self, target: &GoalTarget) -> Result<(), GoalReject> {
        let record = target_record(target);
        self.shared
            .store
            .upsert_target(&record)
            .map_err(|error| GoalReject::Store(error.to_string()))
    }

    fn persist_goal_user_prompt_with_sequence(&self, session_id: &str, text: &str, sequence: i64) {
        let now = now_ms();
        let (message, part) =
            goal_message_rows(session_id, text, "user_prompt", None, sequence, now);
        let _ = self
            .shared
            .store
            .append_message_with_parts(message, &[part]);
    }

    fn persist_goal_state_message_with_sequence(
        &self,
        session_id: &str,
        text: &str,
        source: &str,
        sequence: i64,
    ) {
        let now = now_ms();
        let (message, part) = goal_message_rows(
            session_id,
            text,
            "system_reminder",
            Some(source),
            sequence,
            now,
        );
        let _ = self
            .shared
            .store
            .append_message_with_parts(message, &[part]);
    }

    /// 会话记录（方法层构造冷/热快照的 session 信息用）。
    pub fn record(&self) -> &SessionRecord {
        &self.shared.record
    }

    /// 当前配置快照（方法层判断「会话是否已有模型」用）。
    pub fn config(&self) -> RuntimeConfig {
        self.shared.read_config()
    }

    /// 宿主对 `session/requestRuntimePreferences` 的缓存应答（Task 8 控制器裁决 3）。
    ///
    /// 每运行时只发一次请求：取到宿主应答或登记回退默认后为 `Some`；
    /// worker 尚未完成请求（或没有连接级 hub，不发请求）时为 `None`。
    pub fn runtime_preferences(&self) -> Option<RuntimePreferences> {
        self.shared
            .preferences
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 最近一次 prompt turn 的项目记忆 fail-soft 原因（C2 T3 复核修复；诊断面）。
    ///
    /// 覆盖写语义：每个 prompt turn 的记忆加载结果覆盖——fail-soft 失败
    /// （目录缺失 / workspaceId 歧义 / 索引不可读 / 超限）`Some(原因)`，加载成功或
    /// `memoryEnabled:false` 关闭 `None` 清空。compact / 配置失败 turn 不写
    /// （没有记忆加载）。失败路径在线上**静默**（无日志输出、不进协议/snapshot）：
    /// T0 未观察 oracle 的记忆失败诊断面，C2 不发明——本字段只供测试与宿主诊断。
    pub fn last_memory_error(&self) -> Option<String> {
        self.shared
            .last_memory_error
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// 取走当前排队的后台完成通知，拼成 `(input, notifications)` 批次（C3 T3）。
///
/// `input` 是各条 `<task-notification>` 按完成顺序用空行连接的结果；返回 `None` =
/// 当前没有待投递通知。批次只产生**一个** turn（`background-busy-coalesced.json`）。
fn take_notification_batch(
    session: &Session,
) -> Option<(String, Vec<agent_core::TaskNotification>)> {
    let notifications = session.take_task_notifications();
    if notifications.is_empty() {
        return None;
    }
    let input = notifications
        .iter()
        .map(agent_core::TaskNotification::to_xml)
        .collect::<Vec<_>>()
        .join("\n\n");
    Some((input, notifications))
}

/// 取走已排队的后台完成通知，拼成一个内部 turn 输入（C3 T3）。
///
/// 一批通知只产生**一个** turn（`background-busy-coalesced.json`）：`input` 按完成
/// 顺序用空行分隔。返回 `None` = 当前没有待投递通知。通知 turn 不进入
/// `CommandInbox`（它没有用户受理步骤），但仍走同一个 worker 串行执行。
fn drain_notification_input(session: &Session) -> Option<QueuedInput> {
    let (input, notifications) = take_notification_batch(session)?;
    let input_id = fresh_id("in");
    Some(QueuedInput {
        input: SessionInput {
            input_id: Some(input_id.clone()),
            content: input,
            expected_revision: None,
        },
        input_id,
        compact: None,
        goal: None,
        notifications,
    })
}

/// worker 主体：串行消费队列直到发送端关闭且队列清空。
///
/// 启动时（Task 8 控制器裁决 3）向宿主发一次 `session/requestRuntimePreferences`
/// （scope = `runtime-materialization`；create 与冷 resume 各一次），应答/回退缓存进
/// [`Shared::preferences`]——首个输入的 turn 在此之后才开始（与 oracle 的 create
/// 等待偏好应答同序）。
///
/// **首个 turn 开始前**再发第二次偏好请求（scope = `user-execution`，Task 10）：
/// oracle 观察（`agent-server-c1-contract.md` §2.2）每次 materialize 是**一对**
/// 偏好请求，第二次在首个 send 的 turn 开始运行前发出（`session/send` 受理与
/// `turn.started` 之间）。C1 按此补齐：每运行时一次，取应答/回退后**覆盖**缓存
/// （两次请求同形状，后者是最终事实）。resume 的时机差异（oracle 在 resume 后
/// 立即成对发送、C1 仍等首个 turn）登记在 `tauri-agent-runtime.md` §C1 差异登记。
///
/// provider 按「(providerId, modelId)」缓存：配置未变的连续 turn 复用同一个 provider
/// 实例；setter 改了模型事实后才重新解析。解析放在 `spawn_blocking` 上：账号型
/// provider 的宿主凭据钩子会**阻塞**等待反向请求应答（Task 8 控制器裁决 4），
/// 不能占 tokio worker。配置失败（无模型 / 解析失败）走
/// [`run_configuration_failure`]，不静默用默认模型兜底（Task 7 控制器裁决 7）。
async fn worker_loop(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<QueuedInput>,
    shared: Arc<Shared>,
    mut session: Session,
    store: Arc<SessionStore>,
    source: Arc<dyn ProviderSource>,
    reverse: Option<Arc<crate::reverse::ReverseHub>>,
    notification_signal: Arc<tokio::sync::Notify>,
) {
    // panic 也必须结算 drain 信号：否则 `session/close` 的 closing 台账永远等不到
    // 「worker 已退出」，drain 守卫会把会话卡在 closing（最终评审 Minor）。
    let _drain_guard = DrainGuard(shared.drained.clone());
    if let Some(reverse) = &reverse {
        let preferences = reverse.request_runtime_preferences(&shared.record.id).await;
        *shared
            .preferences
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(preferences);
    }
    // user-execution 偏好只在每运行时的首个 turn 前发一次（见函数注释）。
    let mut user_execution_requested = false;
    let mut current: Option<(String, String, String, Arc<dyn ModelProvider>)> = None;
    loop {
        // C3 T3：先投递已完成的后台通知（当前 turn 的忙时完成在上一轮结束时就已
        // 入队），一批合并成一个 `background_task` turn；没有通知才等下一个输入。
        // `Notify` 只负责叫醒空闲 worker，队列仍归 `TaskRegistry`。
        let item = if let Some(item) = drain_notification_input(&session) {
            item
        } else {
            let maybe_item = tokio::select! {
                maybe_item = receiver.recv() => maybe_item,
                _ = notification_signal.notified() => continue,
            };
            match maybe_item {
                Some(item) => item,
                None => break,
            }
        };
        let is_notification_turn = !item.notifications.is_empty();
        // 取消令牌在**取到输入时**就登记（Task 10）：`session/close` 可能落在
        // 「准入已接受、turn 尚未开始」的等待窗口里（user-execution 偏好请求、
        // provider 装配的宿主钩子）。令牌登记晚于 close 时，`cancel_turn` 落在
        // `current_cancel: None` 上是一次 no-op——被 close 点名的运行中输入反而
        // 照常跑完，drain 等不到终态（methods 的 close-drain 用例因此挂死）。
        // 这里提前登记后，close 的取消必然命中本输入（终态 `cancelled`）；
        // 排队中的后续输入照常执行（裁决 4 不变）。
        let cancel = if item.goal.is_some() {
            shared.lock().goal_cancel.clone().unwrap_or_default()
        } else {
            CancelToken::new()
        };
        {
            let mut live = shared.lock();
            live.status = RuntimeStatus::Running;
            live.current_cancel = Some(cancel.clone());
            live.current_kind = Some(if item.compact.is_some() {
                TurnKind::Compact
            } else if item.goal.is_some() {
                TurnKind::GoalContinuation
            } else {
                TurnKind::Prompt
            });
            // 通知 turn 没有 submit 的受理步骤；按普通输入的口径补一次 revision
            // （`background-idle` 的 4 = 用户 turn 2 + 通知 turn 2）。
            if is_notification_turn {
                live.revision += 1;
                live.updated_at = now_ms();
            }
            // compact 任务：先记「已取到」（相位仍可能是 Queued——provider 装配窗口内
            // 第二个 compact 仍会被受理为排队，见 `CompactCoordinator` 注释）。
            if item.compact.is_some() {
                live.compact.on_pickup();
            }
        }
        if let Some(reverse) = &reverse {
            if !user_execution_requested {
                user_execution_requested = true;
                let preferences = reverse
                    .request_runtime_preferences_scoped(&shared.record.id, "user-execution")
                    .await;
                *shared
                    .preferences
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(preferences);
            }
        }
        // 每个 turn 开始前同步配置：mode/能力进系统提示，模型事实决定装配。
        let config = shared.read_config();
        session.set_mode(config.mode.clone());
        session.set_model_capabilities(config.capabilities);

        let generation = source.cache_key();
        let cached = current
            .as_ref()
            .filter(|(provider_id, model_id, cached_generation, _)| {
                provider_id == &config.provider_id
                    && model_id == &config.model_id
                    && cached_generation == &generation
            })
            .map(|(_, _, _, provider)| Arc::clone(provider));
        let provider = match cached {
            Some(provider) => Some(provider),
            None if config.provider_id.is_empty() || config.model_id.is_empty() => None,
            None => {
                // 阻塞解析放阻塞线程：宿主钩子的等待不占 tokio worker（Task 8）。
                let context = Some(ProviderRequestContext {
                    session_id: shared.record.id.clone(),
                    workspace_path: shared.record.directory.clone(),
                    workspace_identity: shared.record.workspace_id.clone(),
                });
                let blocking_source = Arc::clone(&source);
                let provider_id = config.provider_id.clone();
                let model_id = config.model_id.clone();
                match tokio::task::spawn_blocking(move || {
                    blocking_source.resolve(&provider_id, &model_id, context.as_ref())
                })
                .await
                {
                    Ok(Ok(provider)) => {
                        current = Some((
                            config.provider_id.clone(),
                            config.model_id.clone(),
                            generation,
                            Arc::clone(&provider),
                        ));
                        Some(provider)
                    }
                    Ok(Err(message)) => {
                        current = None;
                        if item.goal.is_some() {
                            run_goal_configuration_failure(&shared, &store, &item, &message).await;
                        } else {
                            run_configuration_failure(
                                &mut session,
                                &shared,
                                &store,
                                &item,
                                &message,
                                cancel,
                            )
                            .await;
                        }
                        continue;
                    }
                    Err(join_error) => {
                        current = None;
                        let message = format!("provider 装配任务失败: {join_error}");
                        if item.goal.is_some() {
                            run_goal_configuration_failure(&shared, &store, &item, &message).await;
                        } else {
                            run_configuration_failure(
                                &mut session,
                                &shared,
                                &store,
                                &item,
                                &message,
                                cancel,
                            )
                            .await;
                        }
                        continue;
                    }
                }
            }
        };
        match provider {
            Some(provider) if item.compact.is_some() => {
                session = run_compact(session, &shared, &store, provider, item).await;
            }
            Some(provider) if item.goal.is_some() => {
                session = run_goal_job(session, &shared, &store, provider, item).await;
            }
            Some(provider) => {
                session = run_one_input(session, &shared, &store, provider, item, cancel).await;
            }
            // 没有配置模型：显式配置错误，不启动模型请求（compact 任务同样走这支，
            // 终态是带 `"/compact …"` 展示串的 `turn.failed`；协调器在收口处复位）。
            None => {
                if item.goal.is_some() {
                    run_goal_configuration_failure(
                        &shared,
                        &store,
                        &item,
                        "Select a model before continuing",
                    )
                    .await;
                } else {
                    run_configuration_failure(
                        &mut session,
                        &shared,
                        &store,
                        &item,
                        "Select a model before continuing",
                        cancel,
                    )
                    .await;
                }
            }
        }
        // 普通 prompt/compact/配置失败路径没有 goal 的 before_terminal 钩子；stop 在
        // 这些 turn 中也可能预分配了 status 事件。这里统一 drain，避免 pending 事件
        // 形成 seq 缺号并卡住后续日志/广播。
        publish_pending_cancel_event(&shared);
    }
    // 队列清空且发送端关闭：广播 drain 完成信号（closing 台账据此移除条目）。
    let _ = shared.drained.send(true);
}

/// worker 退出结算守卫：正常路径显式发送 `true`，panic 时由 Drop 兜底发送。
struct DrainGuard(tokio::sync::watch::Sender<bool>);

impl Drop for DrainGuard {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

/// 配置失败的 turn：`turn.started` → `turn.failed`（`CONFIGURATION_ERROR`），
/// `turn_usage(failed)` 记零值（没有任何模型请求可累计）。终态恰好一条，
/// 与正常 turn 走同一套 publish / 状态收口。取消令牌由 worker 在取到输入时登记
/// （本路径的 turn 即刻失败，令牌只随 `current_cancel` 清空）。
///
/// compact 任务的配置失败（C2 T2）：`turn.started` 用 `"/compact …"` 展示串且
/// **不写 user message 行**（compact 没有 `session_input`/user 行，契约 §1.3）；
/// 协调器按「失败不结算」复位（排队中的下一单仍完整执行）。
async fn run_configuration_failure(
    session: &mut Session,
    shared: &Arc<Shared>,
    store: &Arc<SessionStore>,
    item: &QueuedInput,
    message: &str,
    cancel: CancelToken,
) {
    let _ = cancel;
    let mut recorder = TurnRecorder::new(Arc::clone(shared), Arc::clone(store), item);
    let started = if item.compact.is_some() {
        session.begin_compact_turn(&item.input.content, &item.input_id)
    } else if !item.notifications.is_empty() {
        session.begin_background_notification_turn(
            &item.input.content,
            &item.input_id,
            &item.notifications,
        )
    } else {
        session.begin_turn(&item.input.content)
    };
    let turn_id = started.turn_id.clone();
    recorder.emit(started);
    // 先落 usage 行再发布终态（最终评审 I3）：订阅者看到 `turn.failed` 时，
    // `turn_usage` 必须已经可读。
    recorder.record_failure_progress(&agent_core::TurnProgress::default(), None);
    recorder.emit(session.emit_turn_failed(
        turn_id.as_deref(),
        "CONFIGURATION_ERROR",
        message,
        "configuration",
    ));

    let mut live = shared.lock();
    live.current_cancel = None;
    live.current_kind = None;
    live.turn_count += 1;
    live.revision += 1;
    live.updated_at = now_ms();
    if item.compact.is_some() {
        // compact 的配置失败没有进入 `run_compact`，也**从未进入 `CommandInbox`**
        // （compact 不走 prompt 的 admission 账本）。这里不能调用
        // `inbox.complete(compact_input_id)`：它必然返回 `Stale`，若按 Stale
        // 「状态不动」会把 ended 的 compact 永远留在 Running。显式落 Error，
        // 与普通 turn 的配置失败同口径；失败不改变上下文，`settled=false` 让
        // 排队中的下一单照常完整执行。
        live.compact.on_settled(false);
        live.status = RuntimeStatus::Error;
    } else if !item.notifications.is_empty() {
        // 后台通知 turn 不进入 CommandInbox；配置失败同样按错误收口，不做对账。
        live.status = RuntimeStatus::Error;
    } else {
        // 普通 turn：与 `run_one_input` 的失败分支同形（失败恒 Error；Stale 不动
        // 状态），保证两条收口路径的口径一致。
        let completion = live.inbox.complete(&shared.record.id, &item.input_id);
        live.status = match completion {
            Completion::Promoted(_) | Completion::Idle => RuntimeStatus::Error,
            Completion::Stale => live.status,
        };
    }
    live.last_error = Some(message.to_string());
}

/// 跑一个输入（串行；前后维护 running 状态与取消令牌）。
///
/// turn 执行体**连同终态收口**在 `spawn_blocking` 线程上经 `Handle::block_on` 驱动
/// （Task 8 控制器裁决 1）：同步 [`PermissionHandler`] 的审批等待（反向请求）会
/// **阻塞**当前线程，而 stdio 单写者循环必须留在 tokio worker 上继续写帧/读应答——
/// 阻塞线程 + runtime 线程分工，current-thread 测试 runtime 下也不死锁
/// （`spawn_blocking` 的线程不进 runtime 的 worker 池，`block_on` 在其上合法）。
///
/// **终态事件与收口的可观察顺序**：终态（`turn.completed`/`turn.failed`）在
/// `run_turn` 内发布（阻塞线程），收口（Live 记账 / 状态回 Idle/Error）在其**之后**
/// 的同一阻塞线程上进行——订阅者与其他线程并行，因此**可能短暂看到「终态已发布、
/// 快照仍 Running」的窗口**（Task 8 前 turn 与订阅者同线程串行，窗口不可观察）。
/// 收口与 turn 同闭包保证的是串行语义：收口完成后 worker 才取下一条已接受的输入
/// （`CommandInbox` 对账不迟到）。事件与落库的顺序由 [`TurnRecorder`] 的
/// 「先落库后发布」单独保证（事件可见 ⇒ 行已提交）。断言「终态后状态」的测试
/// 必须等收口：`tests/runtime.rs` 的 `wait_settled`（bounded 轮询快照离开 Running）。
///
/// 终态收口：`run_turn` 的 `Err(cancelled)` 表示终态已由它自己发出
/// （`turn.completed`，`resultType:"cancelled"`），worker 不补发；其余 `Err` 由
/// 本闭包补发唯一的 `turn.failed`。成功与取消路径的终态事件经 recorder 走同一套
/// persist 与 publish。返回会话（阻塞任务的所有权往返：Session/recorder 移入、
/// 移出）。
async fn run_one_input(
    session: Session,
    shared: &Arc<Shared>,
    store: &Arc<SessionStore>,
    provider: Arc<dyn ModelProvider>,
    item: QueuedInput,
    cancel: CancelToken,
) -> Session {
    let token = cancel;

    let recorder = TurnRecorder::new(Arc::clone(shared), Arc::clone(store), &item);
    let content = item.input.content.clone();
    let input_id = item.input_id.clone();
    let notifications = item.notifications.clone();
    let is_notification_turn = !notifications.is_empty();
    let session_id = shared.record.id.clone();
    let shared_for_turn = Arc::clone(shared);
    let handle = tokio::runtime::Handle::current();
    let (session, _recorder) = tokio::task::spawn_blocking(move || {
        handle.block_on(async move {
            // 捕获进 async block 后重绑 `mut`：run_turn 需要 `&mut self` / `&mut sink`。
            let mut session = session;
            let mut recorder = recorder;
            // C2 T3：本 turn 的项目记忆段。`memoryEnabled` 来自宿主对
            // `session/requestRuntimePreferences` 的缓存应答（无 hub / 未应答时按
            // 保守默认关闭）；读取是同步文件 IO，已在本阻塞线程上。任何失败
            // fail-soft：不注入、turn 继续（`agent-c2-contract.md` §3）；失败原因
            // 覆盖写进 `last_memory_error`（诊断面，失败路径线上静默——见该字段
            // 与 [`SessionRuntime::last_memory_error`] 的注释）。
            let memory_enabled = shared_for_turn
                .preferences
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .map(|preferences| preferences.memory_enabled)
                .unwrap_or(false);
            let memory_load = shared_for_turn
                .read_config()
                .data_root
                .as_deref()
                .map(|data_root| {
                    crate::memory::load_memory_section(
                        data_root,
                        &shared_for_turn.record.directory,
                        memory_enabled,
                    )
                })
                .unwrap_or(crate::memory::MemoryLoad {
                    memory: None,
                    error: None,
                });
            session.set_memory(memory_load.memory);
            *shared_for_turn
                .last_memory_error
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = memory_load.error;
            let result = if is_notification_turn {
                session
                    .run_background_notification_turn(
                        &content,
                        &input_id,
                        &notifications,
                        provider.as_ref(),
                        &mut recorder,
                        &token,
                    )
                    .await
            } else {
                session
                    .run_turn(&content, provider.as_ref(), &mut recorder, &token)
                    .await
            };

            // —— 终态收口（与旧内联路径逐行同序，只是在阻塞线程上）——
            let failure_message = match &result {
                Ok(()) => None,
                // 取消：终态已由 run_turn 发出，不再补第二个终态；占位行由
                // `EventSink::cancelled_placeholder` 在终态发布前落库。
                Err(error) if error.is_cancelled() => None,
                Err(error) => {
                    // Task 7 控制器裁决 6：失败 turn 的 usage 行记**已知**累计值，不清零。
                    // 先落 usage 行再发布终态（最终评审 I3）：看到 `turn.failed` 的
                    // 订阅者读 `turn_usage` 时必须已经有这一行。
                    recorder.record_failure_progress(
                        &error.progress,
                        Some(("unknown_error", "UNKNOWN_ERROR")),
                    );
                    // provider 错误走分类投影（attribution.statusCode/retryable）；
                    // 没有分类事实的 TurnError 保持旧的 runtime_error 形状。
                    if error
                        .provider_error
                        .as_ref()
                        .is_some_and(|provider_error| provider_error.is_auth())
                    {
                        recorder.persist_failed_placeholder();
                    }
                    let event = match &error.provider_error {
                        Some(provider_error) => session.emit_turn_failed_for_provider(
                            recorder.turn_id().map(String::as_str),
                            provider_error,
                            "processing_input",
                        ),
                        None => session.emit_turn_failed(
                            recorder.turn_id().map(String::as_str),
                            "runtime_error",
                            &error.message,
                            "model",
                        ),
                    };
                    recorder.emit(event);
                    Some(error.message.clone())
                }
            };
            let mut live = shared_for_turn.lock();
            live.current_cancel = None;
            live.current_kind = None;
            shared_for_turn
                .tool_execution
                .store(false, Ordering::SeqCst);
            // running 完成对账；迟到的 complete 由 CommandInbox 自己挡（Stale 不动队列）。
            // 通知 turn 从未进入 CommandInbox，不能对它调用 complete（否则 Stale）。
            let completion = if is_notification_turn {
                None
            } else {
                Some(live.inbox.complete(&session_id, &input_id))
            };
            live.turn_count += 1;
            live.revision += 1;
            live.updated_at = now_ms();
            live.total_token_count += recorder.turn_total_tokens();
            // Task 7 控制器裁决 6：队首被提升（还有已接受输入在等）时不能闪回 idle。
            live.status = match (&failure_message, completion) {
                (Some(_), _) => RuntimeStatus::Error,
                (None, Some(Completion::Promoted(_))) => RuntimeStatus::Running,
                (None, Some(Completion::Idle)) => RuntimeStatus::Idle,
                // Stale：对账被拒（不应发生——worker 是唯一 complete 的写入方）；状态不动。
                (None, Some(Completion::Stale)) => live.status,
                // 通知 turn：队列里还有已接受输入时保持 Running，否则回 idle。
                (None, None) => {
                    if live.inbox.running(&session_id).is_some() {
                        RuntimeStatus::Running
                    } else {
                        RuntimeStatus::Idle
                    }
                }
            };
            if let Some(message) = failure_message.or(recorder.persist_error.clone()) {
                live.last_error = Some(message);
            }
            (session, recorder)
        })
    })
    .await
    .expect("turn 执行任务不应 panic");
    session
}

/// compact 成功时 `turn.completed.response` 的观察值（契约 §1.2）。
const COMPACTED_RESPONSE: &str = "Compacted";
/// compact `skipped` 终态的观察值（契约 §1.6 trial 4）。
const COMPACT_SKIPPED_RESPONSE: &str = "Context is up to date; no compression needed";

/// 跑一个 compact 任务（与 turn 同一 worker 队列串行执行）。
///
/// 事件面（契约 §1.2/§1.6/§1.8 的可产出子集）：`turn.started` → 成功
/// `turn.completed("Compacted")` / 失败 `turn.failed` / 跳过
/// `turn.completed("Context is up to date…")`。oracle 的 `session.updated` 推送
/// （状态、`model_request_*`、边界）不在 Rust 产出面——`session.updated` 是 C1 起的
/// 登记缺省事件类型，登记在契约「Rust C2 现状」；`model.streaming` 同样不产出
/// （观察的 compact 事件序列里没有它）。
///
/// 执行体与 [`run_one_input`] 同构：连同落库收口放在 `spawn_blocking` 线程上经
/// `Handle::block_on` 驱动（落库是同步 sqlite 短事务，不占 tokio worker）。
/// **不响应取消**：compact 没有观察到的取消语义（契约 §6 未记录），`close` 通过
/// 等待 compact 自然完成来 drain。
async fn run_compact(
    session: Session,
    shared: &Arc<Shared>,
    store: &Arc<SessionStore>,
    provider: Arc<dyn ModelProvider>,
    item: QueuedInput,
) -> Session {
    let handle = tokio::runtime::Handle::current();
    let shared_for_compact = Arc::clone(shared);
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || {
        handle.block_on(run_compact_blocking(
            session,
            shared_for_compact,
            store,
            provider,
            item,
        ))
    })
    .await
    .expect("compact 执行任务不应 panic")
}

/// compact 执行体（阻塞线程上的 async 主体；见 [`run_compact`]）。
async fn run_compact_blocking(
    mut session: Session,
    shared: Arc<Shared>,
    store: Arc<SessionStore>,
    provider: Arc<dyn ModelProvider>,
    item: QueuedInput,
) -> Session {
    let job = item
        .compact
        .clone()
        .expect("run_compact 只处理 compact 队列项");
    let display = item.input.content.clone();
    let session_id = shared.record.id.clone();
    let started_at = now_ms();
    let wall_started = std::time::Instant::now();

    // turn.started（不写行）→ 同一把锁里切入 Running（`already_running` 判定点）
    // 并在**开始时**推进 turnCount（观察：already_running 应答的 projection.turnCount
    // 已含本 compact turn，契约 §1.6 trial 1；终态不再 +1）。
    let started = session.begin_compact_turn(&display, &job.input_id);
    let turn_id = started
        .turn_id
        .clone()
        .expect("begin_compact_turn 已设置 turn id");
    {
        let mut live = shared.lock();
        live.compact.on_started(&turn_id);
        live.turn_count += 1;
        live.updated_at = started_at;
    }
    shared.publish(&started);

    // —— 事实采集（终态行与判定的输入）——
    let history = session.messages().to_vec();
    let pre_tokens = estimate_history_tokens(&history);
    let summarized_count = history.len() as u64;
    let prior_tool_history = history
        .iter()
        .any(|message| matches!(message, ModelMessage::ToolResult { .. }));
    let config = shared.read_config();
    let last_summarized_message_id = store
        .messages(&session_id, None, Some(1))
        .ok()
        .and_then(|rows| rows.last().map(|row| row.message.id.clone()));
    // 排队期间已有 compact 结算 → 本次按观察到的 `skipped` 形状执行（单飞状态机的
    // 第二种真实结果；触发时序依赖登记在契约「Rust C2 现状」）。
    let skip = {
        let live = shared.lock();
        live.compact.skip_due(job.settled_at_enqueue)
    };

    let facts = CompactFacts {
        session_id: session_id.clone(),
        directory: shared.record.directory.clone(),
        task_type: shared.record.task_type.clone(),
        turn_id: turn_id.clone(),
        input_id: job.input_id.clone(),
        operation_id: fresh_id("op"),
        boundary_id: fresh_id("boundary"),
        started_at,
        ended_at: now_ms(),
        pre_tokens,
        post_tokens: 0,
        summarized_count,
        custom_instructions: job.instructions.is_some(),
        provider_id: config.provider_id.clone(),
        model_id: config.model_id.clone(),
        mode: config.mode.clone(),
        thought_level: config.thought_level.clone(),
        last_summarized_message_id,
        summary_message_id: None,
    };

    if skip {
        return finish_compact_skipped(
            session,
            shared,
            store,
            facts,
            &job,
            prior_tool_history,
            wall_started,
        );
    }

    // —— 一次模型请求（不产出事件；compact 的模型输出不走 model.streaming）——
    let mut request_messages = history;
    request_messages.push(ModelMessage::User(compact_prompt(
        job.instructions.as_deref(),
    )));
    let request = session.build_request(&request_messages);
    let model_started_at = now_ms();
    let (delta_sink, mut deltas) = tokio::sync::mpsc::unbounded_channel();
    let mut provider_future = provider.stream(request, delta_sink);
    let mut provider_result = None;
    let mut delta_channel_open = true;
    let mut text = String::new();
    let mut tool_call_count = 0_u64;
    // 只数 `ModelDelta::ToolCall`（完成态调用）：适配器还会发 `ToolInputStart` 等
    // 过程 delta，按过程数会翻倍。
    let absorb = |delta: Option<ModelDelta>,
                  text: &mut String,
                  tool_call_count: &mut u64,
                  open: &mut bool| {
        match delta {
            Some(ModelDelta::Text { text: chunk }) => text.push_str(&chunk),
            Some(ModelDelta::ToolCall { .. }) => *tool_call_count += 1,
            None => *open = false,
            _ => {}
        }
    };
    while provider_result.is_none() {
        tokio::select! {
            biased;
            maybe_delta = deltas.recv(), if delta_channel_open => {
                absorb(maybe_delta, &mut text, &mut tool_call_count, &mut delta_channel_open);
            }
            result = &mut provider_future => provider_result = Some(result),
        }
    }
    while let Ok(delta) = deltas.try_recv() {
        absorb(
            Some(delta),
            &mut text,
            &mut tool_call_count,
            &mut delta_channel_open,
        );
    }
    let model_finished_at = now_ms();

    let response = match provider_result.expect("循环只在 provider future 结束后退出") {
        Ok(response) => response,
        // 网络层 provider 失败未观察到（契约 §6）：按 MODEL_ERROR + provider 文案、
        // retryable=true 落失败面（登记在契约「Rust C2 现状」）。
        Err(error) => {
            return compact_failure(
                session,
                shared,
                store,
                facts,
                &job,
                CompactFailure {
                    message: &error.message,
                    retryable: true,
                    model_response: None,
                    model_started_at,
                },
            );
        }
    };
    let model_response = CompactModelResponse {
        usage: response.usage.clone(),
        stop_reason: response.stop_reason.to_string(),
        tool_call_count,
        started_at: model_started_at,
        finished_at: model_finished_at,
    };
    let summary = match validate_compact_response(tool_call_count as usize, &text) {
        Ok(summary) => summary,
        Err(error) => {
            return compact_failure(
                session,
                shared,
                store,
                facts,
                &job,
                CompactFailure {
                    message: error.message(),
                    retryable: error.retryable(),
                    model_response: Some(model_response),
                    model_started_at,
                },
            );
        }
    };

    // —— 成功：替换内存历史 → 落库 → 发布终态 → 收口 ——
    // 下一轮请求只带摘要续接消息（契约 §1.5：旧 turn 消息不再进入请求）；DB 里的
    // 完整历史保留（回退属 C3）。
    session.replace_history(vec![ModelMessage::User(summary_continuation(&summary))]);
    let summary_message_id = fresh_id("msg");
    let post_tokens = model_response.usage.input_tokens + model_response.usage.output_tokens;
    let facts = CompactFacts {
        summary_message_id: Some(summary_message_id),
        post_tokens,
        // 落库时间取**终态时刻**（facts 构造时只有开始时间；模型请求期间的墙钟
        // 进度属于行事实，不能拿受理时的快照冒充完成时间）。
        ended_at: now_ms(),
        ..facts
    };
    // `runtime.contextUsage` 的事实面覆盖所有模型请求：compact 也是最近一次请求
    // （`used` = provider total），与 oracle 的 read 视图同口径。
    shared
        .lock()
        .usage
        .observe(&RequestUsage::from_model_usage(&model_response.usage));
    let mut persist_error: Option<String> = None;
    if let Err(error) = persist_compact_rows(
        &shared,
        &store,
        &facts,
        CompactRows::Success { summary: &summary },
    ) {
        persist_error = Some(error.to_string());
    }
    if let Err(error) = record_compact_success_usage(&store, &facts, &model_response) {
        persist_error.get_or_insert_with(|| error.to_string());
    }
    if let Err(error) = store.touch_session(&session_id, facts.ended_at) {
        persist_error.get_or_insert_with(|| error.to_string());
    }

    let payload = TurnCompletedPayload {
        response: COMPACTED_RESPONSE.to_string(),
        token_count: post_tokens,
        usage: Some(json!({
            "source": "provider",
            "modelRequestCount": 1,
            "inputTokens": model_response.usage.input_tokens,
            "outputTokens": model_response.usage.output_tokens,
            "totalTokens": post_tokens,
            "cacheReadTokens": model_response.usage.cache_read_tokens,
            "cacheWriteTokens": model_response.usage.cache_write_tokens,
            "reasoningTokens": model_response.usage.reasoning_tokens,
            "webFetchRequests": 0,
            "webSearchRequests": 0,
        })),
        tool_call_count: 0,
        history_round_count: Some(1),
        duration: wall_started.elapsed().as_millis() as f64,
        cache_stats: Some(compact_cache_stats(
            session.messages().len(),
            prior_tool_history,
        )),
        input_id: Some(job.input_id.clone()),
        result_type: TurnResultType::Success,
        background_subagent_result_consumed: None,
        extra: ExtraFields::new(),
    };
    let event = session.emit(Some(&turn_id), SessionEventType::TurnCompleted, &payload);
    shared.publish(&event);
    settle_compact(
        &shared,
        &session_id,
        CompactSettle::Success {
            token_count: post_tokens,
            persist_error,
        },
    );
    session
}

/// `skipped` 终态：不发模型请求、不写摘要行；timeline 消息按观察形状落
/// `finish:"skipped"` + `timelineStatus:"skipped"`（契约 §1.6 trial 4 的
/// `dbSkippedRows`；timeline part 的形状按其余两种终态同构推导，登记在契约注记）。
fn finish_compact_skipped(
    mut session: Session,
    shared: Arc<Shared>,
    store: Arc<SessionStore>,
    facts: CompactFacts,
    job: &CompactJob,
    prior_tool_history: bool,
    wall_started: std::time::Instant,
) -> Session {
    let facts = CompactFacts {
        ended_at: now_ms(),
        ..facts
    };
    let session_id = facts.session_id.clone();
    let turn_id = facts.turn_id.clone();
    let pre_tokens = facts.pre_tokens;
    let mut persist_error: Option<String> = None;
    if let Err(error) = persist_compact_rows(&shared, &store, &facts, CompactRows::Skipped) {
        persist_error = Some(error.to_string());
    }
    // 观察（`compact-same-tick` 的 `dbSkippedRows`）只新增 skipped timeline 消息 +
    // compact part，**没有** skipped `turn_usage` 行；这里不写（§1.9 登记）。
    if let Err(error) = store.touch_session(&session_id, facts.ended_at) {
        persist_error.get_or_insert_with(|| error.to_string());
    }

    let payload = TurnCompletedPayload {
        response: COMPACT_SKIPPED_RESPONSE.to_string(),
        token_count: pre_tokens,
        usage: None,
        tool_call_count: 0,
        history_round_count: Some(1),
        duration: wall_started.elapsed().as_millis() as f64,
        cache_stats: Some(compact_cache_stats(
            session.messages().len(),
            prior_tool_history,
        )),
        input_id: Some(job.input_id.clone()),
        result_type: TurnResultType::Success,
        background_subagent_result_consumed: None,
        extra: ExtraFields::new(),
    };
    let event = session.emit(Some(&turn_id), SessionEventType::TurnCompleted, &payload);
    shared.publish(&event);
    settle_compact(
        &shared,
        &session_id,
        CompactSettle::Skipped {
            token_count: pre_tokens,
            persist_error,
        },
    );
    session
}

/// compact 失败的输入事实（避免 9 参数函数；`model_response` 只在 provider 有应答时
/// 存在——传输失败没有 usage 行）。
struct CompactFailure<'a> {
    message: &'a str,
    retryable: bool,
    model_response: Option<CompactModelResponse>,
    model_started_at: i64,
}

/// compact 失败收口：历史不动；failed timeline 行 +（有应答时）`model_usage` 行 +
/// `turn_usage(error)` 行 → `turn.failed` → 不结算复位（会话仍可用，契约 §1.8）。
fn compact_failure(
    mut session: Session,
    shared: Arc<Shared>,
    store: Arc<SessionStore>,
    facts: CompactFacts,
    job: &CompactJob,
    failure: CompactFailure<'_>,
) -> Session {
    let CompactFailure {
        message,
        retryable,
        model_response,
        model_started_at,
    } = failure;
    let facts = CompactFacts {
        ended_at: now_ms(),
        ..facts
    };
    let session_id = facts.session_id.clone();
    let turn_id = facts.turn_id.clone();
    let mut persist_error: Option<String> = None;
    if let Err(error) = persist_compact_rows(&shared, &store, &facts, CompactRows::Failed) {
        persist_error = Some(error.to_string());
    }
    if let Err(error) =
        record_compact_failure_usage(&store, &facts, model_response.as_ref(), model_started_at)
    {
        persist_error.get_or_insert_with(|| error.to_string());
    }
    if let Err(error) = store.touch_session(&session_id, facts.ended_at) {
        persist_error.get_or_insert_with(|| error.to_string());
    }

    let payload = TurnFailedPayload {
        error: ProtocolErrorDetail {
            // 观察（契约 §1.8）：`type` 恒 `unknown_error`、`code` 恒 `MODEL_ERROR`、
            // `detail` 恒 `Compact failed`、retryable 只出现在 `attribution` 里。
            error_type: "unknown_error".to_string(),
            message: message.to_string(),
            stack: None,
            code: Some("MODEL_ERROR".to_string()),
            detail: Some("Compact failed".to_string()),
            underlying_error_message: Some(message.to_string()),
            underlying_error_detail: None,
            attribution: Some(json!({ "retryable": retryable })),
            retryable: None,
            data: None,
            extra: ExtraFields::new(),
        },
        turn_phase: "compact".to_string(),
        input_id: Some(job.input_id.clone()),
        background_subagent_result_consumed: None,
        extra: ExtraFields::new(),
    };
    let event = session.emit(Some(&turn_id), SessionEventType::TurnFailed, &payload);
    shared.publish(&event);
    settle_compact(
        &shared,
        &session_id,
        CompactSettle::Failure {
            message: message.to_string(),
            persist_error,
        },
    );
    session
}

/// compact 终态的共享收口（与 `run_one_input` 的锁内收口同构；`turnCount` 已在
/// 开始时推进，这里不再 +1）。
enum CompactSettle {
    Success {
        token_count: u64,
        persist_error: Option<String>,
    },
    Skipped {
        token_count: u64,
        persist_error: Option<String>,
    },
    Failure {
        message: String,
        persist_error: Option<String>,
    },
}

fn settle_compact(shared: &Arc<Shared>, session_id: &str, outcome: CompactSettle) {
    let mut live = shared.lock();
    live.current_cancel = None;
    live.current_kind = None;
    match outcome {
        CompactSettle::Success {
            token_count,
            persist_error,
        }
        | CompactSettle::Skipped {
            token_count,
            persist_error,
        } => {
            // 成功与 skipped 都算结算（失败与配置失败不计，见 `CompactCoordinator`）。
            live.compact.on_settled(true);
            live.total_token_count += token_count;
            if let Some(error) = persist_error {
                live.last_error = Some(error);
            }
            // 排队期间被接受的 prompt 已在 CommandInbox 里（compact 不进 inbox），
            // 这里按「还有已接受输入」保持 Running，避免闪回 Idle。
            live.status = if live.inbox.running(session_id).is_some() {
                RuntimeStatus::Running
            } else {
                RuntimeStatus::Idle
            };
        }
        CompactSettle::Failure {
            message,
            persist_error,
        } => {
            live.compact.on_settled(false);
            live.status = RuntimeStatus::Error;
            // 与 `run_one_input` 同口径：失败文案优先，落库失败次之。
            live.last_error = Some(persist_error.unwrap_or(message));
        }
    }
    live.revision += 1;
    live.updated_at = now_ms();
}

// ---------------------------------------------------------------------------
// C3 T1：goal 状态投影 / 落库 / worker 执行
// ---------------------------------------------------------------------------

/// `GoalTarget` → `session_target` 行。
fn target_record(target: &GoalTarget) -> TargetRecord {
    TargetRecord {
        session_id: target.session_id.clone(),
        target_id: target.target_id.clone(),
        objective: target.objective.clone(),
        status: target.status.as_str().to_string(),
        token_budget: target.token_budget.map(|value| value as i64),
        tokens_used: target.tokens_used as i64,
        time_used_seconds: target.time_used_seconds as i64,
        time_created: target.time_created,
        time_updated: target.time_updated,
        summary_title: target.summary_title.clone(),
        active_input_id: target.active_input_id.clone(),
        active_run_started_at: target.active_run_started_at,
        active_run_last_seen_at: target.active_run_last_seen_at,
    }
}

/// `session_target` 行 → `GoalTarget`。未知 status 按 `paused` 落（fail-closed：
/// 不把坏行当成可续跑的 active）。
fn goal_target_from_record(record: TargetRecord) -> GoalTarget {
    GoalTarget {
        session_id: record.session_id,
        target_id: record.target_id,
        objective: record.objective,
        status: GoalStatus::parse(&record.status).unwrap_or(GoalStatus::Paused),
        token_budget: record
            .token_budget
            .and_then(|value| u64::try_from(value).ok()),
        tokens_used: record.tokens_used.max(0) as u64,
        time_used_seconds: record.time_used_seconds.max(0) as u64,
        time_created: record.time_created,
        time_updated: record.time_updated,
        summary_title: record.summary_title,
        active_input_id: record.active_input_id,
        active_run_started_at: record.active_run_started_at,
        active_run_last_seen_at: record.active_run_last_seen_at,
    }
}

/// `GoalTarget` → 快照 / 协议 `SessionGoal`（camelCase 键）。
fn goal_target_protocol(target: &GoalTarget) -> SessionGoal {
    SessionGoal {
        session_id: target.session_id.clone(),
        target_id: target.target_id.clone(),
        objective: target.objective.clone(),
        summary_title: Some(target.summary_title.clone()),
        status: match target.status {
            GoalStatus::Active => SessionGoalStatus::Active,
            GoalStatus::Paused => SessionGoalStatus::Paused,
            GoalStatus::BudgetLimited => SessionGoalStatus::BudgetLimited,
            GoalStatus::Complete => SessionGoalStatus::Complete,
        },
        token_budget: target.token_budget,
        tokens_used: target.tokens_used,
        time_used_seconds: target.time_used_seconds,
        active_input_id: Some(target.active_input_id.clone()),
        active_run_started_at_ms: Some(target.active_run_started_at.map(|value| value as u64)),
        active_run_last_seen_at_ms: Some(target.active_run_last_seen_at.map(|value| value as u64)),
        created_at: target.time_created.max(0) as u64,
        updated_at: target.time_updated.max(0) as u64,
    }
}

/// `session.updated` 的 target 事件形状（观察键：`sessionID`/`targetID`/`time`）。
fn target_event_json(target: &GoalTarget) -> Value {
    json!({
        "sessionID": target.session_id,
        "targetID": target.target_id,
        "objective": target.objective,
        "summaryTitle": target.summary_title,
        "status": target.status.as_str(),
        "tokenBudget": target.token_budget,
        "tokensUsed": target.tokens_used,
        "timeUsedSeconds": target.time_used_seconds,
        "activeInputId": target.active_input_id,
        "activeRunStartedAtMs": target.active_run_started_at,
        "activeRunLastSeenAtMs": target.active_run_last_seen_at,
        "time": {
            "created": target.time_created,
            "updated": target.time_updated,
        }
    })
}

/// `session.updated` 的 goal action 载荷。
fn target_update_payload(
    action: &str,
    source: &str,
    previous: Option<&GoalTarget>,
    target: Option<&GoalTarget>,
) -> Value {
    json!({
        "action": action,
        "source": source,
        "previousTarget": previous.map(target_event_json).unwrap_or(Value::Null),
        "target": target.map(target_event_json).unwrap_or(Value::Null),
    })
}

/// 生效 inputId（请求缺省/空白时生成）。
fn goal_input_id(provided: Option<&str>) -> String {
    provided
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| fresh_id("in"))
}

/// `resume` 的响应文案（观察值是 `Goal resumed` 前缀 + 同构字段）。
fn goal_resumed_response(target: &GoalTarget) -> String {
    format!(
        "Goal resumed\nObjective: {}\nUsage: {} tokens / {}\nTime: {} seconds",
        target.objective,
        target.tokens_used,
        target.budget_label(),
        target.time_used_seconds
    )
}

/// `goalVerifications` 的最近窗口（观察 loop 夹具 `length:2`）。
fn recent_verifications(timeline: &[GoalVerificationTimeline]) -> Vec<GoalVerification> {
    timeline
        .iter()
        .filter(|item| item.status == GoalVerificationStatus::Completed)
        .filter_map(|item| item.verification.clone())
        .rev()
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// 冷恢复：目标行 + 验证 entry → 热状态。
fn load_goal_state(
    store: &SessionStore,
    session_id: &str,
) -> Result<GoalState, session_store::StoreError> {
    let (timeline, verifications) = load_verification_state(store, session_id)?;
    let Some(record) = store.get_target(session_id)? else {
        return Ok(GoalState {
            timeline,
            verifications,
            ..GoalState::default()
        });
    };
    let target = goal_target_from_record(record);
    let mut restored = GoalState::restore(target, timeline);
    if let Some(target) = &restored.target {
        // active → paused / 清 active 字段必须落库（观察 §1.8 的 read/dbAfterRestart）。
        let current = store.get_target(session_id)?.map(goal_target_from_record);
        let changed = current
            .as_ref()
            .map(|current| {
                current.status != target.status
                    || current.active_input_id.is_some()
                    || current.active_run_started_at.is_some()
                    || current.active_run_last_seen_at.is_some()
            })
            .unwrap_or(true);
        if changed {
            let persisted = restored.target.as_mut().expect("上面已判定 Some");
            persisted.time_updated = now_ms();
            store.upsert_target(&target_record(persisted))?;
        }
    }
    Ok(restored)
}

/// 冷读面的 goal 投影（`session/read` 无活跃 runtime 时用；不拉起 runtime）。
pub(crate) type GoalProjection = (
    Option<SessionGoal>,
    Vec<GoalVerification>,
    Vec<GoalVerificationTimeline>,
);

pub(crate) fn load_goal_projection(
    store: &SessionStore,
    session_id: &str,
) -> Result<GoalProjection, session_store::StoreError> {
    let goal = load_goal_state(store, session_id)?;
    Ok((
        goal.target.as_ref().map(goal_target_protocol),
        goal.verifications,
        goal.timeline,
    ))
}

/// 从 `session_entry(type = target_completion_verification)` 重建验证时间线。
fn load_verification_state(
    store: &SessionStore,
    session_id: &str,
) -> Result<(Vec<GoalVerificationTimeline>, Vec<GoalVerification>), session_store::StoreError> {
    let entries = store.entries(session_id)?;
    let mut timeline: Vec<GoalVerificationTimeline> = Vec::new();
    for entry in entries
        .into_iter()
        .filter(|entry| entry.entry_type == "target_completion_verification")
    {
        let payload = entry.data.get("payload").cloned().unwrap_or(Value::Null);
        let Some(verification_id) = payload
            .get("verificationId")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        let target_id = payload
            .get("targetId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let status = match payload.get("status").and_then(Value::as_str) {
            Some("started") => GoalVerificationStatus::Started,
            Some("completed") => GoalVerificationStatus::Completed,
            Some("cancelled") => GoalVerificationStatus::Cancelled,
            _ => GoalVerificationStatus::FailedClosed,
        };
        let verification = payload.get("verification").map(|value| GoalVerification {
            passed: value
                .get("passed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            reason: value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            next_action: Some(
                value
                    .get("nextAction")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ),
        });
        let goal_iteration = payload.get("goalIteration").and_then(Value::as_u64);
        let anchor_assistant_message_id = payload
            .get("anchorAssistantMessageId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let anchor_turn_id = payload
            .get("anchorTurnId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(existing) = timeline
            .iter_mut()
            .find(|item| item.verification_id == verification_id)
        {
            existing.status = status;
            existing.verification = verification;
            existing.updated_at = entry.time_updated.max(0) as u64;
            if existing.started_at.is_none() && status == GoalVerificationStatus::Started {
                existing.started_at = Some(entry.time_created.max(0) as u64);
            }
            continue;
        }
        timeline.push(GoalVerificationTimeline {
            version: 1,
            kind: SyntheticKind::Synthetic,
            timeline_type: GoalVerificationType::GoalVerification,
            display: SeparatorDisplay::Separator,
            target_id,
            verification_id,
            status,
            verification,
            goal_iteration,
            anchor_assistant_message_id,
            anchor_turn_id,
            started_at: (status == GoalVerificationStatus::Started)
                .then(|| entry.time_created.max(0) as u64),
            updated_at: entry.time_updated.max(0) as u64,
        });
    }
    timeline.sort_by_key(|item| item.started_at.unwrap_or(item.updated_at));
    let verifications = recent_verifications(&timeline);
    Ok((timeline, verifications))
}

/// 配置失败的 goal 任务：不启动模型请求、不发 prompt turn；目标暂停并收口。
///
/// 线上未观察该组合（`agent-c3-contract.md` §1.10），登记在 §1.11；由
/// `configuration_failure_drains_pending_pause_event` 覆盖 pending drain。
async fn run_goal_configuration_failure(
    shared: &Arc<Shared>,
    store: &Arc<SessionStore>,
    item: &QueuedInput,
    message: &str,
) {
    let session_id = shared.record.id.clone();
    let mut live = shared.lock();
    live.current_cancel = None;
    live.current_kind = None;
    live.goal.job_active = false;
    live.goal_cancel = None;
    let pending: Vec<SessionEvent> = live.goal.pending_cancel_events.drain(..).collect();
    if let Some(mut target) = live.goal.target.clone() {
        if target.status == GoalStatus::Active {
            target.set_status(GoalStatus::Paused, now_ms());
        }
        target.active_input_id = None;
        target.active_run_started_at = None;
        target.active_run_last_seen_at = None;
        target.time_updated = now_ms();
        let _ = store.upsert_target(&target_record(&target));
        live.goal.target = Some(target);
    }
    if let Some(job) = &item.goal {
        live.inbox.complete(&session_id, &job.input_id);
    }
    live.status = RuntimeStatus::Error;
    live.last_error = Some(message.to_string());
    live.updated_at = now_ms();
    drop(live);
    // pause/stop 在 provider 解析窗口预分配的事件也必须发布，不能留下 seq 缺号。
    if !pending.is_empty() {
        shared.publish_many(pending);
    }
}

/// goal 任务的统一收口：清 worker 相位、结算 CommandInbox 并恢复状态。
///
/// revision 不在这里推进（夹具按命令/控制 turn/取消/完成为口径；turn 收口本身
/// 不额外计数）。
fn finish_goal_job(
    shared: &Arc<Shared>,
    session_id: &str,
    input_id: &str,
    admitted: bool,
    failed: bool,
) {
    let mut live = shared.lock();
    live.current_cancel = None;
    live.current_kind = None;
    live.goal.job_active = false;
    live.goal_cancel = None;
    let pending: Vec<SessionEvent> = live.goal.pending_cancel_events.drain(..).collect();
    if admitted {
        live.inbox.complete(session_id, input_id);
    }
    live.updated_at = now_ms();
    live.status = if failed {
        RuntimeStatus::Error
    } else if live.inbox.running(session_id).is_some() {
        RuntimeStatus::Running
    } else {
        RuntimeStatus::Idle
    };
    drop(live);
    if !pending.is_empty() {
        shared.publish_many(pending);
    }
}

/// 一条 goal 相关的消息行（`user_prompt` 或 `system_reminder`）。
fn goal_message_rows(
    session_id: &str,
    text: &str,
    kind: &str,
    source: Option<&str>,
    sequence: i64,
    now: i64,
) -> (MessageRecord, PartRecord) {
    let message_id = fresh_id("msg");
    let mut semantics = json!({
        "origin": if kind == "user_prompt" { "real_user" } else { "agent_runtime" },
        "kind": kind,
        "uiVisibility": if kind == "user_prompt" { "visible" } else { "hidden" },
        "providerVisibility": "visible",
        "transcriptVisibility": if kind == "user_prompt" { "visible" } else { "hidden" },
    });
    if let Some(source) = source {
        semantics["source"] = json!(source);
    }
    let message = MessageRecord {
        id: message_id.clone(),
        session_id: session_id.to_string(),
        time_created: now,
        time_updated: now,
        data: json!({
            "role": "user",
            "messageId": message_id,
            "sequence": sequence,
            "time": { "created": now },
            "semantics": semantics,
        }),
        sequence: Some(sequence),
    };
    let part = PartRecord {
        id: fresh_id("part"),
        message_id,
        session_id: session_id.to_string(),
        time_created: now,
        time_updated: now,
        data: json!({
            "type": "text",
            "text": text,
            "time": { "start": now, "end": now },
        }),
        sequence: Some(0),
    };
    (message, part)
}

/// 秒级墙钟（向上取整；任何发生过的时间段至少记 1 秒——夹具的
/// `time_used_seconds` 折叠为 `<time>`，只断言非负/保留）。
fn elapsed_seconds(started_at: i64, now: i64) -> u64 {
    let millis = (now - started_at).max(0) as u64;
    millis.div_ceil(1000).max(1)
}

/// 发布方法层预分配的 pause/stop `status_updated` 事件队列；返回最后一个 seq。
fn publish_pending_cancel_event(shared: &Arc<Shared>) -> Option<u64> {
    let events: Vec<SessionEvent> = {
        let mut live = shared.lock();
        live.goal.pending_cancel_events.drain(..).collect()
    };
    let last = events.last().map(|event| event.seq);
    shared.publish_many(events);
    last
}

/// 写 goal 续跑/verifier 的 `model_usage` 行（夹具的 `query_source` 区分：
/// `main_turn` / `target_completion_verification`）。
#[allow(clippy::too_many_arguments)]
fn record_goal_model_usage(
    store: &SessionStore,
    session_id: &str,
    task_type: &str,
    config: &RuntimeConfig,
    query_source: &str,
    usage: &ModelUsage,
    started_at: i64,
    completed_at: i64,
    status: &str,
    error_type: Option<&str>,
    turn_id: Option<&str>,
) {
    let total = usage.input_tokens + usage.output_tokens;
    let record = ModelUsageRecord {
        id: fresh_id("usage_model_goal"),
        session_id: session_id.to_string(),
        logical_request_id: fresh_id("req"),
        provider_id: config.provider_id.clone(),
        model_id: config.model_id.clone(),
        query_source: query_source.to_string(),
        started_at: started_at.max(1),
        status: status.to_string(),
        agent: Some("mode-agent".to_string()),
        attempt_index: 0,
        cache_read_input_tokens: usage.cache_read_tokens as i64,
        cancelled_by_user: status == "cancelled",
        completed_at: Some(completed_at),
        computed_total_tokens: total as i64,
        duration_ms: Some((completed_at - started_at.max(1)).max(0)),
        finish_reason: (status == "completed").then(|| "stop".to_string()),
        input_tokens: usage.input_tokens as i64,
        mode: Some(config.mode.clone()),
        output_tokens: usage.output_tokens as i64,
        provider_total_tokens: Some(total as i64),
        reasoning_tokens: usage.reasoning_tokens as i64,
        span_id: Some(uuid::Uuid::new_v4().to_string()),
        task_type: Some(task_type.to_string()),
        trace_id: Some(uuid::Uuid::new_v4().to_string()),
        turn_id: turn_id.map(str::to_owned),
        variant: config.thought_level.clone(),
        error_type: error_type.map(str::to_owned),
        ..ModelUsageRecord::default()
    };
    let _ = store.record_model_usage(record);
}

/// 一个 goal 任务（set/replace/resume）的 worker 执行体。
///
/// 方法层已经同步产出 title/控制 turn/summary/set/status 事件并落库；worker 只
/// 对齐事件序号后进入续跑循环。
async fn run_goal_job(
    mut session: Session,
    shared: &Arc<Shared>,
    store: &Arc<SessionStore>,
    provider: Arc<dyn ModelProvider>,
    item: QueuedInput,
) -> Session {
    let job = item.goal.clone().expect("run_goal_job 只处理 goal 队列项");
    let session_id = shared.record.id.clone();
    let config = shared.read_config();
    run_goal_loop(&mut session, shared, store, provider, &job, &config).await;
    finish_goal_job(shared, &session_id, &job.input_id, true, false);
    session
}

/// goal 续跑/通知 turn 的 provider 失败收口（C3 T3）。
///
/// 终态恰好一次：`turn.failed` 由这里补发；目标 active 字段清空并同步落库，
/// 但目标仍保持 `active`（T1 §1.11 的失败路径）。调用方随后结束续跑循环。
fn fail_goal_turn(
    session: &mut Session,
    shared: &Arc<Shared>,
    store: &Arc<SessionStore>,
    recorder: &mut TurnRecorder,
    error: &agent_core::TurnError,
) {
    recorder.record_failure_progress(&error.progress, Some(("unknown_error", "UNKNOWN_ERROR")));
    let failed = match &error.provider_error {
        Some(provider_error) => {
            if provider_error.is_auth() {
                recorder.persist_failed_placeholder();
            }
            session.emit_turn_failed_for_provider(
                recorder.turn_id().map(String::as_str),
                provider_error,
                "processing_input",
            )
        }
        None => session.emit_turn_failed(
            recorder.turn_id().map(String::as_str),
            "runtime_error",
            &error.message,
            "model",
        ),
    };
    recorder.emit(failed);
    {
        let mut live = shared.lock();
        live.last_error = Some(error.message.clone());
        live.status = RuntimeStatus::Error;
        shared.tool_execution.store(false, Ordering::SeqCst);
        if let Some(target) = live.goal.target.clone() {
            let mut cleared = target;
            cleared.active_input_id = None;
            cleared.active_run_started_at = None;
            cleared.active_run_last_seen_at = None;
            cleared.time_updated = now_ms();
            let _ = store.upsert_target(&target_record(&cleared));
            live.goal.target = Some(cleared);
        }
        live.updated_at = now_ms();
    }
}

/// 续跑循环：每轮一个 goal-continuation turn + completion verifier；无轮次上限。
async fn run_goal_loop(
    session: &mut Session,
    shared: &Arc<Shared>,
    store: &Arc<SessionStore>,
    provider: Arc<dyn ModelProvider>,
    job: &GoalJob,
    config: &RuntimeConfig,
) {
    let session_id = shared.record.id.clone();
    loop {
        let Some((target, turn_number, continuation_ordinal)) = ({
            let mut live = shared.lock();
            if continuation_stop_reason(live.goal.target.as_ref(), false).is_some() {
                None
            } else {
                live.goal.continuation_ordinal += 1;
                let ordinal = live.goal.continuation_ordinal;
                let turn_number = live.goal.control_turns;
                live.goal
                    .target
                    .clone()
                    .map(|target| (target, turn_number, ordinal))
            }
        }) else {
            // pause/stop 在任务队列等待期/轮次间到达：状态事件由这里补发。
            publish_pending_cancel_event(shared);
            break;
        };
        let cancel = shared.lock().goal_cancel.clone().unwrap_or_default();
        if cancel.is_cancelled() {
            publish_pending_cancel_event(shared);
            break;
        }
        let prompt = continuation_prompt(
            &target.objective,
            target.tokens_used,
            target.token_budget,
            target.time_used_seconds,
        );
        let turn_input_id = fresh_id("goal_input");
        let target_id = target.target_id.clone();
        let active_input_id = job.input_id.clone();
        let foreground_execution_id = format!("runtime_command_{continuation_ordinal}");
        let run_started_at = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(now_ms()));

        let shared_after = Arc::clone(shared);
        let store_after = Arc::clone(store);
        let run_started_for_after = Arc::clone(&run_started_at);
        // 同一轮里的 goal-continuation 与 notification turn 共用这对钩子；
        // activeInputId 在通知 turn 前切换成通知自己的 inputId（夹具 §3.3）。
        let active_input_for_after = Arc::new(Mutex::new(active_input_id.clone()));
        let active_input_handle = Arc::clone(&active_input_for_after);
        let mut after_start = move |session: &mut Session, sink: &mut dyn EventSink| {
            let now = now_ms();
            run_started_for_after.store(now, std::sync::atomic::Ordering::SeqCst);
            let payload = {
                let mut live = shared_after.lock();
                let payload = {
                    let Some(target) = live.goal.target.as_mut() else {
                        return;
                    };
                    let active_input = active_input_for_after
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone();
                    let previous = target.clone();
                    target.start_run(&active_input, now);
                    let _ = store_after.upsert_target(&target_record(target));
                    let next = target.clone();
                    target_update_payload("run_started", "runtime", Some(&previous), Some(&next))
                };
                // goal continuation 在 turn 开始时就计入 projection.turnCount（观察：
                // pause 应答快照在终态前已是 control+continuation）。
                live.turn_count += 1;
                live.updated_at = now;
                payload
            };
            let turn_id = session.current_turn_id().map(str::to_owned);
            let event = session.emit(
                turn_id.as_deref(),
                SessionEventType::SessionUpdated,
                &payload,
            );
            sink.emit(event);
        };

        let shared_before = Arc::clone(shared);
        let store_before = Arc::clone(store);
        let session_id_before = session_id.clone();
        let config_before = config.clone();
        let task_type_before = shared.record.task_type.clone();
        let run_started_for_before = Arc::clone(&run_started_at);
        let mut before_terminal =
            move |session: &mut Session, sink: &mut dyn EventSink, facts: &TurnTerminalFacts| {
                let now = now_ms();
                let payload = {
                    let mut live = shared_before.lock();
                    let Some(current) = live.goal.target.clone() else {
                        return;
                    };
                    let mut next = current.clone();
                    let tokens = if facts.cancelled {
                        0
                    } else {
                        facts.usage.input_tokens + facts.usage.output_tokens
                    };
                    let started_at = current.active_run_started_at.unwrap_or_else(now_ms);
                    let seconds = elapsed_seconds(started_at, now);
                    next.finish_run(now, tokens, seconds);
                    let _ = store_before.upsert_target(&target_record(&next));
                    live.goal.target = Some(next.clone());
                    if facts.cancelled {
                        // 夹具：取消的续跑终态会让 stateRevision +1（pause/stop 之后）。
                        live.revision += 1;
                    }
                    live.updated_at = now;
                    target_update_payload("run_finished", "runtime", Some(&current), Some(&next))
                };
                // 取消的 status_updated 用方法层预分配的事件（seq 已进应答快照），
                // 必须在 run_finished 与 turn.completed 之前发布。
                publish_pending_cancel_event(&shared_before);
                let turn_id = session.current_turn_id().map(str::to_owned);
                let event = session.emit(
                    turn_id.as_deref(),
                    SessionEventType::SessionUpdated,
                    &payload,
                );
                sink.emit(event);
                // 只有真的发出过模型请求才写 model_usage（pre-request pause 观察 0 行）。
                if facts.model_requests > 0 {
                    record_goal_model_usage(
                        &store_before,
                        &session_id_before,
                        &task_type_before,
                        &config_before,
                        "main_turn",
                        &facts.usage,
                        run_started_for_before.load(std::sync::atomic::Ordering::SeqCst),
                        now,
                        if facts.cancelled {
                            "cancelled"
                        } else {
                            "completed"
                        },
                        facts.cancelled.then_some("cancelled"),
                        turn_id.as_deref(),
                    );
                }
            };

        let mut recorder =
            TurnRecorder::goal_turn(Arc::clone(shared), Arc::clone(store), prompt.clone());
        let result = session
            .run_goal_continuation_turn(
                &prompt,
                &turn_input_id,
                &target_id,
                provider.as_ref(),
                &mut recorder,
                &cancel,
                &mut after_start,
                &mut before_terminal,
                turn_number,
                &foreground_execution_id,
            )
            .await;
        match result {
            Ok(()) => {
                // turn 与 verifier 之间可能到达 pause/stop：先补状态事件再停止。
                let stop_between = {
                    let live = shared.lock();
                    !live.goal.pending_cancel_events.is_empty()
                        || live
                            .goal
                            .target
                            .as_ref()
                            .map(|target| target.status != GoalStatus::Active)
                            .unwrap_or(true)
                };
                if stop_between {
                    publish_pending_cancel_event(shared);
                    break;
                }
                // C3 T3 / 夹具 §3.3：续跑 turn 结束后、verifier 之前，把 pending
                // 后台通知合并成一个 `background_task` turn；它包在同一个
                // run_started/run_finished 钩子里，tokens/time 计入目标。
                if let Some((input, notifications)) = take_notification_batch(session) {
                    let notification_input_id = fresh_id("in");
                    *active_input_handle
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        notification_input_id.clone();
                    let mut notification_recorder = TurnRecorder::notification_turn(
                        Arc::clone(shared),
                        Arc::clone(store),
                        input.clone(),
                    );
                    let notification_result = session
                        .run_background_notification_turn_with_hooks(
                            &input,
                            &notification_input_id,
                            &notifications,
                            provider.as_ref(),
                            &mut notification_recorder,
                            &cancel,
                            &mut after_start,
                            &mut before_terminal,
                        )
                        .await;
                    match notification_result {
                        Ok(()) => {}
                        Err(error) if error.is_cancelled() => {
                            break;
                        }
                        Err(error) => {
                            fail_goal_turn(
                                session,
                                shared,
                                store,
                                &mut notification_recorder,
                                &error,
                            );
                            break;
                        }
                    }
                }
                let verdict =
                    run_goal_verifier(session, shared, store, provider.clone(), config).await;
                match verdict {
                    Some(verdict) if verdict.passed => {
                        complete_goal_target(session, shared, store);
                        break;
                    }
                    Some(_) => {
                        // verifier 未通过：继续下一轮（无轮次上限）。
                    }
                    None => break,
                }
            }
            Err(error) if error.is_cancelled() => {
                break;
            }
            Err(error) => {
                fail_goal_turn(session, shared, store, &mut recorder, &error);
                break;
            }
        }
    }
}

/// verifier 的 event payload（`session.updated`，无 action）。
#[allow(clippy::too_many_arguments)]
fn verification_event_payload(
    target_id: &str,
    verification_id: &str,
    continuation_ordinal: u64,
    goal_iteration: u64,
    anchor_assistant_message_id: &str,
    anchor_turn_id: &str,
    status: &str,
    verification: Option<&GoalVerification>,
) -> Value {
    let mut payload = json!({
        "anchorAssistantMessageId": anchor_assistant_message_id,
        "anchorTurnId": anchor_turn_id,
        "foregroundExecutionId": format!("runtime_command_{continuation_ordinal}"),
        "goalIteration": goal_iteration,
        "status": status,
        "targetId": target_id,
        "verificationId": verification_id,
    });
    if let Some(verification) = verification {
        let mut value = json!({
            "passed": verification.passed,
            "reason": verification.reason,
        });
        if let Some(Some(next_action)) = &verification.next_action {
            value["nextAction"] = json!(next_action);
        }
        payload["verification"] = value;
    }
    payload
}

/// 写一条 `session_entry(target_completion_verification)`。
fn persist_verification_entry(
    store: &SessionStore,
    session_id: &str,
    payload: &Value,
    sequence_number: u64,
    event_id: &str,
) {
    let now = now_ms();
    let _ = store.append_entry(SessionEntryRecord {
        id: fresh_id("entry"),
        session_id: session_id.to_string(),
        entry_type: "target_completion_verification".to_string(),
        time_created: now,
        time_updated: now,
        data: json!({
            "eventId": event_id,
            "payload": payload,
            "sequenceNumber": sequence_number,
            "traceId": uuid::Uuid::new_v4().to_string(),
        }),
    });
}

/// 把 timeline 中同一 `verificationId` 的条目更新到终态；缺失时补一条。
fn settle_verification_timeline(
    live: &mut Live,
    verification_id: &str,
    status: GoalVerificationStatus,
    verification: GoalVerification,
    updated_at: u64,
) {
    if let Some(item) = live
        .goal
        .timeline
        .iter_mut()
        .find(|item| item.verification_id == verification_id)
    {
        item.status = status;
        item.verification = Some(verification);
        item.updated_at = updated_at;
    } else {
        live.goal.timeline.push(GoalVerificationTimeline {
            version: 1,
            kind: SyntheticKind::Synthetic,
            timeline_type: GoalVerificationType::GoalVerification,
            display: SeparatorDisplay::Separator,
            target_id: String::new(),
            verification_id: verification_id.to_string(),
            status,
            verification: Some(verification),
            goal_iteration: None,
            anchor_assistant_message_id: None,
            anchor_turn_id: None,
            started_at: None,
            updated_at,
        });
    }
    live.goal.verifications = recent_verifications(&live.goal.timeline);
}

/// 运行 completion verifier（非流式、无 tools；观察 §1.5）。
///
/// 返回 `None` = 停止续跑（pause/stop 取消、provider 失败）；`Some(verdict)` =
/// 拿到合法/fallback 判定。
async fn run_goal_verifier(
    session: &mut Session,
    shared: &Arc<Shared>,
    store: &Arc<SessionStore>,
    provider: Arc<dyn ModelProvider>,
    config: &RuntimeConfig,
) -> Option<agent_core::goal::VerifierVerdict> {
    let (target, anchor_turn_id, anchor_assistant_message_id) = {
        let live = shared.lock();
        let target = live.goal.target.clone()?;
        if target.status != GoalStatus::Active {
            return None;
        }
        (
            target,
            session.current_turn_id().unwrap_or_default().to_string(),
            session
                .current_assistant_message_id()
                .unwrap_or_default()
                .to_string(),
        )
    };
    let cancel = shared.lock().goal_cancel.clone().unwrap_or_default();
    if cancel.is_cancelled() {
        publish_pending_cancel_event(shared);
        return None;
    }
    // 只有真正进入 verifier 的尝试才增加 `goalIteration`（pre-request 取消不算）。
    let (iteration, continuation_ordinal) = {
        let mut live = shared.lock();
        live.goal.iteration += 1;
        (live.goal.iteration, live.goal.continuation_ordinal)
    };
    let verification_id = fresh_id("verify");
    let started_at = now_ms();
    let started_payload = verification_event_payload(
        &target.target_id,
        &verification_id,
        continuation_ordinal,
        iteration,
        &anchor_assistant_message_id,
        &anchor_turn_id,
        "started",
        None,
    );
    let event = session.emit(None, SessionEventType::SessionUpdated, &started_payload);
    let sequence = event.seq;
    let event_id = event.event_id.clone();
    // 先落 session_entry，再发布（事件可见 ⇒ 行已提交）。
    persist_verification_entry(
        store,
        &shared.record.id,
        &started_payload,
        sequence,
        &event_id,
    );
    shared.publish(&event);
    {
        let mut live = shared.lock();
        live.goal.timeline.push(GoalVerificationTimeline {
            version: 1,
            kind: SyntheticKind::Synthetic,
            timeline_type: GoalVerificationType::GoalVerification,
            display: SeparatorDisplay::Separator,
            target_id: target.target_id.clone(),
            verification_id: verification_id.clone(),
            status: GoalVerificationStatus::Started,
            verification: None,
            goal_iteration: Some(iteration),
            anchor_assistant_message_id: Some(anchor_assistant_message_id.clone()),
            anchor_turn_id: Some(anchor_turn_id.clone()),
            started_at: Some(started_at.max(0) as u64),
            updated_at: started_at.max(0) as u64,
        });
    }

    let prompt = verifier_prompt(
        &target.objective,
        target.status,
        target.tokens_used,
        target.token_budget,
        target.time_used_seconds,
    );
    let request = session.build_verification_request(&prompt);
    let (delta_sink, mut deltas) = tokio::sync::mpsc::unbounded_channel();
    let mut future = provider.stream(request, delta_sink);
    let mut text = String::new();
    let mut delta_channel_open = true;
    let mut response: Option<Result<agent_core::ModelResponse, ProviderError>> = None;
    let mut cancelled = false;
    while response.is_none() {
        tokio::select! {
            biased;
            maybe_delta = deltas.recv(), if delta_channel_open => {
                match maybe_delta {
                    Some(ModelDelta::Text { text: chunk }) => text.push_str(&chunk),
                    Some(_) => {}
                    None => delta_channel_open = false,
                }
            }
            // 取消优先于 verifier 应答（pause 时 timeline 记 cancelled）。
            _ = cancel.cancelled() => {
                cancelled = true;
                break;
            }
            result = &mut future => response = Some(result),
        }
    }
    // future 结束前已进入队列、但 select 没取走的 delta（tail）：与 run_turn 同口径。
    while let Ok(delta) = deltas.try_recv() {
        if let ModelDelta::Text { text: chunk } = delta {
            text.push_str(&chunk);
        }
    }
    let finished_at = now_ms();
    if cancelled {
        publish_pending_cancel_event(shared);
        let verification = GoalVerification {
            passed: false,
            reason: CANCELLED_VERIFIER_REASON.to_string(),
            next_action: Some(None),
        };
        let payload = verification_event_payload(
            &target.target_id,
            &verification_id,
            continuation_ordinal,
            iteration,
            &anchor_assistant_message_id,
            &anchor_turn_id,
            "cancelled",
            Some(&verification),
        );
        let event = session.emit(None, SessionEventType::SessionUpdated, &payload);
        let sequence = event.seq;
        let event_id = event.event_id.clone();
        persist_verification_entry(store, &shared.record.id, &payload, sequence, &event_id);
        shared.publish(&event);
        {
            let mut live = shared.lock();
            settle_verification_timeline(
                &mut live,
                &verification_id,
                GoalVerificationStatus::Cancelled,
                verification,
                finished_at.max(0) as u64,
            );
        }
        record_goal_model_usage(
            store,
            &shared.record.id,
            &shared.record.task_type,
            config,
            "target_completion_verification",
            &ModelUsage::default(),
            started_at,
            finished_at,
            "cancelled",
            Some("cancelled"),
            None,
        );
        return None;
    }
    let response = response.expect("循环只在 provider future 结束后退出");
    let usage = match &response {
        Ok(response) => response.usage.clone(),
        Err(error) => {
            publish_pending_cancel_event(shared);
            let verification = GoalVerification {
                passed: false,
                reason: format!("Completion verifier failed: {}", error.message),
                next_action: Some(None),
            };
            let payload = verification_event_payload(
                &target.target_id,
                &verification_id,
                continuation_ordinal,
                iteration,
                &anchor_assistant_message_id,
                &anchor_turn_id,
                "failed_closed",
                Some(&verification),
            );
            let event = session.emit(None, SessionEventType::SessionUpdated, &payload);
            let sequence = event.seq;
            let event_id = event.event_id.clone();
            persist_verification_entry(store, &shared.record.id, &payload, sequence, &event_id);
            shared.publish(&event);
            {
                let mut live = shared.lock();
                settle_verification_timeline(
                    &mut live,
                    &verification_id,
                    GoalVerificationStatus::FailedClosed,
                    verification,
                    finished_at.max(0) as u64,
                );
            }
            return None;
        }
    };
    record_goal_model_usage(
        store,
        &shared.record.id,
        &shared.record.task_type,
        config,
        "target_completion_verification",
        &usage,
        started_at,
        finished_at,
        "completed",
        None,
        None,
    );
    let verdict = parse_verifier_response(&text);
    let verification = GoalVerification {
        passed: verdict.passed,
        reason: verdict.reason.clone(),
        next_action: Some(verdict.next_action.clone()),
    };
    let payload = verification_event_payload(
        &target.target_id,
        &verification_id,
        continuation_ordinal,
        iteration,
        &anchor_assistant_message_id,
        &anchor_turn_id,
        "completed",
        Some(&verification),
    );
    let event = session.emit(None, SessionEventType::SessionUpdated, &payload);
    let sequence = event.seq;
    let event_id = event.event_id.clone();
    persist_verification_entry(store, &shared.record.id, &payload, sequence, &event_id);
    shared.publish(&event);
    {
        let mut live = shared.lock();
        settle_verification_timeline(
            &mut live,
            &verification_id,
            GoalVerificationStatus::Completed,
            verification,
            finished_at.max(0) as u64,
        );
    }
    Some(verdict)
}

/// verifier 通过：target `complete` + `status_updated(runtime)`。
fn complete_goal_target(session: &mut Session, shared: &Arc<Shared>, store: &Arc<SessionStore>) {
    let event_payload = {
        let mut live = shared.lock();
        let Some(previous) = live.goal.target.clone() else {
            return;
        };
        let mut target = previous.clone();
        target.set_status(GoalStatus::Complete, now_ms());
        target.active_input_id = None;
        target.active_run_started_at = None;
        target.active_run_last_seen_at = None;
        target.time_updated = now_ms();
        let _ = store.upsert_target(&target_record(&target));
        live.goal.target = Some(target.clone());
        live.revision += 1;
        live.updated_at = now_ms();
        target_update_payload("status_updated", "runtime", Some(&previous), Some(&target))
    };
    let event = session.emit(None, SessionEventType::SessionUpdated, &event_payload);
    shared.publish(&event);
}

/// compact 终态的落库事实（行构造的唯一输入）。
///
/// id 在进入终态前一次性预生成：timeline 行先于摘要行落库，但它要引用
/// `summaryMessageId`（观察顺序 message sequence 9 → 10，契约 §1.3）。
struct CompactFacts {
    session_id: String,
    directory: String,
    task_type: String,
    turn_id: String,
    input_id: String,
    operation_id: String,
    boundary_id: String,
    started_at: i64,
    ended_at: i64,
    /// `preCompactTokenCount`：本地估算（4 字符 ≈ 1 token，同 `trim_messages` 口径；
    /// 不是 provider usage——公式登记在契约「Rust C2 现状」）。
    pre_tokens: u64,
    /// `postCompactTokenCount`：compact 请求的 provider usage 合计（仅成功路径写入；
    /// 失败/skipped 不产出该字段）。
    post_tokens: u64,
    /// `summarizedMessageCount`：被摘要替换的模型历史条数。
    summarized_count: u64,
    custom_instructions: bool,
    provider_id: String,
    model_id: String,
    mode: String,
    /// `model_usage.variant`（按 `thoughtLevel` 落——夹具观察值与会话档位一致，
    /// 映射登记在契约注记）。
    thought_level: Option<String>,
    last_summarized_message_id: Option<String>,
    /// 成功时的摘要消息 id（预生成，供 timeline 行引用）。
    summary_message_id: Option<String>,
}

/// compact 已收到的模型应答事实（`model_usage` 行的输入；传输失败时为 `None`）。
struct CompactModelResponse {
    usage: ModelUsage,
    stop_reason: String,
    tool_call_count: u64,
    started_at: i64,
    finished_at: i64,
}

/// compact 终态要写的消息/part 行集合。
enum CompactRows<'a> {
    /// 成功：timeline 消息（`finish:"completed"`，2 parts）+ 摘要消息（2 parts）。
    Success { summary: &'a str },
    /// 失败：timeline 消息（`finish:"failed"`，2 parts），不写摘要（契约 §1.8）。
    Failed,
    /// 跳过：timeline 消息（`finish:"skipped"`，2 parts），不写摘要（契约 §1.6）。
    Skipped,
}

/// 写 compact 的消息 + part 行（每条消息一个原子事务；顺序 = sequence 顺序）。
fn persist_compact_rows(
    shared: &Shared,
    store: &SessionStore,
    facts: &CompactFacts,
    rows: CompactRows<'_>,
) -> Result<(), session_store::StoreError> {
    let (finish, status) = match rows {
        CompactRows::Success { .. } => ("completed", "completed"),
        CompactRows::Failed => ("failed", "failed"),
        CompactRows::Skipped => ("skipped", "skipped"),
    };
    let timeline_message_id = fresh_id("msg");
    let timeline_sequence = take_sequence(shared);
    let timeline_message = MessageRecord {
        id: timeline_message_id.clone(),
        session_id: facts.session_id.clone(),
        time_created: facts.started_at.max(1),
        time_updated: facts.ended_at,
        data: timeline_message_data(facts, finish),
        sequence: Some(timeline_sequence),
    };
    let timeline_parts = vec![
        PartRecord {
            id: fresh_id("part"),
            message_id: timeline_message_id.clone(),
            session_id: facts.session_id.clone(),
            time_created: facts.started_at.max(1),
            time_updated: facts.ended_at,
            data: timeline_part_data(facts, status),
            sequence: Some(0),
        },
        PartRecord {
            id: fresh_id("part"),
            message_id: timeline_message_id.clone(),
            session_id: facts.session_id.clone(),
            time_created: facts.started_at.max(1),
            time_updated: facts.ended_at,
            data: timeline_compaction_part_data(facts, status),
            sequence: Some(1),
        },
    ];
    store.append_message_with_parts(timeline_message, &timeline_parts)?;

    if let CompactRows::Success { summary } = rows {
        let summary_message_id = facts
            .summary_message_id
            .clone()
            .expect("成功路径已预生成摘要消息 id");
        let summary_sequence = take_sequence(shared);
        let summary_message = MessageRecord {
            id: summary_message_id.clone(),
            session_id: facts.session_id.clone(),
            time_created: facts.ended_at,
            time_updated: facts.ended_at,
            data: summary_message_data(facts, summary),
            sequence: Some(summary_sequence),
        };
        let summary_parts = vec![
            PartRecord {
                id: fresh_id("part"),
                message_id: summary_message_id.clone(),
                session_id: facts.session_id.clone(),
                time_created: facts.started_at.max(1),
                time_updated: facts.ended_at,
                data: json!({
                    "type": "text",
                    "text": summary_continuation(summary),
                    "synthetic": true,
                    "time": { "start": facts.started_at, "end": facts.ended_at },
                }),
                sequence: Some(0),
            },
            PartRecord {
                id: fresh_id("part"),
                message_id: summary_message_id.clone(),
                session_id: facts.session_id.clone(),
                time_created: facts.started_at.max(1),
                time_updated: facts.ended_at,
                data: summary_compaction_part_data(facts),
                sequence: Some(1),
            },
        ];
        store.append_message_with_parts(summary_message, &summary_parts)?;
    }
    Ok(())
}

/// 从锁内状态取下一条 message `sequence`（写顺序即分配顺序；与
/// [`TurnRecorder::take_sequence`] 同一事实源）。
fn take_sequence(shared: &Shared) -> i64 {
    let mut live = shared.lock();
    let sequence = live.next_message_sequence;
    live.next_message_sequence += 1;
    sequence
}

/// timeline 消息 `data`（观察形状，契约 §1.3：`finish` 三态 + timeline_event 语义，
/// 全零 tokens、无 `messageId`/`anchor` 键）。
fn timeline_message_data(facts: &CompactFacts, finish: &str) -> Value {
    json!({
        "role": "assistant",
        "time": { "created": facts.started_at, "completed": facts.ended_at },
        "parentID": facts.last_summarized_message_id.clone().unwrap_or_default(),
        "modelId": facts.model_id,
        "providerId": facts.provider_id,
        "mode": facts.mode,
        "planEnabled": false,
        "agent": "mode-agent",
        "path": { "cwd": facts.directory, "root": facts.directory },
        "cost": 0,
        "tokens": {
            "input": 0, "output": 0, "reasoning": 0,
            "cache": { "read": 0, "write": 0 }
        },
        "finish": finish,
        "semantics": {
            "origin": "system",
            "kind": "timeline_event",
            "uiVisibility": "visible",
            "providerVisibility": "hidden",
            "transcriptVisibility": "visible"
        }
    })
}

/// timeline 消息的 `timeline` part（三态字段差异按夹具逐键对齐）。
fn timeline_part_data(facts: &CompactFacts, status: &str) -> Value {
    let mut data = json!({
        "type": "timeline",
        "timelineType": "context_compaction",
        "display": "separator",
        "status": status,
        "operationId": facts.operation_id,
        "sourceCommandId": facts.input_id,
        "trigger": "manual",
        "phase": "standalone_turn",
        "compactReason": "user_requested",
        "preCompactTokenCount": facts.pre_tokens,
        "time": { "start": facts.started_at, "end": facts.ended_at },
    });
    match status {
        "completed" => {
            let summary_id = facts
                .summary_message_id
                .clone()
                .expect("completed timeline part 需要摘要消息 id");
            data["boundaryId"] = json!(facts.boundary_id);
            data["summaryMessageId"] = json!(summary_id);
            // `truePostCompactTokenCount = pre + post`：oracle 的公式未观察（其观察值
            // 2792+22≠2852），这里取可复算的本地定义，登记在契约注记。
            data["postCompactTokenCount"] = json!(facts.post_tokens);
            data["truePostCompactTokenCount"] = json!(facts.pre_tokens + facts.post_tokens);
        }
        "failed" => {
            data["attempt"] = json!(1);
            data["reason"] = json!("model_error");
        }
        _ => {}
    }
    data
}

/// timeline 消息的 `compaction` part（三态字段差异按夹具逐键对齐）。
fn timeline_compaction_part_data(facts: &CompactFacts, status: &str) -> Value {
    let mut data = json!({
        "type": "compaction",
        "auto": false,
        "trigger": "manual",
        "phase": "standalone_turn",
        "compactReason": "user_requested",
        "operationId": facts.operation_id,
        "timelineStatus": status,
        "timelineDisplay": "separator",
        "replace": true,
        "preCompactTokenCount": facts.pre_tokens,
        "time": { "start": facts.started_at, "end": facts.ended_at },
    });
    match status {
        "completed" => {
            let summary_id = facts
                .summary_message_id
                .clone()
                .expect("completed compaction part 需要摘要消息 id");
            data["boundaryId"] = json!(facts.boundary_id);
            data["summaryMessageId"] = json!(summary_id);
            data["tail_start_id"] = json!(summary_id);
            data["postCompactTokenCount"] = json!(facts.post_tokens);
            data["truePostCompactTokenCount"] = json!(facts.pre_tokens + facts.post_tokens);
        }
        "failed" => {
            data["reason"] = json!("model_error");
            data["attempt"] = json!(1);
        }
        _ => {}
    }
    data
}

/// 摘要消息 `data`（观察形状，契约 §1.3：`summary` 三键 + compact_summary 语义，
/// **没有** `time.completed`）。
///
/// `tools` 在夹具里折叠为 `<tool-flags>`（对象形状是唯一保留的事实）；内容随运行
/// 环境变化，观察不到键集合，这里写空对象占位，不冒充具体工具开关。
fn summary_message_data(facts: &CompactFacts, summary: &str) -> Value {
    json!({
        "role": "user",
        "time": { "created": facts.ended_at },
        "summary": {
            "title": SUMMARY_TITLE,
            "body": summary,
            "diffs": [],
        },
        "agent": "mode-agent",
        "modelSelection": {
            "providerId": facts.provider_id,
            "modelId": facts.model_id,
        },
        "semantics": {
            "origin": "agent_runtime",
            "kind": "compact_summary",
            "uiVisibility": "hidden",
            "providerVisibility": "visible",
            "transcriptVisibility": "hidden"
        },
        "tools": {},
        "model": {
            "providerID": facts.provider_id,
            "modelID": facts.model_id,
        }
    })
}

/// 摘要消息的 `compaction` part（带 `compactBoundary` 观察形状，契约 §1.3）。
fn summary_compaction_part_data(facts: &CompactFacts) -> Value {
    let summary_id = facts
        .summary_message_id
        .clone()
        .expect("摘要 compaction part 需要摘要消息 id");
    let mut boundary = json!({
        "boundaryId": facts.boundary_id,
        "trigger": "manual",
        "phase": "standalone_turn",
        "compactReason": "user_requested",
        "summarySource": "model",
        "preCompactTokenCount": facts.pre_tokens,
        "postCompactTokenCount": facts.post_tokens,
        "truePostCompactTokenCount": facts.pre_tokens + facts.post_tokens,
        "summarizedMessageCount": facts.summarized_count,
        "keptMessageCount": 0,
        "summaryMessageIds": [summary_id],
        "customInstructions": facts.custom_instructions,
        // `traceId` 观察为非空 id（事件面 traceId 恒空是 C1 登记差异）：这里生成
        // uuid 满足行形状，不冒充事件追踪事实。
        "traceId": uuid::Uuid::new_v4().to_string(),
        "turnId": facts.turn_id,
    });
    if let Some(last) = &facts.last_summarized_message_id {
        boundary["lastSummarizedMessageId"] = json!(last);
    }
    json!({
        "type": "compaction",
        "auto": false,
        "trigger": "manual",
        "phase": "standalone_turn",
        "compactReason": "user_requested",
        "tail_start_id": summary_id,
        "compactBoundary": boundary,
        "operationId": facts.operation_id,
    })
}

/// `turn_usage` + `model_usage`（成功；契约 §1.3 的观察值）。
fn record_compact_success_usage(
    store: &SessionStore,
    facts: &CompactFacts,
    model: &CompactModelResponse,
) -> Result<(), session_store::StoreError> {
    let total = model.usage.input_tokens + model.usage.output_tokens;
    store.record_turn_usage(TurnUsageRecord {
        session_id: facts.session_id.clone(),
        turn_id: facts.turn_id.clone(),
        started_at: facts.started_at.max(1),
        status: "completed".to_string(),
        completed_at: Some(facts.ended_at),
        duration_ms: Some((facts.ended_at - facts.started_at.max(1)).max(0)),
        cancelled_by_user: false,
        input_tokens: model.usage.input_tokens as i64,
        output_tokens: model.usage.output_tokens as i64,
        computed_total_tokens: total as i64,
        model_request_count: 1,
        tool_call_count: 0,
        tool_error_count: 0,
        user_message_id: None,
        first_model_start_at: Some(model.started_at),
        // 夹具观察 compact 的 `turn_usage.trace_id` 非空（折叠为 `<id>`）。
        trace_id: Some(uuid::Uuid::new_v4().to_string()),
        ..TurnUsageRecord::default()
    })?;
    store.record_model_usage(model_usage_record(facts, model))?;
    Ok(())
}

/// `turn_usage(error)` +（有应答时）`model_usage`（契约 §1.8：tokens 全 0、
/// `error_type:"unknown_error"`、`error_code:"UNKNOWN_ERROR"`；`model_usage` 仍记
/// provider 的原始 usage 与 `finish_reason`）。
fn record_compact_failure_usage(
    store: &SessionStore,
    facts: &CompactFacts,
    model: Option<&CompactModelResponse>,
    model_started_at: i64,
) -> Result<(), session_store::StoreError> {
    store.record_turn_usage(TurnUsageRecord {
        session_id: facts.session_id.clone(),
        turn_id: facts.turn_id.clone(),
        started_at: facts.started_at.max(1),
        status: "error".to_string(),
        completed_at: Some(facts.ended_at),
        duration_ms: Some((facts.ended_at - facts.started_at.max(1)).max(0)),
        cancelled_by_user: false,
        input_tokens: 0,
        output_tokens: 0,
        computed_total_tokens: 0,
        model_request_count: 1,
        tool_call_count: 0,
        tool_error_count: 0,
        user_message_id: None,
        first_model_start_at: Some(model_started_at),
        error_type: Some("unknown_error".to_string()),
        error_code: Some("UNKNOWN_ERROR".to_string()),
        // 夹具观察失败 compact 的 `turn_usage.trace_id` 同样非空（折叠为 `<id>`）。
        trace_id: Some(uuid::Uuid::new_v4().to_string()),
        ..TurnUsageRecord::default()
    })?;
    if let Some(model) = model {
        store.record_model_usage(model_usage_record(facts, model))?;
    }
    Ok(())
}

/// `model_usage` 行（`query_source:"compact"`；`id` 前缀 `usage_model_compact_`，
/// `assistant_message_id`/`parent_user_message_id` 恒 null，契约 §1.3）。
fn model_usage_record(facts: &CompactFacts, model: &CompactModelResponse) -> ModelUsageRecord {
    let total = model.usage.input_tokens + model.usage.output_tokens;
    // `rawFinishReason` 的观察映射：`stop` → `end_turn`、`tool-calls` → `tool_use`
    // （夹具 `provider_metadata_json.rawFinishReason` 观察值）。
    let raw_finish_reason = match model.stop_reason.as_str() {
        "tool-calls" => "tool_use",
        _ => "end_turn",
    };
    let raw_usage = json!({
        "inputTokens": model.usage.input_tokens,
        "outputTokens": model.usage.output_tokens,
        "totalTokens": total,
        "cacheReadTokens": model.usage.cache_read_tokens,
        "cacheWriteTokens": model.usage.cache_write_tokens,
    });
    let provider_metadata = json!({ "rawFinishReason": raw_finish_reason });
    ModelUsageRecord {
        id: fresh_id("usage_model_compact"),
        session_id: facts.session_id.clone(),
        logical_request_id: fresh_id("req"),
        provider_id: facts.provider_id.clone(),
        model_id: facts.model_id.clone(),
        query_source: "compact".to_string(),
        started_at: model.started_at,
        status: "completed".to_string(),
        agent: Some("mode-agent".to_string()),
        assistant_message_id: None,
        attempt_index: 0,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: model.usage.cache_read_tokens as i64,
        cancelled_by_user: false,
        completed_at: Some(model.finished_at),
        computed_total_tokens: total as i64,
        context_exceeded: false,
        duration_ms: Some((model.finished_at - model.started_at).max(0)),
        error_code: None,
        error_message: None,
        error_type: None,
        finish_reason: Some(model.stop_reason.clone()),
        first_token_at: None,
        input_tokens: model.usage.input_tokens as i64,
        mode: Some(facts.mode.clone()),
        output_tokens: model.usage.output_tokens as i64,
        parent_user_message_id: None,
        provider_metadata_json: Some(provider_metadata.to_string()),
        provider_total_tokens: Some(total as i64),
        raw_usage_json: Some(raw_usage.to_string()),
        reasoning_tokens: model.usage.reasoning_tokens as i64,
        retry_count: 0,
        retryable: false,
        // 夹具观察 `span_id`/`trace_id` 非空（折叠为 `<id>`）：生成 uuid 满足行形状；
        // 事件面的 traceId 恒空是 C1 登记差异，两者互不冒充。
        span_id: Some(uuid::Uuid::new_v4().to_string()),
        task_type: Some(facts.task_type.clone()),
        time_to_first_token_ms: None,
        tool_call_count: model.tool_call_count as i64,
        trace_id: Some(uuid::Uuid::new_v4().to_string()),
        turn_id: Some(facts.turn_id.clone()),
        variant: facts.thought_level.clone(),
    }
}

/// `cacheStats`（C1 的拟合公式原样复用：终末消息数 −1 + 6 + turn 前已有工具结果）。
///
/// 成功路径在替换历史后调用（`messages.len()==1` → 观察值 6）；skipped 路径在
/// 上一个 compact 的摘要历史上调用（同样是 1 → 观察值 6，契约 §1.6 trial 4）。
fn compact_cache_stats(messages_len: usize, prior_tool_history: bool) -> CacheStats {
    CacheStats {
        total_messages: (messages_len.saturating_sub(1)
            + 6
            + if prior_tool_history { 1 } else { 0 }) as u64,
        cached_messages: 5,
        last_cache_hit: false,
        cache_read_tokens: None,
        extra: ExtraFields::new(),
    }
}

/// 一次 turn 的落库记录器：[`EventSink`] 的实现，先观察落库再发布事件（Task 8：
/// turn 跑在阻塞线程上，订阅者并行；「事件可见 ⇒ 行已提交」，见 [`EventSink`]
/// 实现）。
///
/// 观察规则（只在**完成边界**写行，裁决 3）：
/// - `turn.started`：user message + text part 原子写入；
/// - `model.streaming`：累积本步文本 / 工具调用；
/// - `tool.updated`：result/error 回填工具记录；`batch` 时整条 assistant 消息
///   （step-start / tool / step-finish parts）原子写入 + `tool_usage` 行；
/// - `turn.completed`（成功）：最后一条 assistant 消息（text / step-finish）原子
///   写入 + `turn_usage(completed)`；
/// - `turn.completed`（取消）：不写 assistant 行（没有完整的一步），写
///   `turn_usage(cancelled)`；
/// - `turn.failed`：`turn_usage(failed)`。
///
/// 落库失败记进 [`Self::persist_error`]，不改变事件流（实时事件优先），终态后由
/// worker 折进 `last_error`。
/// `turn.started` 落库的 user 消息语义（goal 续跑用 `system_reminder`，观察
/// `source:"goal-continuation"`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UserMessageSemantics {
    UserPrompt,
    GoalContinuation,
    BackgroundNotification,
}

struct TurnRecorder {
    shared: Arc<Shared>,
    store: Arc<SessionStore>,
    session_id: String,
    /// 本轮用户输入原文（`turn.started` 时落库）。
    content: String,
    /// 是否在 `turn.started` 写 user message 行（compact 任务恒 `false`：
    /// compact 没有 user 行 / `session_input` 行，契约 §1.3）。
    persist_user: bool,
    /// user message 行的语义（prompt vs goal-continuation）。
    user_semantics: UserMessageSemantics,
    turn_id: Option<String>,
    turn_started_at: i64,
    user_message_id: Option<String>,
    /// 最近一条已持久化 message id（assistant 的 `parentID` 链）。
    last_message_id: Option<String>,
    /// 正在累积的模型步（按事件的 `assistantMessageId` 归组）。
    current: Option<AssistantStep>,
    tool_error_count: u64,
    /// 终态聚合 tokens（快照/usage 口径；取消/失败时可能是部分值或 0）。
    turn_total_tokens: u64,
    /// 最近一次**已完成**模型请求的 usage；下一次步骤落库时消费（最终评审 I2：
    /// 每个 assistant 行的 tokens 是该步骤请求的用量，不是 turn 聚合值）。
    last_step_usage: Option<agent_core::provider::ModelUsage>,
    /// C3 T2：本 turn 观察到的 adapter 重试次数（`model_retry_scheduled` 事件数）。
    /// stream recovery 不计入该字段（夹具 §2.2：`model_retry_count:0`）。
    model_retry_count: u64,
    /// C3 T2：DB/轮次口径的请求数（内部事实通道；取消终态的事件载荷不带它）。
    model_request_rounds: Option<u64>,
    persist_error: Option<String>,
}

struct AssistantStep {
    /// 事件侧的 `assistantMessageId`（事件归组键；**不**用作 DB 主键——它按
    /// 请求序号重复，跨 turn 会撞，落库一律用 [`fresh_id`] 生成的新 id）。
    event_message_id: String,
    started_at: i64,
    text: String,
    tools: Vec<ToolCallRecord>,
}

struct ToolCallRecord {
    tool_call_id: String,
    tool_name: String,
    input: Value,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    read_only: Option<bool>,
    side_effect_scope: Option<String>,
    /// `result.serialization`（DB tool part 的 `state.metadata.serialization` 来源）。
    serialization: Option<Value>,
    outcome: Option<Result<String, String>>,
}

impl TurnRecorder {
    fn new(shared: Arc<Shared>, store: Arc<SessionStore>, item: &QueuedInput) -> Self {
        Self {
            session_id: shared.record.id.clone(),
            content: item.input.content.clone(),
            persist_user: item.compact.is_none(),
            user_semantics: if item.notifications.is_empty() {
                UserMessageSemantics::UserPrompt
            } else {
                UserMessageSemantics::BackgroundNotification
            },
            turn_id: None,
            turn_started_at: 0,
            user_message_id: None,
            last_message_id: None,
            current: None,
            tool_error_count: 0,
            turn_total_tokens: 0,
            last_step_usage: None,
            model_retry_count: 0,
            model_request_rounds: None,
            persist_error: None,
            shared,
            store,
        }
    }

    /// goal-continuation turn 的 recorder：prompt 落库为 `system_reminder` /
    /// `source:"goal-continuation"`（观察 messageSummary）。
    fn goal_turn(shared: Arc<Shared>, store: Arc<SessionStore>, prompt: String) -> Self {
        Self {
            session_id: shared.record.id.clone(),
            content: prompt,
            persist_user: true,
            user_semantics: UserMessageSemantics::GoalContinuation,
            turn_id: None,
            turn_started_at: 0,
            user_message_id: None,
            last_message_id: None,
            current: None,
            tool_error_count: 0,
            turn_total_tokens: 0,
            last_step_usage: None,
            model_retry_count: 0,
            model_request_rounds: None,
            persist_error: None,
            shared,
            store,
        }
    }

    /// 后台通知 turn 的 recorder：user 行落库为 `background_notification` /
    /// `source:"background_task"`（C3 §3.1 的 messageSummary）。
    fn notification_turn(shared: Arc<Shared>, store: Arc<SessionStore>, input: String) -> Self {
        Self {
            session_id: shared.record.id.clone(),
            content: input,
            persist_user: true,
            user_semantics: UserMessageSemantics::BackgroundNotification,
            turn_id: None,
            turn_started_at: 0,
            user_message_id: None,
            last_message_id: None,
            current: None,
            tool_error_count: 0,
            turn_total_tokens: 0,
            last_step_usage: None,
            model_retry_count: 0,
            model_request_rounds: None,
            persist_error: None,
            shared,
            store,
        }
    }

    fn turn_id(&self) -> Option<&String> {
        self.turn_id.as_ref()
    }

    fn turn_total_tokens(&self) -> u64 {
        self.turn_total_tokens
    }

    /// 从锁内状态取下一条 message `sequence`（写顺序即分配顺序）。
    fn take_sequence(&self) -> i64 {
        let mut live = self.shared.lock();
        let sequence = live.next_message_sequence;
        live.next_message_sequence += 1;
        sequence
    }

    fn record_store_failure(&mut self, error: session_store::StoreError) {
        if self.persist_error.is_none() {
            self.persist_error = Some(error.to_string());
        }
    }

    fn observe(&mut self, event: &SessionEvent) {
        self.track_tool_window(event);
        match event.event_type {
            SessionEventType::TurnStarted => self.on_turn_started(event),
            SessionEventType::ModelStreaming => self.on_model_streaming(event),
            SessionEventType::ToolUpdated => self.on_tool_updated(event),
            SessionEventType::TurnCompleted => self.on_turn_completed(event),
            SessionEventType::TurnFailed => self.on_turn_failed(event),
            SessionEventType::SessionUpdated => self.on_session_updated(event),
            SessionEventType::StreamRecoveryUpdated => self.on_stream_recovery_updated(event),
            _ => {}
        }
    }

    /// 前台工具执行窗口（C3 T3）：`started` 置位，整批 `batch` 清除。
    ///
    /// `TaskRegistry` 的完成通知在另一个线程上读这个原子位；窗口内完成的通知按
    /// C1 观察丢弃，窗口外的通知入队等 worker 合并投递。turn 起止也清除，
    /// 防止异常路径把窗口留到下一个 turn。
    fn track_tool_window(&self, event: &SessionEvent) {
        match event.event_type {
            SessionEventType::TurnStarted
            | SessionEventType::TurnCompleted
            | SessionEventType::TurnFailed => {
                self.shared.tool_execution.store(false, Ordering::SeqCst);
            }
            SessionEventType::ToolUpdated => {
                if let Ok(Some(payload)) = event.payload_as::<ToolUpdatedPayload>() {
                    match payload {
                        ToolUpdatedPayload::Started(_) => {
                            self.shared.tool_execution.store(true, Ordering::SeqCst);
                        }
                        ToolUpdatedPayload::Batch(_) => {
                            self.shared.tool_execution.store(false, Ordering::SeqCst);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// 重试态落库事实：`model_retry_scheduled` 次数 = `turn_usage.model_retry_count`。
    /// stream recovery 事件不是 adapter 重试（夹具 §2.2 的 retry_count 为 0）。
    fn on_session_updated(&mut self, event: &SessionEvent) {
        if event
            .payload
            .as_ref()
            .is_some_and(|payload| payload["type"] == json!("model_retry_scheduled"))
        {
            self.model_retry_count += 1;
        }
    }

    /// 流恢复的丢弃边界：把当前未完成的 assistant 步持久化为
    /// `finish:"stream_recovery_discarded"`（text 空），恢复后的 delta 另起一条。
    ///
    /// 触发键是夹具的 discard 载荷（`discardedTextBytes` + `assistantMessageId`）；
    /// C1 的工具结果锚点（`kind:"tool_result"`）没有该键，不受影响。
    fn on_stream_recovery_updated(&mut self, event: &SessionEvent) {
        let Some(payload) = event.payload.as_ref() else {
            return;
        };
        if payload.get("discardedTextBytes").is_none() {
            return;
        }
        let Some(step) = self.current.take() else {
            return;
        };
        let expected = payload.get("assistantMessageId").and_then(Value::as_str);
        if expected.is_some_and(|expected| expected != step.event_message_id) {
            // 事件与当前步不一致时不能把错误的消息标记为丢弃：放回并留给
            // 下一条 model.streaming 的边界收口。
            self.current = Some(step);
            return;
        }
        // 被丢弃的 partial 文本/工具调用不进入历史（夹具：text 空、无部分文本 part）。
        let mut discarded = step;
        discarded.text.clear();
        discarded.tools.clear();
        self.persist_assistant_step(discarded, "stream_recovery_discarded");
    }

    fn on_turn_started(&mut self, event: &SessionEvent) {
        if self.turn_id.is_some() {
            return;
        }
        self.turn_id = event.turn_id.clone();
        self.turn_started_at = event.timestamp;
        // `projection.currentTurnId`：本 runtime 生命周期内最近一次已开始的 turn
        // （T0 观察的 read 视图在 turn 终态后仍带该键；resume 后无）。
        if let Some(turn_id) = &event.turn_id {
            self.shared.lock().current_turn_id = Some(turn_id.clone());
        }
        if self.persist_user {
            let content = self.content.clone();
            self.persist_user_message(&content);
        }
    }

    fn on_model_streaming(&mut self, event: &SessionEvent) {
        let payload = match event.payload_as::<ModelStreamingPayload>() {
            Ok(Some(payload)) => payload,
            _ => return,
        };
        let event_message_id = payload
            .assistant_message_id
            .clone()
            .unwrap_or_else(|| fresh_id("assistant"));
        // 下一步的流已经开始 = 上一步（无论 batch 是否被观察到）已完成：防御性收口，
        // 保证「每条 assistant 消息都在它完成的边界落库」。
        if self
            .current
            .as_ref()
            .map(|step| step.event_message_id != event_message_id)
            .unwrap_or(false)
        {
            let step = self.current.take().expect("上面已判定 Some");
            let finish = if step.tools.is_empty() {
                "stop"
            } else {
                "tool-calls"
            };
            self.persist_assistant_step(step, finish);
        }
        let step = self.current.get_or_insert(AssistantStep {
            event_message_id: event_message_id.clone(),
            started_at: event.timestamp,
            text: String::new(),
            tools: Vec::new(),
        });
        match payload.kind {
            ModelStreamingKind::TextDelta => {
                if let Some(delta) = &payload.delta {
                    step.text.push_str(delta);
                }
            }
            ModelStreamingKind::ToolCall => {
                if let (Some(tool_call_id), Some(tool_name)) =
                    (&payload.tool_call_id, &payload.tool_name)
                {
                    step.tools.push(ToolCallRecord {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: tool_name.clone(),
                        input: payload.input.clone().unwrap_or(Value::Null),
                        started_at: None,
                        finished_at: None,
                        read_only: None,
                        side_effect_scope: None,
                        serialization: None,
                        outcome: None,
                    });
                }
            }
            _ => {}
        }
    }

    fn on_tool_updated(&mut self, event: &SessionEvent) {
        let payload = match event.payload_as::<ToolUpdatedPayload>() {
            Ok(Some(payload)) => payload,
            _ => return,
        };
        match payload {
            ToolUpdatedPayload::Scheduled(scheduled) => {
                // 调度事件不带 input（spec §1.6）；工具记录已在 model.streaming 的
                // tool_call 处建立，这里只兜底防漏。
                self.tool_record_mut(&scheduled.call.tool_call_id, Some(&scheduled.tool_name));
            }
            ToolUpdatedPayload::Started(started) => {
                if let Some(record) = self.tool_record_mut(&started.call.tool_call_id, None) {
                    record.read_only = started.read_only;
                    record.side_effect_scope = started.side_effect_scope.clone();
                    record.started_at = Some(event.timestamp);
                }
            }
            ToolUpdatedPayload::Result(result) => {
                let content = result
                    .result
                    .content
                    .as_ref()
                    .map(|value| match value {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                if let Some(record) = self.tool_record_mut(&result.call.tool_call_id, None) {
                    record.outcome = Some(Ok(content));
                    record.finished_at = Some(event.timestamp);
                    // `result_object` 把 serialization 的四个键平铺在 result 上；
                    // DB tool part 的 metadata.serialization 是它们的对象形状。
                    let extras = &result.result.extra;
                    if let (Some(truncated), Some(original), Some(returned), Some(strategy)) = (
                        extras.get("truncated"),
                        extras.get("originalBytes"),
                        extras.get("returnedBytes"),
                        extras.get("budgetStrategy"),
                    ) {
                        record.serialization = Some(json!({
                            "truncated": truncated,
                            "originalBytes": original,
                            "returnedBytes": returned,
                            "budgetStrategy": strategy,
                        }));
                    }
                }
            }
            ToolUpdatedPayload::Error(error) => {
                self.tool_error_count += 1;
                if let Some(record) = self.tool_record_mut(&error.call.tool_call_id, None) {
                    record.outcome = Some(Err(error.error.message.clone()));
                    record.finished_at = Some(event.timestamp);
                }
            }
            ToolUpdatedPayload::Batch(_) => {
                // 批次 = 这条 assistant 消息的完成边界：原子落库。
                if let Some(step) = self.current.take() {
                    self.persist_assistant_step(step, "tool-calls");
                }
            }
            ToolUpdatedPayload::Progress(_) => {}
        }
    }

    fn on_turn_completed(&mut self, event: &SessionEvent) {
        let payload = match event.payload_as::<TurnCompletedPayload>() {
            Ok(Some(payload)) => payload,
            _ => return,
        };
        self.turn_total_tokens = payload.token_count;
        let cancelled =
            payload.result_type == agent_protocol::session_event::TurnResultType::Cancelled;
        if cancelled {
            // 取消的步没有「完整的一步」可落库；usage 行如实记录取消。
            self.record_turn_usage("cancelled", true, Some(&payload));
        } else {
            if let Some(step) = self.current.take() {
                self.persist_assistant_step(step, "stop");
            }
            self.record_turn_usage("completed", false, Some(&payload));
        }
        self.touch_session();
    }

    /// 失败 turn 的 usage 行（Task 7 控制器裁决 6）。
    ///
    /// 由 worker 在补发 `turn.failed` 后**显式**调用：错误自带的 `TurnProgress` 是
    /// 失败时刻的已知累计，按它写行（配置失败为全零——没有模型请求可累计）。
    /// `failure` 是线上观察到的错误面分类（C3 §2.4：provider 失败记
    /// `unknown_error` / `UNKNOWN_ERROR`）；配置失败传 `None`。
    /// 事件观察侧（`observe`）不再写 failed 行，避免同一 turn 落两行。
    fn record_failure_progress(
        &mut self,
        progress: &agent_core::TurnProgress,
        failure: Option<(&str, &str)>,
    ) {
        if self.turn_id.is_none() {
            return;
        }
        let (error_type, error_code) = failure
            .map(|(error_type, error_code)| {
                (Some(error_type.to_string()), Some(error_code.to_string()))
            })
            .unwrap_or((None, None));
        let record = TurnUsageRecord {
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone().unwrap_or_default(),
            started_at: self.turn_started_at.max(1),
            status: "failed".to_string(),
            completed_at: Some(now_ms()),
            duration_ms: Some((now_ms() - self.turn_started_at.max(1)).max(0)),
            cancelled_by_user: false,
            input_tokens: progress.usage.input_tokens as i64,
            output_tokens: progress.usage.output_tokens as i64,
            computed_total_tokens: (progress.usage.input_tokens + progress.usage.output_tokens)
                as i64,
            model_request_count: progress.model_requests as i64,
            model_retry_count: self.model_retry_count as i64,
            tool_call_count: progress.tool_calls as i64,
            tool_error_count: self.tool_error_count as i64,
            user_message_id: self.user_message_id.clone(),
            error_type,
            error_code,
            ..TurnUsageRecord::default()
        };
        if let Err(error) = self.store.record_turn_usage(record) {
            self.record_store_failure(error);
        }
        self.touch_session();
    }

    fn on_turn_failed(&mut self, _event: &SessionEvent) {
        // provider 错误：没有完成的 assistant 步可落库（本步累积内容不写半成品行）。
        // usage 行由 `record_failure_progress` 在 worker 收口处写（带已知累计）。
    }

    /// 按 toolCallId 找当前步的工具记录；找不到且给了工具名就补建。
    fn tool_record_mut(
        &mut self,
        tool_call_id: &str,
        tool_name: Option<&str>,
    ) -> Option<&mut ToolCallRecord> {
        let step = self.current.as_mut()?;
        if let Some(position) = step
            .tools
            .iter()
            .position(|record| record.tool_call_id == tool_call_id)
        {
            return step.tools.get_mut(position);
        }
        let tool_name = tool_name?;
        step.tools.push(ToolCallRecord {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            input: Value::Null,
            started_at: None,
            finished_at: None,
            read_only: None,
            side_effect_scope: None,
            serialization: None,
            outcome: None,
        });
        step.tools.last_mut()
    }

    fn persist_user_message(&mut self, content: &str) {
        let now = now_ms();
        let message_id = fresh_id("msg");
        let sequence = self.take_sequence();
        let turn_id = self.turn_id.clone().unwrap_or_default();
        // 落库行的模型事实取写入时刻的配置（setter 改模型后，新行记新值）。
        let config = self.shared.read_config();
        let semantics = match self.user_semantics {
            UserMessageSemantics::UserPrompt => json!({
                "origin": "real_user",
                "kind": "user_prompt",
                "uiVisibility": "visible",
                "providerVisibility": "visible",
                "transcriptVisibility": "visible",
            }),
            UserMessageSemantics::GoalContinuation => json!({
                "origin": "agent_runtime",
                "kind": "system_reminder",
                "source": "goal-continuation",
                "uiVisibility": "hidden",
                "providerVisibility": "visible",
                "transcriptVisibility": "hidden",
            }),
            UserMessageSemantics::BackgroundNotification => json!({
                "origin": "agent_runtime",
                "kind": "background_notification",
                "source": "background_task",
                "uiVisibility": "hidden",
                "providerVisibility": "visible",
                "transcriptVisibility": "hidden",
            }),
        };
        let mut data = json!({
            "role": "user",
            "messageId": message_id,
            "sequence": sequence,
            "time": { "created": now },
            "model": {
                "providerID": config.provider_id,
                "modelID": config.model_id,
            },
            "semantics": semantics,
        });
        if !turn_id.is_empty() {
            data["anchor"] = json!({ "turnId": turn_id, "origin": "realUser" });
        }
        let message = MessageRecord {
            id: message_id.clone(),
            session_id: self.session_id.clone(),
            time_created: now,
            time_updated: now,
            data,
            sequence: Some(sequence),
        };
        let part = PartRecord {
            id: fresh_id("part"),
            message_id: message_id.clone(),
            session_id: self.session_id.clone(),
            time_created: now,
            time_updated: now,
            data: json!({
                "type": "text",
                "text": content,
                "time": { "start": now, "end": now },
            }),
            sequence: Some(0),
        };
        match self.store.append_message_with_parts(message, &[part]) {
            Ok(()) => {
                self.user_message_id = Some(message_id.clone());
                self.last_message_id = Some(message_id);
            }
            Err(error) => self.record_store_failure(error),
        }
    }

    /// `turn.failed` 的 assistant 占位行（夹具 §2.4：`finish:null`、无文本 part）。
    ///
    /// 只在认证失败（401/403）时调用：夹具观察到失败会话留下一个空 assistant 占位，
    /// 而不是把失败的那一步写成半成品。其它失败路径保持「没有可提交消息」的既有
    /// 形状（C1 登记）。
    fn persist_failed_placeholder(&mut self) {
        let Some(turn_id) = self.turn_id.clone() else {
            return;
        };
        let now = now_ms();
        let message_id = fresh_id("msg");
        let sequence = self.take_sequence();
        let parent_id = self
            .last_message_id
            .clone()
            .or_else(|| self.user_message_id.clone())
            .unwrap_or_default();
        let config = self.shared.read_config();
        let data = json!({
            "role": "assistant",
            "messageId": message_id,
            "sequence": sequence,
            "parentID": parent_id,
            "finish": Value::Null,
            "providerId": config.provider_id,
            "modelId": config.model_id,
            "mode": config.mode,
            "time": { "created": now, "completed": now },
            "anchor": { "turnId": turn_id },
            "semantics": {
                "origin": "agent_runtime",
                "kind": "assistant_response",
                "uiVisibility": "visible",
                "providerVisibility": "visible",
                "transcriptVisibility": "visible",
            },
        });
        let message = MessageRecord {
            id: message_id.clone(),
            session_id: self.session_id.clone(),
            time_created: now,
            time_updated: now,
            data,
            sequence: Some(sequence),
        };
        match self.store.append_message_with_parts(message, &[]) {
            Ok(()) => self.last_message_id = Some(message_id),
            Err(error) => self.record_store_failure(error),
        }
    }

    /// 一条 assistant 消息 + 它的 parts（step-start / [text] / tool ×N / step-finish）
    /// 原子落库；工具结果同时写 `tool_usage`。
    ///
    /// `tokens` 取该步骤对应的**模型请求** usage（`last_step_usage`）。缺失时（没有
    /// 观察到的请求用量）不写 tokens，而不是拿 turn 聚合值硬拆。
    fn persist_assistant_step(&mut self, step: AssistantStep, finish: &str) {
        let tokens = self
            .last_step_usage
            .take()
            .map(|usage| tokens_value_from_usage(&usage));
        let now = now_ms();
        let message_id = fresh_id("msg");
        let sequence = self.take_sequence();
        let turn_id = self.turn_id.clone().unwrap_or_default();
        let parent_id = self
            .last_message_id
            .clone()
            .unwrap_or_else(|| self.user_message_id.clone().unwrap_or_default());
        let config = self.shared.read_config();
        let mut data = json!({
            "role": "assistant",
            "messageId": message_id,
            "sequence": sequence,
            "parentID": parent_id,
            "finish": finish,
            "providerId": config.provider_id,
            "modelId": config.model_id,
            "mode": config.mode,
            "time": { "created": step.started_at, "completed": now },
            "anchor": { "turnId": turn_id },
            "semantics": {
                "origin": "agent_runtime",
                "kind": "assistant_response",
                "uiVisibility": "visible",
                "providerVisibility": "visible",
                "transcriptVisibility": "visible",
            },
        });
        if let Some(tokens) = &tokens {
            data["tokens"] = tokens.clone();
        }
        let message = MessageRecord {
            id: message_id.clone(),
            session_id: self.session_id.clone(),
            time_created: step.started_at.max(1),
            time_updated: now,
            data,
            sequence: Some(sequence),
        };

        let mut parts: Vec<PartRecord> = Vec::with_capacity(step.tools.len() + 3);
        let mut part_sequence = 0_i64;
        let mut push_part = |parts: &mut Vec<PartRecord>, data: Value, time: i64| {
            parts.push(PartRecord {
                id: fresh_id("part"),
                message_id: message_id.clone(),
                session_id: self.session_id.clone(),
                time_created: time,
                time_updated: time,
                data,
                sequence: Some(part_sequence),
            });
            part_sequence += 1;
        };
        push_part(
            &mut parts,
            json!({ "type": "step-start" }),
            step.started_at.max(1),
        );
        if !step.text.is_empty() {
            let text = step.text.clone();
            push_part(
                &mut parts,
                json!({
                    "type": "text",
                    "text": text,
                    "time": { "start": step.started_at.max(1), "end": now },
                }),
                now,
            );
        }
        for call in &step.tools {
            let tool_part = json!({
                "type": "tool",
                "callID": call.tool_call_id,
                "tool": call.tool_name,
                "state": tool_state_value(call, now),
            });
            push_part(&mut parts, tool_part, now);
        }
        let mut step_finish = json!({ "type": "step-finish", "reason": finish, "cost": 0 });
        if let Some(tokens) = &tokens {
            step_finish["tokens"] = tokens.clone();
        }
        let step_finish_time = now;
        push_part(&mut parts, step_finish, step_finish_time);

        if let Err(error) = self.store.append_message_with_parts(message, &parts) {
            self.record_store_failure(error);
            return;
        }
        self.last_message_id = Some(message_id);

        for call in &step.tools {
            let (status, error_message) = match &call.outcome {
                Some(Ok(_)) => ("completed", None),
                Some(Err(message)) => ("error", Some(message.clone())),
                None => ("pending", None),
            };
            let started = call.started_at.unwrap_or(self.turn_started_at.max(1));
            let finished = call.finished_at.unwrap_or(now);
            let record = ToolUsageRecord {
                id: fresh_id("tu"),
                session_id: self.session_id.clone(),
                tool_call_id: call.tool_call_id.clone(),
                tool_name: call.tool_name.clone(),
                started_at: started,
                status: status.to_string(),
                completed_at: Some(finished),
                duration_ms: Some((finished - started).max(0)),
                error_message,
                read_only: call.read_only,
                side_effect_scope: call.side_effect_scope.clone(),
                turn_id: self.turn_id.clone(),
                ..ToolUsageRecord::default()
            };
            if let Err(error) = self.store.record_tool_usage(record) {
                self.record_store_failure(error);
            }
        }
    }

    fn record_turn_usage(
        &mut self,
        status: &str,
        cancelled: bool,
        payload: Option<&TurnCompletedPayload>,
    ) {
        // turn 尚未开始（无归属 turnId）时不写 usage 行——主键 (session_id, turn_id)
        // 用空串会破坏后续真实 turn 的对账。
        let Some(turn_id) = self.turn_id.clone() else {
            return;
        };
        let now = now_ms();
        let (input_tokens, output_tokens, computed_total, model_requests, tool_calls) = payload
            .map(|payload| {
                let usage = payload.usage.clone().unwrap_or(Value::Null);
                (
                    usage_value(&usage, "inputTokens"),
                    usage_value(&usage, "outputTokens"),
                    payload.token_count as i64,
                    // 取消终态的事件载荷按夹具记 `historyRoundCount:0`；DB 行用
                    // `model_request_rounds` 内部事实（取消示例为 1）。
                    self.model_request_rounds
                        .map(|rounds| rounds as i64)
                        .unwrap_or_else(|| payload.history_round_count.unwrap_or(0) as i64),
                    payload.tool_call_count as i64,
                )
            })
            .unwrap_or((0, 0, 0, 0, 0));
        let record = TurnUsageRecord {
            session_id: self.session_id.clone(),
            turn_id,
            started_at: self.turn_started_at.max(1),
            status: status.to_string(),
            completed_at: Some(now),
            duration_ms: Some((now - self.turn_started_at.max(1)).max(0)),
            cancelled_by_user: cancelled,
            input_tokens,
            output_tokens,
            computed_total_tokens: computed_total,
            model_request_count: model_requests,
            // C3 T2：adapter 重试次数（stream recovery 不计入；夹具 §2.2/§2.3）。
            model_retry_count: self.model_retry_count as i64,
            tool_call_count: tool_calls,
            tool_error_count: self.tool_error_count as i64,
            user_message_id: self.user_message_id.clone(),
            // 观察（C3 §1.6 user stop / pause）：取消行带 `turn_cancelled`。
            error_type: cancelled.then(|| "turn_cancelled".to_string()),
            error_code: cancelled.then(|| "TURN_CANCELLED".to_string()),
            ..TurnUsageRecord::default()
        };
        if let Err(error) = self.store.record_turn_usage(record) {
            self.record_store_failure(error);
        }
    }

    fn touch_session(&mut self) {
        if let Err(error) = self.store.touch_session(&self.session_id, now_ms()) {
            self.record_store_failure(error);
        }
    }
}

impl EventSink for TurnRecorder {
    fn emit(&mut self, event: SessionEvent) {
        // 先观察落库，再发布（Task 8 起倒转顺序）：turn 跑在 `spawn_blocking` 线程上
        // 后，订阅者与本线程**并行**，publish-then-persist 会让「看到 turn.completed
        // 的订阅者读不到刚落库的行」（Task 7 之前单线程执行，publish→persist 之间
        // 没有任何任务能让出，窗口不可观察）。倒转后「事件可见 ⇒ 行已提交」，
        // 代价是实时事件落后落库一个本地 sqlite 写（短事务，亚毫秒级）——一致性
        // 优先于这点延迟。落库失败仍不改变事件流（实时事件优先于存储失败面）。
        self.observe(&event);
        self.shared.publish(&event);
    }

    fn model_usage(&mut self, usage: &agent_core::provider::ModelUsage) {
        // 请求完成的 usage 只交给落库侧（assistant 步骤 tokens），不进入事件流。
        self.last_step_usage = Some(usage.clone());
        // `runtime.contextUsage` 的唯一运行期写入点：每个完成的请求累计一次。
        // 同一把锁里更新 `projection.contextUsed`（最近一次请求 total；T0 观察它
        // 跟随 used=22 而不是累计 44/66）。
        let request = RequestUsage::from_model_usage(usage);
        let mut live = self.shared.lock();
        live.usage.observe(&request);
        live.context_used = request.total_tokens;
    }

    fn model_request_rounds(&mut self, rounds: u64) {
        // DB/轮次口径的内部事实（C3 T2）：取消终态的事件载荷按夹具省略它。
        self.model_request_rounds = Some(rounds);
    }

    fn cancelled_placeholder(&mut self, model_requested: bool) {
        // 夹具（`retry-cancel-during-backoff` / `goal-pause-in-flight`）：已观察到
        // 模型请求的取消留下 `finish:null`、无 part 的 assistant 占位；请求前取消
        // 没有占位。钩子在终态发布前调用，保证事件可见时行已提交。
        if model_requested {
            self.persist_failed_placeholder();
        }
    }
}

/// `usage` 对象里的非负整数；缺失/非法按 0，不编数字。
fn usage_value(usage: &Value, key: &str) -> i64 {
    usage
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as i64)
        .unwrap_or(0)
}

/// 一套模型请求 usage → 持久化行的 tokens 形状（与 oracle DB 形状同构）。
///
/// 每个 assistant 步骤记的是**产生它的请求**的 usage：工具调用步骤 = 调用请求，
/// 收尾步骤 = 收尾请求。turn 聚合值只进 `turn_usage`。
fn tokens_value_from_usage(usage: &agent_core::provider::ModelUsage) -> Value {
    let total = usage.input_tokens + usage.output_tokens;
    json!({
        "total": total,
        "input": usage.input_tokens,
        "output": usage.output_tokens,
        "reasoning": usage.reasoning_tokens,
        "cache": {
            "read": usage.cache_read_tokens,
            "write": usage.cache_write_tokens,
        },
    })
}

/// tool part 的 `state`（观察形状见 `agent-server-c1-contract.md` §3.4：completed 带
/// `title`/`metadata`；`readFileState` 等工具私有 metadata 不在这里重建，保持空对象）。
/// `input` 恒为对象（线上形状；缺失入参投影成 `{}`）。
fn tool_state_value(call: &ToolCallRecord, now: i64) -> Value {
    let started = call.started_at.unwrap_or(now);
    let finished = call.finished_at.unwrap_or(now);
    let input = if call.input.is_null() {
        json!({})
    } else {
        call.input.clone()
    };
    match &call.outcome {
        Some(Ok(output)) => {
            // oracle 的 completed metadata 是 `schemaVersion` + `serialization`；
            // `readFileState` 等工具私有字段**不重建**（登记在 §C1.3-6）。
            let mut metadata = serde_json::Map::new();
            metadata.insert("schemaVersion".to_string(), json!(1));
            if let Some(serialization) = &call.serialization {
                metadata.insert("serialization".to_string(), serialization.clone());
            }
            json!({
                "status": "completed",
                "input": input,
                "output": output,
                "title": call.tool_name,
                "metadata": Value::Object(metadata),
                "time": { "start": started, "end": finished },
            })
        }
        Some(Err(message)) => json!({
            "status": "error",
            "input": input,
            "error": message,
            "title": call.tool_name,
            "metadata": {},
            "time": { "start": started, "end": finished },
        }),
        None => json!({
            "status": "pending",
            "input": input,
        }),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
