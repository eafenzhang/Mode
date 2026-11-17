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

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::command_inbox::{CommandInbox, Completion};
use agent_core::compact::{compact_prompt, summary_continuation, validate_compact_response};
use agent_core::context::{ContextCacheUsage, ContextUsage};
use agent_core::provider::{ModelDelta, ModelMessage, ProviderError};
use agent_core::session::Session;
use agent_core::{
    estimate_history_tokens, CancelToken, EventSink, ModelProvider, ModelUsage, SessionEvent,
    SessionEventType, SUMMARY_TITLE,
};
use agent_protocol::session_event::{
    CacheStats, ModelStreamingKind, ModelStreamingPayload, ProtocolErrorDetail, ToolUpdatedPayload,
    TurnCompletedPayload, TurnFailedPayload, TurnResultType,
};
use agent_protocol::session_write::SessionCompactParams;
use agent_protocol::ExtraFields;
use serde_json::{json, Value};
use session_store::{
    MessageRecord, MessageWithParts, ModelUsageRecord, PartRecord, SessionRecord, SessionStore,
    ToolUsageRecord, TurnUsageRecord,
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
    event_seq: u64,
    /// 下一条持久化 message 行的 `sequence`（冷启动时按库内已有行的最大序号 +1 初始化，
    /// Task 7 控制器裁决 6：恢复历史时不得与旧行撞序号）。
    next_message_sequence: i64,
    /// 有界内存事件日志（订阅回放源；最旧先丢）。
    log: VecDeque<SessionEvent>,
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
    bus: tokio::sync::broadcast::Sender<SessionEvent>,
    drained: tokio::sync::watch::Sender<bool>,
    record: SessionRecord,
    preferences: RwLock<Option<RuntimePreferences>>,
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

    /// 发布一条事件：先进内存日志（同一临界区），再上 broadcast。
    ///
    /// 订阅方在**同一把锁内**「读日志 + 建 receiver」，因此：日志里已有的事件不会
    /// 再出现在 receiver 缓冲里（无重复），锁释放后发布的事件必进 receiver（无缺口）。
    fn publish(&self, event: &SessionEvent) {
        let mut live = self.lock();
        live.event_seq = live.event_seq.max(event.seq);
        live.log.push_back(event.clone());
        let capacity = self.read_config().event_log_capacity.max(1);
        while live.log.len() > capacity {
            live.log.pop_front();
        }
        // 没有订阅方时 send 返回 Err：事件仍在内存日志里，回放可补——不是错误。
        let _ = self.bus.send(event.clone());
    }
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

        let (bus, _bus_receiver) =
            tokio::sync::broadcast::channel(config.broadcast_capacity.max(1));
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
                event_seq: 0,
                next_message_sequence,
                log: VecDeque::new(),
                seen_inputs: HashMap::new(),
                inbox: CommandInbox::new(),
                current_cancel: None,
                current_kind: None,
                compact: CompactCoordinator::default(),
                usage,
            }),
            config: RwLock::new(config),
            bus,
            drained,
            preferences: RwLock::new(None),
            record,
        });

        let mut session = Session::new(shared.record.id.clone(), shared.record.directory.clone())
            .with_mode(shared.read_config().mode)
            .with_model_capabilities(shared.read_config().capabilities)
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
        let worker = tokio::spawn(worker_loop(
            receiver,
            Arc::clone(&shared),
            session,
            store,
            source,
            wiring.reverse,
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
        let live = self.shared.lock();
        let effective = match after_seq {
            Some(after) if after <= live.event_seq => Some(after),
            _ => None,
        };
        let replay: Vec<SessionEvent> = live
            .log
            .iter()
            .filter(|event| match effective {
                None => true,
                Some(after) => event.seq > after,
            })
            .cloned()
            .collect();
        let receiver = self.shared.bus.subscribe();
        (replay, receiver, effective)
    }

    /// 取消当前运行中的 turn（裁决 4）。空闲时是幂等 no-op；排队中的输入不受影响。
    pub fn cancel_turn(&self) -> Result<(), RuntimeError> {
        let live = self.shared.lock();
        if let Some(token) = &live.current_cancel {
            token.cancel();
        }
        Ok(())
    }

    /// 当前乐观并发版本（setter 的 `expectedRevision` 校验用）。
    pub fn state_revision(&self) -> u64 {
        self.shared.lock().revision
    }

    /// 当前事件 seq 高水位（`session/subscribe` 结果的 `eventSeq`）。
    pub fn event_seq(&self) -> u64 {
        self.shared.lock().event_seq
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

    /// 最小合法快照（形状对齐 `agent-process-fixture::minimal_snapshot`）：
    /// mode / model / status / turnCount / usage 反映活跃状态；`messages` 恒空
    /// （历史消息由 Task 7 的 `session/read` / `session/resume` 从 store 投影）。
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
            event_seq: live.event_seq,
            context_usage: live.usage.context_usage(config.capabilities.context_window),
            active_turn: live
                .compact
                .running_turn_id
                .clone()
                .map(|turn_id| (turn_id, TurnKind::Compact)),
        };
        build_snapshot(&self.shared.record, &config, &view)
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
    while let Some(item) = receiver.recv().await {
        // 取消令牌在**取到输入时**就登记（Task 10）：`session/close` 可能落在
        // 「准入已接受、turn 尚未开始」的等待窗口里（user-execution 偏好请求、
        // provider 装配的宿主钩子）。令牌登记晚于 close 时，`cancel_turn` 落在
        // `current_cancel: None` 上是一次 no-op——被 close 点名的运行中输入反而
        // 照常跑完，drain 等不到终态（methods 的 close-drain 用例因此挂死）。
        // 这里提前登记后，close 的取消必然命中本输入（终态 `cancelled`）；
        // 排队中的后续输入照常执行（裁决 4 不变）。
        let cancel = CancelToken::new();
        {
            let mut live = shared.lock();
            live.status = RuntimeStatus::Running;
            live.current_cancel = Some(cancel.clone());
            live.current_kind = Some(if item.compact.is_some() {
                TurnKind::Compact
            } else {
                TurnKind::Prompt
            });
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
                        run_configuration_failure(
                            &mut session,
                            &shared,
                            &store,
                            &item,
                            &message,
                            cancel,
                        )
                        .await;
                        continue;
                    }
                    Err(join_error) => {
                        current = None;
                        let message = format!("provider 装配任务失败: {join_error}");
                        run_configuration_failure(
                            &mut session,
                            &shared,
                            &store,
                            &item,
                            &message,
                            cancel,
                        )
                        .await;
                        continue;
                    }
                }
            }
        };
        match provider {
            Some(provider) if item.compact.is_some() => {
                session = run_compact(session, &shared, &store, provider, item).await;
            }
            Some(provider) => {
                session = run_one_input(session, &shared, &store, provider, item, cancel).await;
            }
            // 没有配置模型：显式配置错误，不启动模型请求（compact 任务同样走这支，
            // 终态是带 `"/compact …"` 展示串的 `turn.failed`；协调器在收口处复位）。
            None => {
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
    } else {
        session.begin_turn(&item.input.content)
    };
    let turn_id = started.turn_id.clone();
    recorder.emit(started);
    // 先落 usage 行再发布终态（最终评审 I3）：订阅者看到 `turn.failed` 时，
    // `turn_usage` 必须已经可读。
    recorder.record_failure_progress(&agent_core::TurnProgress::default());
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
            // fail-soft：不注入、turn 继续（`agent-c2-contract.md` §3）。
            let memory_enabled = shared_for_turn
                .preferences
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .map(|preferences| preferences.memory_enabled)
                .unwrap_or(false);
            let memory = shared_for_turn
                .read_config()
                .data_root
                .as_deref()
                .and_then(|data_root| {
                    crate::memory::load_memory_section(
                        data_root,
                        &shared_for_turn.record.directory,
                        memory_enabled,
                    )
                });
            session.set_memory(memory);
            let result = session
                .run_turn(&content, provider.as_ref(), &mut recorder, &token)
                .await;

            // —— 终态收口（与旧内联路径逐行同序，只是在阻塞线程上）——
            let failure_message = match &result {
                Ok(()) => None,
                // 取消：终态已由 run_turn 发出，不再补第二个终态。
                Err(error) if error.is_cancelled() => None,
                Err(error) => {
                    // Task 7 控制器裁决 6：失败 turn 的 usage 行记**已知**累计值，不清零。
                    // 先落 usage 行再发布终态（最终评审 I3）：看到 `turn.failed` 的
                    // 订阅者读 `turn_usage` 时必须已经有这一行。
                    recorder.record_failure_progress(&error.progress);
                    let event = session.emit_turn_failed(
                        recorder.turn_id().map(String::as_str),
                        "runtime_error",
                        &error.message,
                        "model",
                    );
                    recorder.emit(event);
                    Some(error.message.clone())
                }
            };
            let mut live = shared_for_turn.lock();
            live.current_cancel = None;
            live.current_kind = None;
            // running 完成对账；迟到的 complete 由 CommandInbox 自己挡（Stale 不动队列）。
            let completion = live.inbox.complete(&session_id, &input_id);
            live.turn_count += 1;
            live.revision += 1;
            live.updated_at = now_ms();
            live.total_token_count += recorder.turn_total_tokens();
            // Task 7 控制器裁决 6：队首被提升（还有已接受输入在等）时不能闪回 idle。
            live.status = match (&failure_message, completion) {
                (Some(_), _) => RuntimeStatus::Error,
                (None, Completion::Promoted(_)) => RuntimeStatus::Running,
                (None, Completion::Idle) => RuntimeStatus::Idle,
                // Stale：对账被拒（不应发生——worker 是唯一 complete 的写入方）；状态不动。
                (None, Completion::Stale) => live.status,
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
        Err(ProviderError { message }) => {
            return compact_failure(
                session,
                shared,
                store,
                facts,
                &job,
                CompactFailure {
                    message: &message,
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
struct TurnRecorder {
    shared: Arc<Shared>,
    store: Arc<SessionStore>,
    session_id: String,
    /// 本轮用户输入原文（`turn.started` 时落库）。
    content: String,
    /// 是否在 `turn.started` 写 user message 行（compact 任务恒 `false`：
    /// compact 没有 user 行 / `session_input` 行，契约 §1.3）。
    persist_user: bool,
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
            turn_id: None,
            turn_started_at: 0,
            user_message_id: None,
            last_message_id: None,
            current: None,
            tool_error_count: 0,
            turn_total_tokens: 0,
            last_step_usage: None,
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
        match event.event_type {
            SessionEventType::TurnStarted => self.on_turn_started(event),
            SessionEventType::ModelStreaming => self.on_model_streaming(event),
            SessionEventType::ToolUpdated => self.on_tool_updated(event),
            SessionEventType::TurnCompleted => self.on_turn_completed(event),
            SessionEventType::TurnFailed => self.on_turn_failed(event),
            _ => {}
        }
    }

    fn on_turn_started(&mut self, event: &SessionEvent) {
        if self.turn_id.is_some() {
            return;
        }
        self.turn_id = event.turn_id.clone();
        self.turn_started_at = event.timestamp;
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
    /// 事件观察侧（`observe`）不再写 failed 行，避免同一 turn 落两行。
    fn record_failure_progress(&mut self, progress: &agent_core::TurnProgress) {
        if self.turn_id.is_none() {
            return;
        }
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
            tool_call_count: progress.tool_calls as i64,
            tool_error_count: self.tool_error_count as i64,
            user_message_id: self.user_message_id.clone(),
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
        let mut data = json!({
            "role": "user",
            "messageId": message_id,
            "sequence": sequence,
            "time": { "created": now },
            "model": {
                "providerID": config.provider_id,
                "modelID": config.model_id,
            },
            "semantics": {
                "origin": "real_user",
                "kind": "user_prompt",
                "uiVisibility": "visible",
                "providerVisibility": "visible",
                "transcriptVisibility": "visible",
            },
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
                    payload.history_round_count.unwrap_or(0) as i64,
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
            tool_call_count: tool_calls,
            tool_error_count: self.tool_error_count as i64,
            user_message_id: self.user_message_id.clone(),
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
        self.shared
            .lock()
            .usage
            .observe(&RequestUsage::from_model_usage(usage));
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
