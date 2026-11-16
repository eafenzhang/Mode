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
use agent_core::session::Session;
use agent_core::{CancelToken, EventSink, ModelProvider, SessionEvent, SessionEventType};
use agent_protocol::session_event::{
    ModelStreamingKind, ModelStreamingPayload, ToolUpdatedPayload, TurnCompletedPayload,
};
use serde_json::{json, Value};
use session_store::{
    MessageRecord, PartRecord, SessionRecord, SessionStore, ToolUsageRecord, TurnUsageRecord,
};

use crate::reverse::{RuntimePreferences, RuntimeWiring};
use crate::state::{
    build_snapshot, fresh_id, Accepted, LiveView, RuntimeConfig, RuntimeError, RuntimeStatus,
    SessionInput,
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
}

/// worker 队列的一项：已接受、等待串行执行的输入。
struct QueuedInput {
    input: SessionInput,
    input_id: String,
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
            }),
            config: RwLock::new(config),
            bus,
            drained,
            preferences: RwLock::new(None),
            record,
        });

        let mut session = Session::new(shared.record.id.clone(), shared.record.directory.clone())
            .with_mode(shared.read_config().mode)
            .with_model_capabilities(shared.read_config().capabilities);
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
        live.seen_inputs.insert(input_id.clone(), accepted.clone());
        self.queue
            .send(QueuedInput { input, input_id })
            .map_err(|_| RuntimeError::RuntimeClosed)?;
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
        let live = self.shared.lock();
        let replay: Vec<SessionEvent> = live
            .log
            .iter()
            .filter(|event| match after_seq {
                None => true,
                Some(after) => event.seq > after,
            })
            .cloned()
            .collect();
        let receiver = self.shared.bus.subscribe();
        (replay, receiver)
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

    /// 最小合法快照（形状对齐 `agent-process-fixture::minimal_snapshot`）：
    /// mode / model / status / turnCount / usage 反映活跃状态；`messages` 恒空
    /// （历史消息由 Task 7 的 `session/read` / `session/resume` 从 store 投影）。
    pub fn snapshot(&self) -> agent_protocol::snapshot::SessionStateSnapshot {
        let live = self.shared.lock();
        let view = LiveView {
            status: live.status,
            revision: live.revision,
            updated_at: live.updated_at,
            last_error: live.last_error.clone(),
            turn_count: live.turn_count,
            total_token_count: live.total_token_count,
            event_seq: live.event_seq,
        };
        let config = self.shared.read_config();
        build_snapshot(&self.shared.record, &config, &view)
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
/// （物化即发；create 与冷 resume 各一次，send 不重发），应答/回退缓存进
/// [`Shared::preferences`]——首个输入的 turn 在此之后才开始（与 oracle 的 create
/// 等待偏好应答同序）。
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
    if let Some(reverse) = &reverse {
        let preferences = reverse.request_runtime_preferences(&shared.record.id).await;
        *shared
            .preferences
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(preferences);
    }
    let mut current: Option<(String, String, Arc<dyn ModelProvider>)> = None;
    while let Some(item) = receiver.recv().await {
        // 每个 turn 开始前同步配置：mode/能力进系统提示，模型事实决定装配。
        let config = shared.read_config();
        session.set_mode(config.mode.clone());
        session.set_model_capabilities(config.capabilities);

        let cached = current
            .as_ref()
            .filter(|(provider_id, model_id, _)| {
                provider_id == &config.provider_id && model_id == &config.model_id
            })
            .map(|(_, _, provider)| Arc::clone(provider));
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
                            Arc::clone(&provider),
                        ));
                        Some(provider)
                    }
                    Ok(Err(message)) => {
                        current = None;
                        run_configuration_failure(&mut session, &shared, &store, &item, &message)
                            .await;
                        continue;
                    }
                    Err(join_error) => {
                        current = None;
                        let message = format!("provider 装配任务失败: {join_error}");
                        run_configuration_failure(&mut session, &shared, &store, &item, &message)
                            .await;
                        continue;
                    }
                }
            }
        };
        match provider {
            Some(provider) => {
                session = run_one_input(session, &shared, &store, provider, item).await;
            }
            // 没有配置模型：显式配置错误，不启动模型请求。
            None => {
                run_configuration_failure(
                    &mut session,
                    &shared,
                    &store,
                    &item,
                    "Select a model before continuing",
                )
                .await;
            }
        }
    }
    // 队列清空且发送端关闭：广播 drain 完成信号（closing 台账据此移除条目）。
    let _ = shared.drained.send(true);
}

/// 配置失败的 turn：`turn.started` → `turn.failed`（`CONFIGURATION_ERROR`），
/// `turn_usage(failed)` 记零值（没有任何模型请求可累计）。终态恰好一条，
/// 与正常 turn 走同一套 publish / 状态收口。
async fn run_configuration_failure(
    session: &mut Session,
    shared: &Arc<Shared>,
    store: &Arc<SessionStore>,
    item: &QueuedInput,
    message: &str,
) {
    let token = CancelToken::new();
    {
        let mut live = shared.lock();
        live.status = RuntimeStatus::Running;
        live.current_cancel = Some(token.clone());
    }
    let mut recorder = TurnRecorder::new(Arc::clone(shared), Arc::clone(store), item);
    let started = session.begin_turn(&item.input.content);
    let turn_id = started.turn_id.clone();
    recorder.emit(started);
    recorder.emit(session.emit_turn_failed(
        turn_id.as_deref(),
        "CONFIGURATION_ERROR",
        message,
        "configuration",
    ));
    recorder.record_failure_progress(&agent_core::TurnProgress::default());

    let mut live = shared.lock();
    live.current_cancel = None;
    let completion = live.inbox.complete(&shared.record.id, &item.input_id);
    live.turn_count += 1;
    live.revision += 1;
    live.updated_at = now_ms();
    // 与 `run_one_input` 的失败分支同形（失败恒 Error；Stale 不动状态），
    // 保证两条收口路径的口径一致。
    live.status = match completion {
        Completion::Promoted(_) | Completion::Idle => RuntimeStatus::Error,
        Completion::Stale => live.status,
    };
    live.last_error = Some(message.to_string());
}

