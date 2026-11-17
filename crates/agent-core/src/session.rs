//! 最小会话循环的**工具面**：一次 assistant 消息里的若干工具调用，按观察到的顺序发事件。
//!
//! # 覆盖范围（要说清楚，否则会以为它是「完整会话循环」）
//!
//! 本模块只做「工具执行」这一段。实测一次完整 turn 的事件序列（夹具 `read-single` 的
//! `expect.events`）是：
//!
//! ```text
//! session.titleUpdated → turn.started → session.updated ×2 → model.streaming ×4
//!   → tool.updated(scheduled) → tool.updated(started) → session.updated ×2
//!   → tool.updated(result) → tool.updated(batch) → streamRecovery.updated
//!   → session.updated ×2 → model.streaming → session.updated ×2 → turn.completed
//! ```
//!
//! 其中 `model.streaming`（模型输出）、`session.updated`（状态推送）、`streamRecovery.updated`
//! （恢复锚点）与 `turn.completed` 都不是工具面，且它们的载荷需要模型层数字
//! （`tokenCount`/`usage`/`cacheStats`）与追踪 id——本任务没有这些数据，**编一个数字比不发更糟**。
//! 所以这里只发：
//!
//! - [`Session::begin_turn`] → `turn.started`（只带工具层确实知道的 `turnNumber` 与 `input`）；
//! - [`Session::run_tool_batch`] → 每次调用 `tool.updated` 的 `scheduled` /（解析到工具时）
//!   `started` / `result`|`error`，以及整批的 `batch`。
//!
//! 终态事件、模型流与双链路重放归 Task 5。第三层（事件流）对拍因此**尚未**在只读切片上做：
//! 本任务的对拍判据是第一层（工具调用序列）与第二层（文件快照），见
//! `tests/read_only_parity.rs`。
//!
//! # 顺序与字段的出处
//!
//! 逐条来自夹具 `read-single` / `read-twice-one-turn` / `read-absent-search-tools` 的
//! `expect.events`：
//!
//! | 事实 | 观察值 |
//! | --- | --- |
//! | `scheduled` 不带入参 | `inputOmitted: true` + `inputRef: "model_stream"`（spec §1.6） |
//! | `scheduled` 还带 | `dependencies: []`、`parallelGroupIndex: 0`、`canRunParallel: true`、`schedule.parallelGroups`/`executionOrder`、`inputByteLength`（入参 JSON 文本的字节数） |
//! | `started` 的载荷 | `toolName`、`startedAt`、`readOnly`、`sideEffectScope`（只读工具是 `"none"`） |
//! | **未注册的工具没有 `started`** | `Glob`/`Grep`：`scheduled` 之后**直接** `error` |
//! | `batch` | 一条 assistant 消息一批：`toolCallIds` + `successCount` + `errorCount` |
//! | 事件信封 | `deliveryKind: "desktop-continuous"`，`seq` 单调，`eventId`/`traceId` 是 uuid |

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use agent_protocol::permission::{
    PermissionDecision, PermissionOption, PermissionOptionKind, PermissionRequest,
    PermissionResponse, PermissionRuleValue, PermissionUpdate, PermissionUpdateType, RuleBehavior,
};
use agent_protocol::session_event::{
    DeliveryKind, ModelStreamingKind, ModelStreamingPayload, ProtocolErrorDetail, ProtocolInstant,
    SessionEvent, SessionEventType, ToolBatchPayload, ToolCallIdentity, ToolErrorPayload,
    ToolResultPayload, ToolSchedule, ToolScheduledPayload, ToolStartedPayload, ToolUpdatedPayload,
    TurnCompletedPayload, TurnFailedPayload, TurnResultType, TurnStartedPayload,
};
use agent_protocol::ExtraFields;
use agent_tools::{
    ArtifactLocation, FileState, ModeToolFamily, TaskNotification, TaskRegistry, ToolContext,
    ToolOutcome, ToolSpec,
};

use crate::approval::Ruleset;
use crate::cancel::CancelToken;
use crate::permission::{AutoAllow, PermissionHandler, DENY_REASON};
use crate::plugin_tools::{PluginToolDeclaration, PluginToolSource};
use crate::provider::{
    ModelCapabilities, ModelDelta, ModelMessage, ModelProvider, ModelResponse, ModelUsage,
    ProviderError,
};
use crate::rule_store::{scope_id_for, RuleStore, SqliteRuleStore};
use crate::stream::{EventSink, TurnError};
use crate::tool::{error_object, result_object};

/// 模型流里的一条工具调用（`model.streaming` / `kind:"tool_call"` 的入参来源，spec §1.6）。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallRequest {
    pub tool_call_id: String,
    pub name: String,
    pub input: Value,
}

/// 会话循环：工具面 + 可选的模型驱动 turn（[`Session::run_turn`]）。
///
/// 不落盘：消息历史只在内存里，持久化归宿主。`web-remote-replayable` 的快照/队列
/// 归 Host/relay（DR-T5-1），本结构只保证 `desktop-continuous` 的实时事件顺序。
pub struct Session {
    session_id: String,
    cwd: String,
    turn_number: u64,
    sequence: u64,
    current_turn_id: Option<String>,
    /// 会话内的文件记忆：`Read` 的重复读去重（跨 turn 命中）+ `Write` 的「先读再写」前置条件。
    /// 见 `agent_tools::FileState`。
    file_state: FileState,
    /// `.mode` 数据根（`{base}/.mode`）。`None` = 本会话还没接线数据根，工具 artifact 落临时目录。
    data_root: Option<PathBuf>,
    /// 后台任务表：`Bash(run_in_background)` 登记，`TaskOutput` / `TaskStop` 消费。
    /// 完成通知（新 turn）不在本骨架里，归 Task 5C。
    tasks: Arc<TaskRegistry>,
    /// 模型视角的消息历史（`run_turn` 维护；不含持久化格式）。
    messages: Vec<ModelMessage>,
    /// 事件信封的投递链路。默认 `desktop-continuous`；另一条链路的快照/队列归 Host/relay。
    delivery_kind: DeliveryKind,
    /// 会话内的 allow 规则（`allow_project` 的投影；D1 裁决后可从 sqlite 加载/写回）。
    ruleset: Ruleset,
    /// 审批交互的消费端；默认 `AutoAllow`（等价于驱动默认应答）。
    permission_handler: Box<dyn PermissionHandler>,
    /// 审批请求 id 的单调序号（`perm_<n>`）。
    permission_seq: u64,
    /// `allow_project` 的跨会话规则存储（D1 裁决 (a)：Rust 直连 sqlite）；`None` = 仅内存。
    rule_store: Option<Box<dyn RuleStore>>,
    /// 当前 cwd 对应的 `scope_id`（与规则库行匹配）。
    rule_scope_id: Option<String>,
    /// 最近一次落盘失败（不静默丢；宿主/测试可读）。
    rule_store_error: Option<String>,
    /// 会话级插件工具（M3 Task 3）：静态 `registry()` 不变，插件工具追加在这里。
    extra_tools: Vec<ToolSpec>,
    /// 插件工具的执行源；`None` = 没有插件宿主（插件工具不会出现在表里）。
    plugin_tools: Option<Arc<dyn PluginToolSource>>,
    /// 当前模式（进系统提示；宿主层经 `with_mode` 接线，缺省 `build`）。
    mode: String,
    /// 模型能力（进系统提示的限额行；缺省 = provider 未上报）。
    model_capabilities: ModelCapabilities,
    /// 当前请求的 assistant 消息 id（`scheduled.assistantMessageId` 与 `checkpoint.created`
    /// 的回指；与 `model.streaming` 事件里的同一 id 保持一致）。
    /// 直接调 [`Self::run_tool_batch`]（不经 turn 循环）时为 `None`，此时生成兜底 id。
    current_assistant_message_id: Option<String>,
    /// 本 turn 开始时消息历史里是否已有工具结果（`cacheStats` 口径用；见 `run_turn`）。
    prior_tool_history: bool,
}

