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

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use agent_protocol::session_event::{
    DeliveryKind, ProtocolInstant, SessionEvent, SessionEventType, ToolBatchPayload,
    ToolCallIdentity, ToolErrorPayload, ToolResultPayload, ToolSchedule, ToolScheduledPayload,
    ToolStartedPayload, ToolUpdatedPayload, TurnStartedPayload,
};
use agent_protocol::ExtraFields;
use agent_tools::{lookup, ReadHistory, ToolContext, ToolOutcome, ToolSpec};

use crate::tool::{error_object, result_object};

/// 模型流里的一条工具调用（`model.streaming` / `kind:"tool_call"` 的入参来源，spec §1.6）。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallRequest {
    pub tool_call_id: String,
    pub name: String,
    pub input: Value,
}

/// 会话的工具执行循环。
///
/// **只拥有工具面**：不持有消息历史、不发模型请求、不落盘（那些是宿主与后续任务的事）。
pub struct Session {
    session_id: String,
    cwd: String,
    turn_number: u64,
    sequence: u64,
    current_turn_id: Option<String>,
    /// `Read` 的**会话内**重复读记忆（跨 turn 命中；见 `agent_tools::ReadHistory`）。
    read_history: ReadHistory,
}

impl Session {
    pub fn new(session_id: impl Into<String>, cwd: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            cwd: cwd.into(),
            turn_number: 0,
            sequence: 0,
            current_turn_id: None,
            read_history: ReadHistory::new(),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 工具执行上下文（工作目录就是会话工作目录）。
    pub fn tool_context(&self) -> ToolContext {
        ToolContext {
            cwd: self.cwd.clone(),
        }
    }

    /// 开一个 turn：产出 `turn.started`，并把它设为后续 [`Self::run_tool_batch`] 的归属 turn。
    ///
    /// 载荷只填工具层知道的字段：`turnNumber` 与 `input`。`messageId` / `inputId` / `queryId` /
    /// `foregroundExecutionId` 由会话与模型层产生，这里留空而不是编造（Task 5 补）。
    pub fn begin_turn(&mut self, prompt: &str) -> SessionEvent {
        self.turn_number += 1;
        let turn_id = format!("turn_{}", self.turn_number);
        self.current_turn_id = Some(turn_id.clone());
        let payload = TurnStartedPayload {
            // 观察值：`executionStartedAt` 是小数毫秒。
            execution_started_at: Some(now_ms() as f64),
            turn_number: self.turn_number,
            input: prompt.to_string(),
            input_id: None,
            query_id: None,
            message_id: None,
            foreground_execution_id: None,
            extra: ExtraFields::new(),
        };
        self.emit(&turn_id, SessionEventType::TurnStarted, &payload)
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
        let context = self.tool_context();
        let mut events = Vec::with_capacity(calls.len() * 3 + 1);
        let mut success_count = 0_u64;
        let mut error_count = 0_u64;

        // 一条 assistant 消息里的调用会被并行执行（spec §3 无序组②），所以它们是**一个**并行组；
        // 单调用批次的观察值就是 `parallelGroups: [[id]]`（夹具 `read-single`）。
        let group: Vec<String> = calls.iter().map(|call| call.tool_call_id.clone()).collect();

        for call in calls {
            let spec = lookup(&call.name);
            events.push(self.scheduled_event(&turn_id, call, &group));

            // 未注册的工具**没有** `started`（观察：`Glob`/`Grep` 的 `scheduled` 之后直接是
            // `error`）——工具在解析阶段就出局了，没有「开始执行」这一步。
            if let Some(spec) = &spec {
                events.push(self.started_event(&turn_id, call, spec));
            }

            // `Read` 的重复读去重：**会话状态**在这一层（工具本身是纯函数），见 ReadHistory 的注释。
            // 命中就不调用工具，直接发一条与 oracle 同形的成功结果；
            // 只有**成功**的读才记进历史（实测：读不存在的文件两次都是错误，错误不去重）。
            let started = std::time::Instant::now();
            let stubbed = if call.name == "Read" {
                self.read_history.stub_for(&call.input, &context)
            } else {
                None
            };
            let outcome = match stubbed {
                Some(stub) => Ok(stub),
                None => {
                    let result = agent_tools::invoke_by_name(&call.name, &call.input, &context);
                    if call.name == "Read" && result.is_ok() {
                        self.read_history.record(&call.input, &context);
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
            // 观察值：只读工具的 `started` 带 `readOnly: true` 与 `sideEffectScope: "none"`。
            // 非只读工具的 `sideEffectScope` 取值集合没有观察（镜像注释也这么说），
            // 所以那种情况**留空**而不是编一个档位。
            read_only: Some(spec.read_only),
            side_effect_scope: if spec.read_only {
                Some("none".to_string())
            } else {
                None
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
            // 观察值：对拍驱动订阅的是 `desktop-continuous`（协议夹具的信封）。
            delivery_kind: Some(DeliveryKind::DesktopContinuous),
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