/// 跑一个输入（串行；前后维护 running 状态与取消令牌）。
///
/// turn 执行体**连同终态收口**在 `spawn_blocking` 线程上经 `Handle::block_on` 驱动
/// （Task 8 控制器裁决 1）：同步 [`PermissionHandler`] 的审批等待（反向请求）会
/// **阻塞**当前线程，而 stdio 单写者循环必须留在 tokio worker 上继续写帧/读应答——
/// 阻塞线程 + runtime 线程分工，current-thread 测试 runtime 下也不死锁
/// （`spawn_blocking` 的线程不进 runtime 的 worker 池，`block_on` 在其上合法）。
/// 收口放在同一闭包里：终态事件发布时状态已收口（先前「同任务内 publish→收口→
/// 才让出」的观察顺序不因搬运而改变，订阅者不会看到 `turn.completed` 后快照仍是
/// Running 的窗口）。
///
/// 终态收口：`run_turn` 的 `Err(cancelled)` 表示终态已由它自己发出
/// （`turn.completed`，`resultType:"cancelled"`），worker 不补发；其余 `Err` 由
/// 本闭包补发唯一的 `turn.failed`。成功与取消路径的终态事件经 recorder 走同一套
/// publish 与 persist。返回会话（阻塞任务的所有权往返：Session/recorder 移入、
/// 移出）。
async fn run_one_input(
    session: Session,
    shared: &Arc<Shared>,
    store: &Arc<SessionStore>,
    provider: Arc<dyn ModelProvider>,
    item: QueuedInput,
) -> Session {
    let token = CancelToken::new();
    {
        let mut live = shared.lock();
        live.status = RuntimeStatus::Running;
        live.current_cancel = Some(token.clone());
    }

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
            let result = session
                .run_turn(&content, provider.as_ref(), &mut recorder, &token)
                .await;

            // —— 终态收口（与旧内联路径逐行同序，只是在阻塞线程上）——
            let failure_message = match &result {
                Ok(()) => None,
                // 取消：终态已由 run_turn 发出，不再补第二个终态。
                Err(error) if error.is_cancelled() => None,
                Err(error) => {
                    let event = session.emit_turn_failed(
                        recorder.turn_id().map(String::as_str),
                        "runtime_error",
                        &error.message,
                        "model",
                    );
                    recorder.emit(event);
                    // Task 7 控制器裁决 6：失败 turn 的 usage 行记**已知**累计值，不清零。
                    recorder.record_failure_progress(&error.progress);
                    Some(error.message.clone())
                }
            };
            let mut live = shared_for_turn.lock();
            live.current_cancel = None;
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

/// 一次 turn 的落库记录器：[`EventSink`] 的实现，先发布事件再观察落库。
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
    outcome: Option<Result<String, String>>,
}

impl TurnRecorder {
    fn new(shared: Arc<Shared>, store: Arc<SessionStore>, item: &QueuedInput) -> Self {
        Self {
            session_id: shared.record.id.clone(),
            content: item.input.content.clone(),
            turn_id: None,
            turn_started_at: 0,
            user_message_id: None,
            last_message_id: None,
            current: None,
            tool_error_count: 0,
            turn_total_tokens: 0,
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
        let content = self.content.clone();
        self.persist_user_message(&content);
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
            self.persist_assistant_step(step, finish, None);
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
                    self.persist_assistant_step(step, "tool-calls", None);
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
            let tokens = tokens_value_from_payload(&payload);
            if let Some(step) = self.current.take() {
                self.persist_assistant_step(step, "stop", Some(tokens));
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
    /// `tokens` 只在终态（成功）时给出——per-request 的用量事件里没有，不拆分、不编造。
    fn persist_assistant_step(&mut self, step: AssistantStep, finish: &str, tokens: Option<Value>) {
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
        let mut step_finish = json!({ "type": "step-finish", "reason": finish });
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
}

/// `usage` 对象里的非负整数；缺失/非法按 0，不编数字。
fn usage_value(usage: &Value, key: &str) -> i64 {
    usage
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as i64)
        .unwrap_or(0)
}

/// `turn.completed` 载荷的 usage → 持久化行的 tokens 形状（与 oracle DB 形状同构）。
fn tokens_value_from_payload(payload: &TurnCompletedPayload) -> Value {
    let usage = payload.usage.clone().unwrap_or(Value::Null);
    json!({
        "total": payload.token_count,
        "input": usage_value(&usage, "inputTokens"),
        "output": usage_value(&usage, "outputTokens"),
        "reasoning": usage_value(&usage, "reasoningTokens"),
        "cache": {
            "read": usage_value(&usage, "cacheReadTokens"),
            "write": usage_value(&usage, "cacheWriteTokens"),
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
        Some(Ok(output)) => json!({
            "status": "completed",
            "input": input,
            "output": output,
            "title": call.tool_name,
            "metadata": {},
            "time": { "start": started, "end": finished },
        }),
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