impl Session {
    pub fn new(session_id: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            cwd: cwd.into(),
            turn_number: 0,
            sequence: 0,
            current_turn_id: None,
            file_state: FileState::new(),
            data_root: None,
            tasks: Arc::new(TaskRegistry::new()),
            messages: Vec::new(),
            delivery_kind: DeliveryKind::DesktopContinuous,
            ruleset: Ruleset::new(),
            permission_handler: Box::new(AutoAllow),
            permission_seq: 0,
            rule_store: None,
            rule_scope_id: None,
            rule_store_error: None,
            extra_tools: Vec::new(),
            plugin_tools: None,
            mode: "build".to_string(),
            model_capabilities: ModelCapabilities::default(),
            current_assistant_message_id: None,
            prior_tool_history: false,
        }
    }

    /// 接线当前模式（进系统提示；LT-1 C1 的运行时从 `RuntimeConfig` 传入）。
    pub fn with_mode(mut self, mode: impl Into<String>) -> Self {
        self.mode = mode.into();
        self
    }

    /// 就地更新模式（`session/setMode` 后由 worker 在下个 turn 前应用）。
    pub fn set_mode(&mut self, mode: impl Into<String>) {
        self.mode = mode.into();
    }

    /// 就地更新模型能力（`session/setModel` 后由 worker 在下个 turn 前应用）。
    pub fn set_model_capabilities(&mut self, capabilities: ModelCapabilities) {
        self.model_capabilities = capabilities;
    }

    /// 接线模型能力（系统提示的「Model limits」行；缺省值 = provider 未上报）。
    pub fn with_model_capabilities(mut self, capabilities: ModelCapabilities) -> Self {
        self.model_capabilities = capabilities;
        self
    }

    /// 接线插件工具源（M3 Task 3）：构造时拉一次 `listTools`，之后随会话使用。
    ///
    /// 插件工具的 `family`/`read_only` 目前是占位值（Task 0.4 未观察），权限引擎因此判 Allow；
    /// Task 4 观察后再收紧。
    pub fn with_plugin_tools(mut self, source: Arc<dyn PluginToolSource>) -> Result<Self, String> {
        let declarations = source.list_tools()?;
        self.extra_tools = declarations
            .into_iter()
            .map(PluginToolDeclaration::into_spec)
            .collect();
        self.plugin_tools = Some(source);
        Ok(self)
    }

    /// 当前会话可见的工具（静态注册表 + 插件工具）。
    pub fn tools(&self) -> Vec<ToolSpec> {
        let mut tools = agent_tools::registry();
        tools.extend(self.extra_tools.clone());
        tools
    }

    fn lookup_tool(&self, name: &str) -> Option<ToolSpec> {
        agent_tools::lookup(name).or_else(|| {
            self.extra_tools
                .iter()
                .find(|spec| spec.name == name)
                .cloned()
        })
    }

    fn is_plugin_tool(&self, name: &str) -> bool {
        self.extra_tools.iter().any(|spec| spec.name == name)
    }

    fn invoke_plugin_tool(
        &self,
        name: &str,
        input: &Value,
    ) -> Result<ToolOutcome, agent_tools::ToolError> {
        let Some(source) = &self.plugin_tools else {
            return Err(agent_tools::not_found(name));
        };
        match source.call_tool(name, input) {
            Ok(value) => {
                let content = value
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                // 观察：插件 MCP 结果 truncate，内置 node_repl 是 artifact。
                let budget = self
                    .extra_tools
                    .iter()
                    .find(|tool| tool.name == name)
                    .and_then(|tool| tool.plugin.as_ref())
                    .map(|meta| meta.budget_strategy.as_str())
                    .unwrap_or("truncate");
                if budget == "artifact" {
                    Ok(ToolOutcome::artifact(content))
                } else {
                    Ok(ToolOutcome::text(content))
                }
            }
            Err(error) => {
                let error_type: &'static str = if error.error_type == "SdkError" {
                    "SdkError"
                } else {
                    "tool_execution_failed"
                };
                let code: Option<&'static str> = match error.code.as_deref() {
                    Some("CONNECTION_CLOSED") => Some("CONNECTION_CLOSED"),
                    _ => Some("TOOL_EXECUTION_FAILED"),
                };
                Err(agent_tools::ToolError {
                    error_type,
                    message: error.message,
                    code,
                    detail: None,
                })
            }
        }
    }

    /// 指定事件信封的投递链路（默认 `desktop-continuous`）。
    pub fn with_delivery_kind(mut self, delivery_kind: DeliveryKind) -> Self {
        self.delivery_kind = delivery_kind;
        self
    }

    /// 替换审批交互的消费端（默认 `AutoAllow`）。
    pub fn with_permission_handler(mut self, handler: Box<dyn PermissionHandler>) -> Self {
        self.permission_handler = handler;
        self
    }

    /// 接线 `allow_project` 的跨会话规则（D1 裁决 (a)：Rust 直连 sqlite）。
    ///
    /// 构造时按 `cwd` 计算 `scope_id` 并**读取已有规则**；之后每次 `allow_project`
    /// 应答都会 upsert 同一行。打开/读取失败不阻断会话（规则退化为内存），
    /// 但写失败会记在 [`Self::rule_store_error`]，不静默吞掉。
    pub fn with_sqlite_rules(mut self, db_path: impl Into<PathBuf>) -> Self {
        let store = SqliteRuleStore::new(db_path);
        let scope_id = scope_id_for(&self.cwd);
        if let Some(ruleset) = store.load(&scope_id) {
            self.ruleset = ruleset;
        }
        self.rule_store = Some(Box::new(store));
        self.rule_scope_id = Some(scope_id);
        self
    }

    /// 最近一次规则落盘失败（`None` = 没有失败）。
    pub fn rule_store_error(&self) -> Option<&str> {
        self.rule_store_error.as_deref()
    }

    /// 模型视角的消息历史（只读；持久化格式归宿主）。
    pub fn messages(&self) -> &[ModelMessage] {
        &self.messages
    }

    /// 后台任务表（测试与后续 host 接线用）。
    pub fn tasks(&self) -> &Arc<TaskRegistry> {
        &self.tasks
    }

    /// 取走已排队的后台完成通知（非阻塞）。
    pub fn take_task_notifications(&self) -> Vec<TaskNotification> {
        self.tasks.take_notifications()
    }

    /// 在**空闲时**把排队中的通知逐条跑成新 turn。
    ///
    /// oracle 在忙时**直接丢弃**通知（spec §3.5）；因此调用方只能在空闲时调用本方法。
    /// `&mut self` 保证它不会与 [`Self::run_turn`] 并发；是否在忙时丢弃由调用方决定，
    /// 这里不偷偷替调用方排队。返回实际跑掉的通知条数。
    ///
    /// `cancel` 与 [`Self::run_turn`] 同源：调用方给同一个 token，通知 turn 也遵守取消。
    pub async fn run_pending_notifications(
        &mut self,
        provider: &dyn ModelProvider,
        sink: &mut dyn EventSink,
        cancel: &CancelToken,
    ) -> Result<usize, TurnError> {
        let notifications = self.tasks.take_notifications();
        let count = notifications.len();
        for notification in notifications {
            self.run_turn(&notification.to_xml(), provider, sink, cancel)
                .await?;
        }
        Ok(count)
    }

    /// 接线数据根：工具的大输出 artifact 落 `<dataRoot>/.mode/cli/exec/<sessionId>/…`
    /// （`data_root` 传已解析的 `.mode` 根，见 [`ArtifactLocation`]）。
    pub fn with_data_root(mut self, data_root: impl Into<PathBuf>) -> Self {
        self.data_root = Some(data_root.into());
        self
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 工具执行上下文（工作目录就是会话工作目录；不带 artifact 落点）。
    pub fn tool_context(&self) -> ToolContext {
        ToolContext {
            cwd: self.cwd.clone(),
            artifact: None,
            tasks: Some(Arc::clone(&self.tasks)),
        }
    }

    /// 本次调用专用的上下文：已接线数据根时带上 artifact 落点。
    ///
    /// 落点形状来自 oracle 观察：`<dataRoot>/.mode/cli/exec/<sessionId>/<toolCallId>-stdout.log`。
    pub fn tool_context_for_call(&self, tool_call_id: &str) -> ToolContext {
        ToolContext {
            cwd: self.cwd.clone(),
            artifact: self.data_root.as_ref().map(|data_root| ArtifactLocation {
                data_root: data_root.clone(),
                session_id: self.session_id.clone(),
                tool_call_id: tool_call_id.to_string(),
            }),
            tasks: Some(Arc::clone(&self.tasks)),
        }
    }

    /// 开一个 turn：产出 `turn.started`，并把它设为后续 [`Self::run_tool_batch`] 的归属 turn。
    ///
    /// 身份字段（C1.1-#3 对齐；形状取自观察）：
    /// - `foregroundExecutionId`：`runtime_command_<n>`（`session-lifecycle` 夹具观察值，
    ///   `<n>` 是会话内第几条被接受的输入，从 1 起）；
    /// - `messageId` / `queryId`：uuid 形状（夹具里这两个键被 §3 白名单折叠成 `<id>`，
    ///   原始形态不可见，按 uuid 生成，不编别的格式）；
    /// - `inputId`：**不产出**（行为夹具的 `turn.started` 里没有这个键；
    ///   `session-lifecycle-events.json` 同样没有）。
    pub fn begin_turn(&mut self, prompt: &str) -> SessionEvent {
        // 观察值（spec §3.5）：第一个 turn 的 `turnNumber` 是 **0**，之后逐次 +1。
        let turn_number = self.turn_number;
        self.turn_number += 1;
        let turn_id = format!("turn_{turn_number}");
        self.current_turn_id = Some(turn_id.clone());
        // `cacheStats.totalMessages` 的口径要在 turn 开始前采样（turn 内追加的工具结果
        // 不算「turn 前历史」；逐 turn 值见 `run_turn` 的注释）。
        self.prior_tool_history = self
            .messages
            .iter()
            .any(|message| matches!(message, ModelMessage::ToolResult { .. }));
        let payload = TurnStartedPayload {
            // 观察值：`executionStartedAt` 是小数毫秒。
            execution_started_at: Some(now_ms() as f64),
            turn_number,
            input: prompt.to_string(),
            input_id: None,
            query_id: Some(new_uuid()),
            message_id: Some(new_uuid()),
            foreground_execution_id: Some(format!("runtime_command_{}", turn_number + 1)),
            extra: ExtraFields::new(),
        };
        self.emit(Some(&turn_id), SessionEventType::TurnStarted, &payload)
    }

    /// 跑完一个完整 turn：模型流 →（可选）工具批次 → 终态。
    ///
    /// 事件顺序按 spec §3.5：`turn.started` → `model.streaming` ×N →（工具事件）→
    /// 下一轮模型流 → `turn.completed`。provider 的 delta **增量**消费：每到达一条就
    /// 立刻发 `model.streaming`，不等 provider future 返回；future 结束后再排空一次队列，
    /// 保证最后一批 delta 不丢。oracle 还会发若干 `session.updated` /
    /// `streamRecovery.updated`（需要消息计数与追踪 id），本步**不产出**，差异已登记。
    ///
    /// # 取消边界（LT-1 C1 控制器裁决 4）
    ///
    /// `cancel` 在两类边界生效：
    /// - **模型流中**：`select!` 的 cancel 分支在下一条 delta 边界停住——已到达的
    ///   delta 已发事件、已累积进本步文本，不再消费后续 delta，provider future 随
    ///   turn 一起放弃；
    /// - **步之间**：下一次模型请求开始前（含工具批次之后）检查，命中即不再发请求。
    ///
    /// 取消的终态是**唯一的** `turn.completed`（`resultType:"cancelled"`，协议注释把
    /// 用户中断归为正常结束），随后返回 [`TurnError::cancelled`] 让调用方知道 turn
    /// 没有正常完成但**不要再发**第二个终态。provider 错误仍返回普通
    /// [`TurnError`] 且不发终态——终态（`turn.failed`）由调用方负责。
    ///
    /// 只实现 `desktop-continuous` 的实时语义；重放归 Host/relay（DR-T5-1）。
    pub async fn run_turn(
        &mut self,
        prompt: &str,
        provider: &dyn ModelProvider,
        sink: &mut dyn EventSink,
        cancel: &CancelToken,
    ) -> Result<(), TurnError> {
        let started_at = std::time::Instant::now();
        sink.emit(self.begin_turn(prompt));
        self.messages.push(ModelMessage::User(prompt.to_string()));
        let turn_id = self
            .current_turn_id
            .clone()
            .expect("begin_turn 已设置 turn id");
        // 请求装配走统一的上下文模块（见循环体内）：系统提示（身份/工作区/工具/安全
        // 边界）在最前，随后是历史消息与本轮输入。系统提示不进 `self.messages`——它是
        // **每次请求**装配的，不是会话历史的一部分；工具结果要进下一轮请求，所以
        // 每次迭代都从当前历史重新装配。

        let mut usage = ModelUsage::default();
        let mut model_requests = 0_u64;
        let mut tool_call_count = 0_u64;
        let mut cancelled = false;
        // 取消时正在流的这一步已经到达的文本（终态如实携带，不丢已投递的 delta）。
        let mut cancelled_response = String::new();

        let response_text = 'drive: loop {
            // 取消边界：下一次模型请求开始前（首个请求、工具批次之后同样适用）。
            // 此时没有「正在流的本步文本」，终态 response 保持为空。
            if cancel.is_cancelled() {
                cancelled = true;
                break 'drive String::new();
            }
            model_requests += 1;
            // 本请求的 assistant 消息 id：`model.streaming` 与 `tool.updated(scheduled)`、
            // `checkpoint.created` 的回指共用同一个（夹具里被 §3 折叠成 `<id>`，但同一条
            // assistant 消息内取值一致是事件序的事实）。
            self.current_assistant_message_id =
                Some(format!("msg_{}_{model_requests}", self.session_id));
            let context_input = crate::context::ContextInput {
                cwd: &self.cwd,
                mode: &self.mode,
                tools: &self.tools(),
                messages: &self.messages,
                model: &self.model_capabilities,
            };
            let request = crate::context::build_model_request(&context_input);

            // 一次模型请求一条 delta 通道：provider 产出一条就 send，会话侧随到随发。
            let (delta_sink, mut deltas) = tokio::sync::mpsc::unbounded_channel();
            let mut provider_future = provider.stream(request, delta_sink);
            let mut provider_result: Option<Result<ModelResponse, ProviderError>> = None;
            let mut delta_channel_open = true;
            let mut text = String::new();
            let mut tool_calls: Vec<ToolCallRequest> = Vec::new();

            while provider_result.is_none() {
                tokio::select! {
                    // `biased` + delta 优先：已到达的 delta 先发，future 就绪后队列里的尾巴
                    // 仍按顺序走完，不会被「future 先完成」抢掉；取消也只在 delta 边界
                    // （没有立即可消费的 delta 时）生效，不会打断半条 delta。
                    biased;
                    maybe_delta = deltas.recv(), if delta_channel_open => {
                        match maybe_delta {
                            Some(delta) => self.absorb_model_delta(
                                &turn_id,
                                model_requests,
                                delta,
                                &mut text,
                                &mut tool_calls,
                                sink,
                            ),
                            // provider 提前关掉发送端但 future 还没结束：继续等 future。
                            None => delta_channel_open = false,
                        }
                    }
                    result = &mut provider_future => provider_result = Some(result),
                    _ = cancel.cancelled() => {
                        cancelled = true;
                        break;
                    }
                }
            }
            if cancelled {
                cancelled_response = text;
                break 'drive String::new();
            }

            // future 结束前已经进入队列、但还没被 select 取走的 delta（tail）。
            while let Ok(delta) = deltas.try_recv() {
                self.absorb_model_delta(
                    &turn_id,
                    model_requests,
                    delta,
                    &mut text,
                    &mut tool_calls,
                    sink,
                );
            }

            let response = provider_result
                .expect("循环只在 provider future 结束后退出")
                .map_err(|error| {
                    // 失败时刻的已知累计（Task 7 裁决 6）：本请求之前的成功请求用量、
                    // 请求次数与工具调用数随错误带回，供 `turn_usage` 记录已知值。
                    TurnError::with_progress(
                        error.message,
                        crate::stream::TurnProgress {
                            usage: usage.clone(),
                            model_requests,
                            tool_calls: tool_call_count,
                        },
                    )
                })?;
            usage.input_tokens += response.usage.input_tokens;
            usage.output_tokens += response.usage.output_tokens;
            usage.cache_read_tokens += response.usage.cache_read_tokens;
            usage.cache_write_tokens += response.usage.cache_write_tokens;
            usage.reasoning_tokens += response.usage.reasoning_tokens;

            if tool_calls.is_empty() {
                self.messages.push(ModelMessage::Assistant {
                    text: Some(text.clone()),
                    tool_calls: Vec::new(),
                });
                break 'drive text;
            }

            self.messages.push(ModelMessage::Assistant {
                text: if text.is_empty() { None } else { Some(text) },
                tool_calls: tool_calls
                    .iter()
                    .map(|call| crate::provider::ModelToolCall {
                        tool_call_id: call.tool_call_id.clone(),
                        tool_name: call.name.clone(),
                        input: call.input.clone(),
                    })
                    .collect(),
            });
            tool_call_count += tool_calls.len() as u64;
            let tool_events = self.run_tool_batch(&tool_calls);
            self.record_tool_results(&tool_events);
            for event in tool_events {
                sink.emit(event);
            }
            // 工具批次跑完后回到循环顶：若已取消，不再发起新的模型请求。
        };

        if cancelled {
            let total_tokens = usage.input_tokens + usage.output_tokens;
            let payload = TurnCompletedPayload {
                response: cancelled_response,
                token_count: total_tokens,
                usage: Some(serde_json::json!({
                    "source": "provider",
                    "modelRequestCount": model_requests,
                    "inputTokens": usage.input_tokens,
                    "outputTokens": usage.output_tokens,
                    "totalTokens": total_tokens,
                    "cacheReadTokens": usage.cache_read_tokens,
                    "cacheWriteTokens": usage.cache_write_tokens,
                    "reasoningTokens": usage.reasoning_tokens,
                    "webFetchRequests": 0,
                    "webSearchRequests": 0,
                })),
                tool_call_count,
                history_round_count: Some(model_requests),
                duration: started_at.elapsed().as_millis() as f64,
                cache_stats: None,
                input_id: None,
                result_type: TurnResultType::Cancelled,
                background_subagent_result_consumed: None,
                extra: ExtraFields::new(),
            };
            sink.emit(self.emit(Some(&turn_id), SessionEventType::TurnCompleted, &payload));
            return Err(TurnError::cancelled());
        }

        let total_tokens = usage.input_tokens + usage.output_tokens;
        let payload = TurnCompletedPayload {
            response: response_text,
            token_count: total_tokens,
            usage: Some(serde_json::json!({
                "source": "provider",
                "modelRequestCount": model_requests,
                "inputTokens": usage.input_tokens,
                "outputTokens": usage.output_tokens,
                "totalTokens": total_tokens,
                "cacheReadTokens": usage.cache_read_tokens,
                "cacheWriteTokens": usage.cache_write_tokens,
                "reasoningTokens": usage.reasoning_tokens,
                "webFetchRequests": 0,
                "webSearchRequests": 0,
            })),
            tool_call_count,
            history_round_count: Some(model_requests),
            duration: started_at.elapsed().as_millis() as f64,
            // `cacheStats`（C1.1-#4 对齐）。**口径是拟合规则**，不是语义分解：全部 41 个
            // 观察值（38 条行为夹具 + session-lifecycle 两条 turn）都满足
            // `totalMessages = 本 turn 终末请求时消息数 + 6 + (turn 前已有工具结果 ? 1 : 0)`、
            // `cachedMessages = 5`、`lastCacheHit = false`。oracle 把请求装配的哪些前缀
            // 条目计入 totalMessages/cachedMessages 无法从事件面观察（逐字值见
            // `session-lifecycle-events.json`：文本 turn 7、工具 turn 11、跨 turn 累计 30），
            // 这里只实现复现全部观察值的规则，并把口径登记在 spec §C1。
            // mock provider 不返回缓存读（`cacheReadTokens: 0`），`lastCacheHit` 按观察恒 false。
            // 实现写法：`self.messages` 此刻**已包含**本 turn 的收尾 assistant（它不是
            // 「终末请求时消息数」的一部分），所以先减 1 再加 6。
            cache_stats: Some(agent_protocol::session_event::CacheStats {
                // 消息列表此刻已含本 turn 的收尾 assistant（终末请求时尚未 push），所以减 1。
                total_messages: (self.messages.len().saturating_sub(1)
                    + 6
                    + if self.prior_tool_history { 1 } else { 0 })
                    as u64,
                cached_messages: 5,
                last_cache_hit: false,
                cache_read_tokens: None,
                extra: ExtraFields::new(),
            }),
            input_id: None,
            result_type: TurnResultType::Success,
            background_subagent_result_consumed: None,
            extra: ExtraFields::new(),
        };
        sink.emit(self.emit(Some(&turn_id), SessionEventType::TurnCompleted, &payload));
        Ok(())
    }

    /// 消费一条模型 delta：先累积本轮文本/工具调用，再按到达顺序发 `model.streaming`。
    ///
    /// 这是 delta 的唯一消费点：流式阶段与 future 结束后的 tail 排空共用它，
    /// 保证事件顺序与统计口径一致。
    fn absorb_model_delta(
        &mut self,
        turn_id: &str,
        request_index: u64,
        delta: ModelDelta,
        text: &mut String,
        tool_calls: &mut Vec<ToolCallRequest>,
        sink: &mut dyn EventSink,
    ) {
        match &delta {
            ModelDelta::Text { text: chunk } => text.push_str(chunk),
            ModelDelta::ToolCall {
                tool_call_id,
                tool_name,
                input,
            } => tool_calls.push(ToolCallRequest {
                tool_call_id: tool_call_id.clone(),
                name: tool_name.clone(),
                input: input.clone(),
            }),
            _ => {}
        }
        let event = self.model_streaming_event(turn_id, request_index, &delta);
        sink.emit(event);
    }

    /// 把模型 delta 投影成 `model.streaming` 事件（顺序即 delta 顺序）。
    fn model_streaming_event(
        &mut self,
        turn_id: &str,
        request_index: u64,
        delta: &ModelDelta,
    ) -> SessionEvent {
        let assistant_message_id = Some(format!("msg_{}_{request_index}", self.session_id));
        let payload = match delta {
            ModelDelta::Text { text } => ModelStreamingPayload {
                assistant_message_id,
                delta: Some(text.clone()),
                done: Some(false),
                input: None,
                kind: ModelStreamingKind::TextDelta,
                part_id: None,
                provider_executed: None,
                tool_call_id: None,
                tool_name: None,
                extra: ExtraFields::new(),
            },
            ModelDelta::ToolInputStart {
                tool_call_id,
                tool_name,
            } => ModelStreamingPayload {
                assistant_message_id,
                delta: Some(String::new()),
                done: Some(false),
                input: None,
                kind: ModelStreamingKind::ToolInputStart,
                part_id: None,
                provider_executed: None,
                tool_call_id: Some(tool_call_id.clone()),
                tool_name: Some(tool_name.clone()),
                extra: ExtraFields::new(),
            },
            ModelDelta::ToolInputDelta {
                tool_call_id,
                delta,
            } => ModelStreamingPayload {
                assistant_message_id,
                delta: Some(delta.clone()),
                done: Some(false),
                input: None,
                kind: ModelStreamingKind::ToolInputDelta,
                part_id: None,
                provider_executed: None,
                tool_call_id: Some(tool_call_id.clone()),
                tool_name: None,
                extra: ExtraFields::new(),
            },
            ModelDelta::ToolInputEnd { tool_call_id } => ModelStreamingPayload {
                assistant_message_id,
                delta: Some(String::new()),
                done: Some(false),
                input: None,
                kind: ModelStreamingKind::ToolInputEnd,
                part_id: None,
                provider_executed: None,
                tool_call_id: Some(tool_call_id.clone()),
                tool_name: None,
                extra: ExtraFields::new(),
            },
            ModelDelta::ToolCall {
                tool_call_id,
                tool_name,
                input,
            } => ModelStreamingPayload {
                assistant_message_id,
                delta: Some(String::new()),
                done: Some(false),
                input: Some(input.clone()),
                kind: ModelStreamingKind::ToolCall,
                part_id: None,
                provider_executed: None,
                tool_call_id: Some(tool_call_id.clone()),
                tool_name: Some(tool_name.clone()),
                extra: ExtraFields::new(),
            },
        };
        self.emit(Some(turn_id), SessionEventType::ModelStreaming, &payload)
    }

    /// 把工具结果写回模型历史（下一轮请求要用）。
    fn record_tool_results(&mut self, events: &[SessionEvent]) {
        for event in events {
            if event.event_type != SessionEventType::ToolUpdated {
                continue;
            }
            let Some(payload) = event
                .payload_as::<ToolUpdatedPayload>()
                .expect("工具载荷能解码")
            else {
                continue;
            };
            match payload {
                ToolUpdatedPayload::Result(result) => {
                    let content = result
                        .result
                        .content
                        .as_ref()
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    self.messages.push(ModelMessage::ToolResult {
                        tool_call_id: result.call.tool_call_id,
                        content,
                    });
                }
                ToolUpdatedPayload::Error(error) => {
                    self.messages.push(ModelMessage::ToolResult {
                        tool_call_id: error.call.tool_call_id,
                        content: error.error.message,
                    });
                }
                _ => {}
            }
        }
    }

    /// 执行一条 assistant 消息里的若干工具调用，按观察到的顺序发事件。
    ///
    /// 返回的事件序列：每个调用 `scheduled`（+ 解析到工具时 `started`）→ `result`|`error`，
    /// 最后是整批的 `batch`；本批有**成功的写类工具**（`Write` / `Edit`）时，`batch` 之后
    /// 还有一条 `checkpoint.created`（C1.1-#2 对齐；夹具 `write-new` / `edit-unique` 等：
    /// batch 后、下一轮模型请求前恰好一条）。
    ///
    /// 每个调用的判定顺序（C1.2-#1 对齐）：**schema 级入参校验在权限评估之前**——
    /// 非法入参 `scheduled → error`，不产生审批对、也没有 `started`。
    ///
    /// 必须先 [`Self::begin_turn`]：没有归属 turn 的事件在线上不该出现，这里直接失败（不猜）。
    pub fn run_tool_batch(&mut self, calls: &[ToolCallRequest]) -> Vec<SessionEvent> {
        let turn_id = self
            .current_turn_id
            .clone()
            .expect("run_tool_batch 之前必须先 begin_turn（否则事件没有归属 turn）");
        let mut events = Vec::with_capacity(calls.len() * 3 + 1);
        let mut success_count = 0_u64;
        let mut error_count = 0_u64;
        // 本批成功的写类调用数（checkpoint 的 `fileCount` 口径；全部夹具都是单调用批）。
        let mut checkpoint_files = 0_u64;

        // 一条 assistant 消息里的调用会被并行执行（spec §3 无序组②），所以它们是**一个**并行组；
        // 单调用批次的观察值就是 `parallelGroups: [[id]]`（夹具 `read-single`）。
        let group: Vec<String> = calls.iter().map(|call| call.tool_call_id.clone()).collect();

        for call in calls {
            // 每次调用各自的上下文：artifact 文件名要用本次 `toolCallId`（观察值）。
            let context = self.tool_context_for_call(&call.tool_call_id);
            let spec = self.lookup_tool(&call.name);
            events.push(self.scheduled_event(&turn_id, call, &group));

            // 审批路径事实：本次调用是否走过了 Ask（`started` 的 Bash 档位与
            // `perf.permissionWaitMs` 的有无都取决于它）；初始为「没走」。
            let mut asked = false;
            let mut permission_wait_ms: Option<u64> = None;
            // 未注册的工具**没有** `started`（观察：`Glob`/`Grep` 的 `scheduled` 之后直接是
            // `error`）——工具在解析阶段就出局了，没有「开始执行」这一步。
            if let Some(spec) = &spec {
                // **schema 级入参校验在权限之前**（C1.2-#1；夹具 `bash-empty-args` /
                // `edit-missing-new-string` / `write-empty-args` / `write-missing-content`：
                // 非法入参不产生审批对、也没有 started，直接 error）。
                if let Some(error) = agent_tools::input_validation_error(&call.name, &call.input) {
                    error_count += 1;
                    events.push(self.error_event(&turn_id, call, &error));
                    continue;
                }
                // `TaskOutput` 的查找是启动前检查（观察：找不到任务时 scheduled → error，
                // **没有** started）。
                if let Some(error) = agent_tools::preflight_error(&call.name, &call.input, &context)
                {
                    error_count += 1;
                    events.push(self.error_event(&turn_id, call, &error));
                    continue;
                }
                match crate::permission::evaluate(spec, &call.input, &self.ruleset) {
                    crate::permission::Decision::Allow => {}
                    crate::permission::Decision::Ask { risk_level, reason } => {
                        asked = true;
                        let request =
                            self.permission_request(&turn_id, call, spec, risk_level, reason);
                        events.push(self.emit(
                            Some(&turn_id),
                            SessionEventType::PermissionRequested,
                            &permission_event_payload(&request),
                        ));
                        let wait_started = std::time::Instant::now();
                        let response = self.permission_handler.request(&request);
                        // `perf.permissionWaitMs`：审批等待时长（观察：有审批对的调用才有
                        // 这个键；时间白名单盖着数值本身）。
                        permission_wait_ms = Some(wait_started.elapsed().as_millis() as u64);
                        let decision = response.decision;
                        events.push(self.emit(
                            Some(&turn_id),
                            SessionEventType::PermissionResolved,
                            &serde_json::json!({
                                "requestId": request.request_id,
                                "toolCallId": request.tool_call_id,
                                "decision": match decision {
                                    PermissionDecision::Allow => "allow",
                                    PermissionDecision::Deny => "deny",
                                },
                            }),
                        ));
                        if decision == PermissionDecision::Deny {
                            // 观察：deny 只留下 scheduled + batch（errorCount 1），
                            // 没有 started / result / error 帧。
                            error_count += 1;
                            continue;
                        }
                        if let Some(updates) = &response.permission_updates {
                            self.ruleset.apply_updates(updates);
                            if let (Some(store), Some(scope)) =
                                (&self.rule_store, &self.rule_scope_id)
                            {
                                match store.append(scope, updates) {
                                    Ok(()) => self.rule_store_error = None,
                                    Err(error) => self.rule_store_error = Some(error),
                                }
                            }
                        }
                    }
                }
                events.push(self.started_event(&turn_id, call, spec, asked));
            }

            // `Read` 的重复读去重：**会话状态**在这一层（工具本身是纯函数），见 FileState 的注释。
            // 命中就不调用工具，直接发一条与 oracle 同形的成功结果；
            // 只有**成功**的读才记进历史（实测：读不存在的文件两次都是错误，错误不去重）。
            let started = std::time::Instant::now();
            let stubbed = if call.name == "Read" {
                self.file_state.stub_for(&call.input, &context)
            } else {
                None
            };
            let outcome = match stubbed {
                Some(stub) => Ok(stub),
                None => {
                    let result = if self.is_plugin_tool(&call.name) {
                        self.invoke_plugin_tool(&call.name, &call.input)
                    } else {
                        agent_tools::invoke_by_name(
                            &call.name,
                            &call.input,
                            &context,
                            &self.file_state,
                        )
                    };
                    match (&call.name[..], &result) {
                        // 读成功 → 记进去重表，同时进入「本会话碰过它」集合。
                        ("Read", Ok(_)) => self.file_state.record(&call.input, &context),
                        // 写/编辑成功 → 该文件也进入「已读」状态（观察：成功文案自述
                        // 「file state is current in your context — no need to Read it back」）。
                        ("Write" | "Edit", Ok(_)) => {
                            if let Some(file_path) =
                                call.input.get("file_path").and_then(Value::as_str)
                            {
                                self.file_state.record_written(file_path, &context);
                            }
                        }
                        _ => {}
                    }
                    result
                }
            };
            let elapsed_ms = started.elapsed().as_millis() as u64;
            match outcome {
                Ok(result) => {
                    success_count += 1;
                    if matches!(call.name.as_str(), "Write" | "Edit") {
                        checkpoint_files += 1;
                    }
                    events.push(self.result_event(
                        &turn_id,
                        call,
                        &result,
                        elapsed_ms,
                        permission_wait_ms,
                    ));
                }
                Err(error) => {
                    error_count += 1;
                    events.push(self.error_event(&turn_id, call, &error));
                }
            }
        }

        let batch = ToolBatchPayload {
            tool_call_ids: calls.iter().map(|call| call.tool_call_id.clone()).collect(),
            success_count,
            error_count,
            extra: ExtraFields::new(),
        };
        events.push(self.emit_tool(&turn_id, ToolUpdatedPayload::Batch(batch)));

        // `checkpoint.created`（C1.1-#2）：写类工具成功路径在 batch 后、下一轮模型请求前
        // 发出（夹具 `write-new` / `write-overwrite-read` / `edit-unique` /
        // `edit-empty-old-creates` 等；失败路径与只读工具没有这条——同批夹具反证）。
        // `agent-core` 只发事件，不接 git checkpoint（那是 Host 面的
        // `git-checkpoint-service`）；回指 id 与 `mode-artifact://` 引用按观察形状生成。
        if checkpoint_files > 0 {
            events.push(self.checkpoint_event(&turn_id, checkpoint_files));
        }
        events
    }

    /// 构造一条审批请求（三条选项的文案与响应逐字来自 spec §3.3）。
    fn permission_request(
        &mut self,
        turn_id: &str,
        call: &ToolCallRequest,
        spec: &ToolSpec,
        risk_level: &str,
        reason: &str,
    ) -> PermissionRequest {
        self.permission_seq += 1;
        let request_id = format!("perm_{}", self.permission_seq);
        let rule_content = match spec.family {
            ModeToolFamily::FileWrite => call.input.get("file_path").and_then(Value::as_str),
            ModeToolFamily::Shell => call.input.get("command").and_then(Value::as_str),
            _ => None,
        }
        .unwrap_or_default()
        .to_string();
        let update = PermissionUpdate {
            update_type: PermissionUpdateType::AddRules,
            behavior: RuleBehavior::Allow,
            rules: vec![PermissionRuleValue {
                tool_name: spec.name.clone(),
                rule_content,
                extra: ExtraFields::new(),
            }],
            extra: ExtraFields::new(),
        };
        let make_response =
            |decision: PermissionDecision,
             response_reason: &str,
             permission_updates: Option<Vec<PermissionUpdate>>| {
                PermissionResponse {
                    decision,
                    reason: Some(response_reason.to_string()),
                    modified_input: None,
                    permission_updates,
                    extra: ExtraFields::new(),
                }
            };
        let options = vec![
            PermissionOption {
                option_id: "allow_once".to_string(),
                kind: PermissionOptionKind::AllowOnce,
                name: "Allow once".to_string(),
                description: None,
                response: make_response(PermissionDecision::Allow, "Approved once", None),
                extra: ExtraFields::new(),
            },
            PermissionOption {
                option_id: "allow_project".to_string(),
                kind: PermissionOptionKind::AllowAlways,
                name: "Always allow in this project".to_string(),
                description: Some(
                    "Do not ask again for matching requests in this project".to_string(),
                ),
                response: make_response(
                    PermissionDecision::Allow,
                    "Approved for this project",
                    Some(vec![update.clone()]),
                ),
                extra: ExtraFields::new(),
            },
            PermissionOption {
                option_id: "deny".to_string(),
                kind: PermissionOptionKind::Deny,
                name: "Deny".to_string(),
                description: None,
                response: make_response(PermissionDecision::Deny, DENY_REASON, None),
                extra: ExtraFields::new(),
            },
        ];
        let mut extra = ExtraFields::new();
        extra.insert(
            "suggestedPermissionUpdates".to_string(),
            serde_json::json!([update]),
        );
        PermissionRequest {
            // 观察（夹具 `edit-unique` 等的 `permission.requested.input`）：审批入参带
            // schema 默认值（`replace_all:false`），而 `scheduled.inputByteLength` 用的是
            // 原始三键入参——默认值只补进审批请求。
            input: agent_tools::input_with_schema_defaults(&spec.name, &call.input),
            reason: reason.to_string(),
            request_id,
            risk_level: risk_level.to_string(),
            session_id: self.session_id.clone(),
            options,
            tool_call_id: call.tool_call_id.clone(),
            tool_name: spec.name.clone(),
            turn_id: turn_id.to_string(),
            extra,
        }
    }

    fn scheduled_event(
        &mut self,
        turn_id: &str,
        call: &ToolCallRequest,
        group: &[String],
    ) -> SessionEvent {
        let payload = ToolScheduledPayload {
            call: ToolCallIdentity {
                tool_call_id: call.tool_call_id.clone(),
                parent_tool_call_id: None,
                source: None,
                agent_id: None,
                agent_type: None,
                background: None,
                child_session_id: None,
                child_tool_call_id: None,
                description: None,
            },
            tool_name: call.name.clone(),
            // 观察值（C1.1-#5）：`assistantMessageId` 与 `model.streaming` 里同一条
            // assistant 消息的 id 相同；turn 循环外的直接调用（测试）给兜底 id。
            assistant_message_id: Some(
                self.current_assistant_message_id
                    .clone()
                    .unwrap_or_else(|| format!("msg_{}", new_uuid())),
            ),
            input: None,
            // 观察值：入参 JSON 文本的字节数（`{"pattern":"*.txt"}` → 19）。
            input_byte_length: Some(serialized_len(&call.input)),
            // 观察值：调度事件故意不带入参，入参只在 model.streaming 里（spec §1.6）。
            input_omitted: Some(true),
            input_ref: Some("model_stream".to_string()),
            dependencies: Some(Vec::new()),
            parallel_group_index: Some(0),
            // 观察值（C1.1-#5）：`canRunParallel` **按工具**——Read 与未注册工具（Glob/Grep）
            // 是 `true`，Bash / Write / Edit 是 `false`。取值来源是 `ToolSpec.read_only`
            // 这个既有事实（C1 不新增猜测档位）；未注册工具无声明，按观察值 `true`。
            can_run_parallel: Some(
                self.lookup_tool(&call.name)
                    .map(|spec| spec.read_only)
                    .unwrap_or(true),
            ),
            schedule: Some(ToolSchedule {
                parallel_groups: Some(vec![group.to_vec()]),
                execution_order: Some(group.to_vec()),
                extra: ExtraFields::new(),
            }),
            extra: ExtraFields::new(),
        };
        self.emit_tool(turn_id, ToolUpdatedPayload::Scheduled(payload))
    }

    /// `started` 载荷。`asked` 表示本次调用走过了审批（`Ask` 且答复 allow）。
    ///
    /// 档位观察（C1.1-#9，逐夹具）：
    /// - `Read` / 安全直通的 `Bash`：`readOnly:true` / `sideEffectScope:"none"`；
    ///   ——注意 `Bash` 的 `ToolSpec.read_only` 是 `false`（`canRunParallel` 口径），
    ///   它的 `started` 档位按**审批路径**给：安全 allowlist 直通 → 只读档。
    /// - 审批后执行的 `Bash`（`bash-exit-7` / `bash-command-not-found` / `bash-output-then-exit-7`）：
    ///   `readOnly:false` / `sideEffectScope:"system"`；
    /// - `Write` / `Edit`：`readOnly:false` / `sideEffectScope:"workspace"`（`write-new` 等）；
    /// - `TaskStop`：`readOnly:false` / `sideEffectScope:"session"`（spec §3.4）；
    /// - 插件工具按声明携带的 scope（MCP `network` / node_repl `system`）。
    fn started_event(
        &mut self,
        turn_id: &str,
        call: &ToolCallRequest,
        spec: &ToolSpec,
        asked: bool,
    ) -> SessionEvent {
        let (read_only, side_effect_scope) = if let Some(meta) = &spec.plugin {
            // 观察：插件 MCP 是 `network`，内置 node_repl 是 `system`。
            (spec.read_only, Some(meta.side_effect_scope.clone()))
        } else if spec.family == ModeToolFamily::Shell {
            if asked {
                (false, Some("system".to_string()))
            } else {
                (true, Some("none".to_string()))
            }
        } else {
            match (spec.read_only, spec.family) {
                (true, _) => (true, Some("none".to_string())),
                (false, ModeToolFamily::FileWrite) => (false, Some("workspace".to_string())),
                (false, ModeToolFamily::TaskControl) => (false, Some("session".to_string())),
                // 其余非只读档的取值集合**仍未观察**，留空而非编一个档位。
                (false, _) => (false, None),
            }
        };
        let payload = ToolStartedPayload {
            call: ToolCallIdentity {
                tool_call_id: call.tool_call_id.clone(),
                parent_tool_call_id: None,
                source: None,
                agent_id: None,
                agent_type: None,
                background: None,
                child_session_id: None,
                child_tool_call_id: None,
                description: None,
            },
            started_at: ProtocolInstant::Ms(now_ms() as f64),
            read_only: Some(read_only),
            side_effect_scope,
            tool_name: Some(call.name.clone()),
            extra: ExtraFields::new(),
        };
        self.emit_tool(turn_id, ToolUpdatedPayload::Started(payload))
    }

    fn result_event(
        &mut self,
        turn_id: &str,
        call: &ToolCallRequest,
        result: &ToolOutcome,
        elapsed_ms: u64,
        permission_wait_ms: Option<u64>,
    ) -> SessionEvent {
        let payload = ToolResultPayload {
            call: ToolCallIdentity {
                tool_call_id: call.tool_call_id.clone(),
                parent_tool_call_id: None,
                source: None,
                agent_id: None,
                agent_type: None,
                background: None,
                child_session_id: None,
                child_tool_call_id: None,
                description: None,
            },
            result: result_object(result, elapsed_ms, permission_wait_ms),
            // 观察值：成功结果的载荷里**没有** `toolName` 键（协议夹具
            // `session-event-tool-updated-result.json` 与每条行为夹具的 result 载荷都如此）。
            // 同一个帧里 `"delta": ""` / `"previousTitle": ""` 这类空串是**保留**的，所以
            // 「键不存在」不是空串被剥掉的结果——`None` 才是忠实的投影（复评 Minor 5）。
            tool_name: None,
            duration: elapsed_ms as f64,
            extra: ExtraFields::new(),
        };
        self.emit_tool(turn_id, ToolUpdatedPayload::Result(payload))
    }

    fn error_event(
        &mut self,
        turn_id: &str,
        call: &ToolCallRequest,
        error: &agent_tools::ToolError,
    ) -> SessionEvent {
        let payload = ToolErrorPayload {
            call: ToolCallIdentity {
                tool_call_id: call.tool_call_id.clone(),
                parent_tool_call_id: None,
                source: None,
                agent_id: None,
                agent_type: None,
                background: None,
                child_session_id: None,
                child_tool_call_id: None,
                description: None,
            },
            error: error_object(error),
            // 观察值：错误载荷**没有** `toolName`（夹具 `read-absent-search-tools` 的 error 载荷）。
            tool_name: None,
            extra: ExtraFields::new(),
        };
        self.emit_tool(turn_id, ToolUpdatedPayload::Error(payload))
    }

    /// `checkpoint.created`（C1.1-#2 对齐；形状逐字来自夹具录制）。
    ///
    /// 载荷：`{checkpointId, messageId, targetMessageId, toolMessageId, scope:"workspace",
    /// snapshotRef, diffRef, fileCount}`。所有 id 键都在 spec §3 白名单里折叠成 `<id>`；
    /// `snapshotRef` / `diffRef` 的 `mode-artifact://sess_<id>/tool-result-<id>` 形状里
    /// 的 uuid 由「id（值）」规则折叠。本实现只发事件，不实现 git 快照存储
    /// （`git-checkpoint-service` 是 Host 面；事件里的引用是形状占位）。
    fn checkpoint_event(&mut self, turn_id: &str, file_count: u64) -> SessionEvent {
        let reference = format!(
            "mode-artifact://sess_{}/tool-result-{}",
            new_uuid(),
            new_uuid()
        );
        let payload = serde_json::json!({
            "checkpointId": new_uuid(),
            "messageId": self
                .current_assistant_message_id
                .clone()
                .unwrap_or_else(new_uuid),
            "targetMessageId": new_uuid(),
            "toolMessageId": new_uuid(),
            "scope": "workspace",
            "snapshotRef": reference,
            "diffRef": reference,
            "fileCount": file_count,
        });
        self.emit(Some(turn_id), SessionEventType::CheckpointCreated, &payload)
    }

    /// 发一条 `tool.updated`：载荷是**带 `kind` 判别键的枚举**，所以包信封的是枚举而不是内部结构体
    /// （只序列化内部结构体会丢掉 `kind`，实测解回来报 `missing field «kind»`）。
    fn emit_tool(&mut self, turn_id: &str, payload: ToolUpdatedPayload) -> SessionEvent {
        self.emit(Some(turn_id), SessionEventType::ToolUpdated, &payload)
    }

    /// 包一个会话事件信封。
    ///
    /// `eventId` 用单调 id 而不是 uuid：spec §3 把 `eventId` 归一成 `<id>`，它的真实形态不是
    /// 被对拍的行为，单调 id 反而让用例可以断言。`traceId` 留空（追踪 id 由模型/会话层给）。
    /// `turn_id` 为 `None` 时信封省略 `turnId`（turn 尚未开始就失败的场景）。
    fn emit<T: Serialize>(
        &mut self,
        turn_id: Option<&str>,
        event_type: SessionEventType,
        payload: &T,
    ) -> SessionEvent {
        self.sequence += 1;
        SessionEvent {
            event_id: format!("evt_{}", self.sequence),
            session_id: self.session_id.clone(),
            turn_id: turn_id.map(str::to_string),
            seq: self.sequence,
            trace_id: None,
            timestamp: now_ms(),
            // 默认 `desktop-continuous`（协议夹具的信封）；另一条链路只透传标记，
            // 快照/队列归 Host/relay（DR-T5-1）。
            delivery_kind: Some(self.delivery_kind),
            event_type,
            payload: Some(serde_json::to_value(payload).expect("镜像类型可序列化")),
        }
    }

    /// 发一条 `turn.failed` 终态（LT-1 C1 控制器裁决 3：终态恰好一次）。
    ///
    /// 用于 provider 错误等 `run_turn` 自身没有发终态的失败路径：seq 归 [`Session`]
    /// 统一分配，信封的 `turnId` 在 turn 尚未开始（如 turn 前持久化失败）时省略。
    ///
    /// `error_type` 由调用方给：provider/模型请求失败用 `"runtime_error"`；缺模型等
    /// 配置失败用 `"CONFIGURATION_ERROR"`（oracle 观察：缺 `model` → `turn.failed`
    /// 携带 `CONFIGURATION_ERROR` / "Select a model before continuing"，见
    /// `docs/specs/tauri-agent-runtime.md` §1.5）。其余未观察字段（`error.code` 等）
    /// 仍然不编造。
    pub fn emit_turn_failed(
        &mut self,
        turn_id: Option<&str>,
        error_type: &str,
        error_message: &str,
        turn_phase: &str,
    ) -> SessionEvent {
        let payload = TurnFailedPayload {
            error: ProtocolErrorDetail {
                error_type: error_type.to_string(),
                message: error_message.to_string(),
                stack: None,
                code: None,
                detail: None,
                underlying_error_message: None,
                underlying_error_detail: None,
                attribution: None,
                retryable: None,
                data: None,
                extra: ExtraFields::new(),
            },
            turn_phase: turn_phase.to_string(),
            input_id: None,
            background_subagent_result_consumed: None,
            extra: ExtraFields::new(),
        };
        self.emit(turn_id, SessionEventType::TurnFailed, &payload)
    }
}

