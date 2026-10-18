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
    DeliveryKind, ModelStreamingKind, ModelStreamingPayload, ProtocolInstant, SessionEvent,
    SessionEventType, ToolBatchPayload, ToolCallIdentity, ToolErrorPayload, ToolResultPayload,
    ToolSchedule, ToolScheduledPayload, ToolStartedPayload, ToolUpdatedPayload,
    TurnCompletedPayload, TurnResultType, TurnStartedPayload,
};
use agent_protocol::ExtraFields;
use agent_tools::{
    ArtifactLocation, FileState, ModeToolFamily, TaskNotification, TaskRegistry, ToolContext,
    ToolOutcome, ToolSpec,
};

use crate::approval::Ruleset;
use crate::permission::{AutoAllow, PermissionHandler, DENY_REASON};
use crate::plugin_tools::{PluginToolDeclaration, PluginToolSource};
use crate::provider::{ModelDelta, ModelMessage, ModelProvider, ModelRequest, ModelUsage};
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
        }
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
            Err(message) => Err(agent_tools::ToolError {
                error_type: "tool_execution_failed",
                message,
                code: Some("TOOL_EXECUTION_FAILED"),
                detail: None,
            }),
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
    pub fn run_pending_notifications(
        &mut self,
        provider: &dyn ModelProvider,
        sink: &mut dyn EventSink,
    ) -> Result<usize, TurnError> {
        let notifications = self.tasks.take_notifications();
        let count = notifications.len();
        for notification in notifications {
            self.run_turn(&notification.to_xml(), provider, sink)?;
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
    /// 载荷只填工具层知道的字段：`turnNumber` 与 `input`。`messageId` / `inputId` / `queryId` /
    /// `foregroundExecutionId` 由会话与模型层产生，这里留空而不是编造（Task 5 补）。
    pub fn begin_turn(&mut self, prompt: &str) -> SessionEvent {
        // 观察值（spec §3.5）：第一个 turn 的 `turnNumber` 是 **0**，之后逐次 +1。
        let turn_number = self.turn_number;
        self.turn_number += 1;
        let turn_id = format!("turn_{turn_number}");
        self.current_turn_id = Some(turn_id.clone());
        let payload = TurnStartedPayload {
            // 观察值：`executionStartedAt` 是小数毫秒。
            execution_started_at: Some(now_ms() as f64),
            turn_number,
            input: prompt.to_string(),
            input_id: None,
            query_id: None,
            message_id: None,
            foreground_execution_id: None,
            extra: ExtraFields::new(),
        };
        self.emit(&turn_id, SessionEventType::TurnStarted, &payload)
    }

    /// 跑完一个完整 turn：模型流 →（可选）工具批次 → 终态。
    ///
    /// 事件顺序按 spec §3.5：`turn.started` → `model.streaming` ×N →（工具事件）→
    /// 下一轮模型流 → `turn.completed`。oracle 还会发若干 `session.updated` /
    /// `streamRecovery.updated`（需要消息计数与追踪 id），本步**不产出**，差异已登记。
    ///
    /// 只实现 `desktop-continuous` 的实时语义；重放归 Host/relay（DR-T5-1）。
    pub fn run_turn(
        &mut self,
        prompt: &str,
        provider: &dyn ModelProvider,
        sink: &mut dyn EventSink,
    ) -> Result<(), TurnError> {
        let started_at = std::time::Instant::now();
        sink.emit(self.begin_turn(prompt));
        self.messages.push(ModelMessage::User(prompt.to_string()));
        let turn_id = self
            .current_turn_id
            .clone()
            .expect("begin_turn 已设置 turn id");
        let tools: Vec<String> = agent_tools::registry()
            .iter()
            .map(|spec| spec.name.clone())
            .chain(self.extra_tools.iter().map(|spec| spec.name.clone()))
            .collect();

        let mut usage = ModelUsage::default();
        let mut model_requests = 0_u64;
        let mut tool_call_count = 0_u64;

        let response_text = loop {
            model_requests += 1;
            let request = ModelRequest {
                messages: self.messages.clone(),
                tools: tools.clone(),
            };
            let response = provider
                .stream(&request)
                .map_err(|error| TurnError::new(error.message))?;
            usage.input_tokens += response.usage.input_tokens;
            usage.output_tokens += response.usage.output_tokens;
            usage.cache_read_tokens += response.usage.cache_read_tokens;
            usage.cache_write_tokens += response.usage.cache_write_tokens;
            usage.reasoning_tokens += response.usage.reasoning_tokens;

            let mut text = String::new();
            let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
            for delta in &response.deltas {
                if let ModelDelta::Text { text: chunk } = delta {
                    text.push_str(chunk);
                }
                if let ModelDelta::ToolCall {
                    tool_call_id,
                    tool_name,
                    input,
                } = delta
                {
                    tool_calls.push(ToolCallRequest {
                        tool_call_id: tool_call_id.clone(),
                        name: tool_name.clone(),
                        input: input.clone(),
                    });
                }
                let event = self.model_streaming_event(&turn_id, model_requests, delta);
                sink.emit(event);
            }

            if tool_calls.is_empty() {
                self.messages.push(ModelMessage::Assistant {
                    text: Some(text.clone()),
                    tool_calls: Vec::new(),
                });
                break text;
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
        };

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
            cache_stats: None,
            input_id: None,
            result_type: TurnResultType::Success,
            background_subagent_result_consumed: None,
            extra: ExtraFields::new(),
        };
        sink.emit(self.emit(&turn_id, SessionEventType::TurnCompleted, &payload));
        Ok(())
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
        self.emit(turn_id, SessionEventType::ModelStreaming, &payload)
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
    /// 最后是整批的 `batch`。
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

        // 一条 assistant 消息里的调用会被并行执行（spec §3 无序组②），所以它们是**一个**并行组；
        // 单调用批次的观察值就是 `parallelGroups: [[id]]`（夹具 `read-single`）。
        let group: Vec<String> = calls.iter().map(|call| call.tool_call_id.clone()).collect();

        for call in calls {
            // 每次调用各自的上下文：artifact 文件名要用本次 `toolCallId`（观察值）。
            let context = self.tool_context_for_call(&call.tool_call_id);
            let spec = self.lookup_tool(&call.name);
            events.push(self.scheduled_event(&turn_id, call, &group));

            // 未注册的工具**没有** `started`（观察：`Glob`/`Grep` 的 `scheduled` 之后直接是
            // `error`）——工具在解析阶段就出局了，没有「开始执行」这一步。
            if let Some(spec) = &spec {
                // `TaskOutput` 的查找是启动前检查（观察：找不到任务时 scheduled → error，
                // **没有** started）；入参校验失败同样发生在这一层。
                if let Some(error) = agent_tools::preflight_error(&call.name, &call.input, &context)
                {
                    error_count += 1;
                    events.push(self.error_event(&turn_id, call, &error));
                    continue;
                }
                match crate::permission::evaluate(spec, &call.input, &self.ruleset) {
                    crate::permission::Decision::Allow => {}
                    crate::permission::Decision::Ask { risk_level, reason } => {
                        let request =
                            self.permission_request(&turn_id, call, spec, risk_level, reason);
                        events.push(self.emit(
                            &turn_id,
                            SessionEventType::PermissionRequested,
                            &request,
                        ));
                        let response = self.permission_handler.request(&request);
                        let decision = response.decision;
                        events.push(self.emit(
                            &turn_id,
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
                events.push(self.started_event(&turn_id, call, spec));
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
                    events.push(self.result_event(&turn_id, call, &result, elapsed_ms));
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
            input: call.input.clone(),
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
            // 观察值：`assistantMessageId` 由模型层给；工具面只知道「同一条消息里的这一批」。
            assistant_message_id: None,
            input: None,
            // 观察值：入参 JSON 文本的字节数（`{"pattern":"*.txt"}` → 19）。
            input_byte_length: Some(serialized_len(&call.input)),
            // 观察值：调度事件故意不带入参，入参只在 model.streaming 里（spec §1.6）。
            input_omitted: Some(true),
            input_ref: Some("model_stream".to_string()),
            dependencies: Some(Vec::new()),
            parallel_group_index: Some(0),
            can_run_parallel: Some(true),
            schedule: Some(ToolSchedule {
                parallel_groups: Some(vec![group.to_vec()]),
                execution_order: Some(group.to_vec()),
                extra: ExtraFields::new(),
            }),
            extra: ExtraFields::new(),
        };
        self.emit_tool(turn_id, ToolUpdatedPayload::Scheduled(payload))
    }

    fn started_event(
        &mut self,
        turn_id: &str,
        call: &ToolCallRequest,
        spec: &ToolSpec,
    ) -> SessionEvent {
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
            // 观察值：只读工具的 `started` 带 `readOnly: true` 与 `sideEffectScope: "none"`；
            // `Write`（`file-write`）带 `readOnly: false` 与 `sideEffectScope: "workspace"`
            // （夹具 `write-new` / `write-overwrite-read` 的 `started` 事件）。
            // 其余非只读档（shell / node-repl …）的取值集合**仍未观察**，所以那种情况留空而非编一个档位。
            read_only: Some(spec.read_only),
            side_effect_scope: if let Some(meta) = &spec.plugin {
                // 观察：插件 MCP 是 `network`，内置 node_repl 是 `system`。
                Some(meta.side_effect_scope.clone())
            } else {
                match (spec.read_only, spec.family) {
                    (true, _) => Some("none".to_string()),
                    (false, ModeToolFamily::FileWrite) => Some("workspace".to_string()),
                    // 观察（spec §3.4）：`TaskStop` 的 started 是 `sideEffectScope:"session"`。
                    (false, ModeToolFamily::TaskControl) => Some("session".to_string()),
                    (false, _) => None,
                }
            },
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
            result: result_object(result, elapsed_ms),
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

    /// 发一条 `tool.updated`：载荷是**带 `kind` 判别键的枚举**，所以包信封的是枚举而不是内部结构体
    /// （只序列化内部结构体会丢掉 `kind`，实测解回来报 `missing field «kind»`）。
    fn emit_tool(&mut self, turn_id: &str, payload: ToolUpdatedPayload) -> SessionEvent {
        self.emit(turn_id, SessionEventType::ToolUpdated, &payload)
    }

    /// 包一个会话事件信封。
    ///
    /// `eventId` 用单调 id 而不是 uuid：spec §3 把 `eventId` 归一成 `<id>`，它的真实形态不是
    /// 被对拍的行为，单调 id 反而让用例可以断言。`traceId` 留空（追踪 id 由模型/会话层给）。
    fn emit<T: Serialize>(
        &mut self,
        turn_id: &str,
        event_type: SessionEventType,
        payload: &T,
    ) -> SessionEvent {
        self.sequence += 1;
        SessionEvent {
            event_id: format!("evt_{}", self.sequence),
            session_id: self.session_id.clone(),
            turn_id: Some(turn_id.to_string()),
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