/// `permission.requested` 事件的载荷投影（C1.1-#8 对齐）。
///
/// 线上事件载荷与反向请求 `params` **不同形**：事件载荷里没有 `sessionId` / `turnId`
/// （夹具 `permission.requested` 逐字如此），但多一个 `fullAccessSupported: true`
/// （观察值恒 true）。反向请求帧仍发完整 [`PermissionRequest`]（spec §3.3 的帧形状）。
fn permission_event_payload(request: &PermissionRequest) -> Value {
    let mut payload = serde_json::to_value(request).expect("审批请求可序列化");
    if let Some(map) = payload.as_object_mut() {
        map.remove("sessionId");
        map.remove("turnId");
        map.insert("fullAccessSupported".to_string(), Value::Bool(true));
    }
    // 观察（夹具 `write-empty-path`）：`ruleContent` 为空时**整个键都不出现**
    // （空路径的规则是 `{toolName}`）。`PermissionRuleValue.rule_content` 在协议里是
    // 必填 String，所以只在事件载荷投影里剔掉空值——反向请求帧的形状不受影响。
    strip_empty_rule_content(&mut payload);
    payload
}

/// 递归剔除 `rules[]` 条目里的空 `ruleContent`（只动 `rules` 数组内的对象）。
fn strip_empty_rule_content(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                if key == "rules" {
                    if let Value::Array(rules) = inner {
                        for rule in rules {
                            if let Value::Object(rule) = rule {
                                if rule.get("ruleContent") == Some(&Value::String(String::new())) {
                                    rule.remove("ruleContent");
                                }
                            }
                        }
                    }
                }
                strip_empty_rule_content(inner);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_empty_rule_content(item);
            }
        }
        _ => {}
    }
}

/// 事件载荷里的 id 值（`<id>` 白名单键）。用 uuid 而不是序号：观察到的 id 形状都是
/// `sess_/msg_/turn_` 前缀后跟 uuid（夹具里的原始形态被 §3 折叠，但 `msg_<base36>_<uuid>`
/// 形状在 DB 观察里出现过）。
fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 入参 JSON 文本的字节数（观察值：`inputByteLength`）。
fn serialized_len(input: &Value) -> u64 {
    serde_json::to_string(input)
        .map(|text| text.len() as u64)
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
