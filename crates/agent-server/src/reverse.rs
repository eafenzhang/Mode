//! 反向请求 hub 与审批接线（LT-1 C1 · Task 8）。
//!
//! stdio 上的反向请求（运行时 → 宿主）与它们的应答共用**一条连接**上的单个
//! 出站写者（[`crate::server::run_stdio`] 的循环）：本模块维护
//! 「id → 等待方」的 pending 映射，宿主应答帧（`Response` / `ErrorResponse`）到达时
//! 按 id 认领并唤醒等待方；连接关闭（EOF / 写失败）时 `close` 失败**所有** pending，
//! 等待方一律按 fail-closed 处理（审批 → deny，偏好/凭据 → 回退默认）。
//!
//! # 两种等待形态
//!
//! - **阻塞**（[`ProtocolPermissionHandler`]）：`agent_core::PermissionHandler` 是同步
//!   trait，会话循环在 `run_tool_batch` 里同步调用 [`PermissionHandler::request`]。
//!   turn 因此运行在 `spawn_blocking` 线程上（[`crate::runtime`]），handler 在该线程
//!   上用 `std::sync::mpsc` + `recv_timeout` 等待——**不占用** tokio worker 线程，
//!   current-thread 测试 runtime 也不会死锁（应答由 stdio 循环任务投递，`send` 是
//!   同步无阻塞调用）。
//! - **异步**（runtime 偏好请求）：worker 任务在物化后用 tokio oneshot 等待。
//!
//! # 超时与回退
//!
//! 观察值（`docs/specs/tauri-agent-runtime.md` §1.3）：宿主对未应答的反向请求
//! `15s` 后报 `-32022 Client request timed out`。本侧对所有出站反向请求使用同一
//! 超时（[`REVERSE_REPLY_TIMEOUT`]）：审批超时 → deny；偏好超时 → 登记保守默认。
//! 未知方法等错误应答（`-32601` 等）同样按失败回退——运行时只发合法方法与参数，
//! 错误应答说明宿主不支持该钩子。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::PermissionHandler;
use agent_protocol::frame::{Frame, RequestFrame, RequestId, ReverseRequestIds};
use agent_protocol::permission::{PermissionDecision, PermissionRequest, PermissionResponse};
use agent_protocol::provider_runtime_headers::{
    ProviderRuntimeHeadersRequestParams, ProviderRuntimeHeadersResponse,
    PROVIDER_RUNTIME_HEADERS_METHOD,
};
use serde_json::Value;
use tokio::sync::mpsc;

/// 审批反向请求之外的第二条钩子：运行时偏好（spec §1.3）。
pub const RUNTIME_PREFERENCES_METHOD: &str = "session/requestRuntimePreferences";

/// 反向请求应答超时（观察：宿主 15s 未应答报 `-32022`，spec §1.3）。
pub const REVERSE_REPLY_TIMEOUT: Duration = Duration::from_secs(15);

/// 宿主对 `session/requestRuntimePreferences` 的应答（Task 8 控制器裁决 3）。
///
/// `raw` 原样保留宿主应答（未建模字段不丢）；四个已知字段缺失时按
/// [`RuntimePreferences::fallback`] 的同键取值补——「未列出的字段可能缺席，宿主
/// 怎么答就怎么用」按字段生效。回退取值来自 spec §1.3 的宿主应答表（唯一观察到的
/// 默认应答形状）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePreferences {
    pub ask_user_question_auto_resolution_enabled: bool,
    pub native_search_enhancements_enabled: bool,
    pub memory_enabled: bool,
    pub model_context_budget_strategy: Option<String>,
    /// 宿主的原始应答（未建模字段原样保留）。
    pub raw: Value,
}

impl RuntimePreferences {
    /// 保守默认（spec §1.3 宿主应答表的逐字取值）。
    pub fn fallback() -> Self {
        Self {
            ask_user_question_auto_resolution_enabled: true,
            native_search_enhancements_enabled: false,
            memory_enabled: false,
            model_context_budget_strategy: Some("preflight-v1".to_string()),
            raw: serde_json::json!({
                "askUserQuestionAutoResolutionEnabled": true,
                "nativeSearchEnhancementsEnabled": false,
                "memoryEnabled": false,
                "modelContextBudgetStrategy": "preflight-v1",
            }),
        }
    }

    /// 从宿主应答解析；缺失字段回退默认，`raw` 恒为原样应答。
    fn from_reply(value: &Value) -> Self {
        let fallback = Self::fallback();
        let flag =
            |key: &str, default: bool| value.get(key).and_then(Value::as_bool).unwrap_or(default);
        Self {
            ask_user_question_auto_resolution_enabled: flag(
                "askUserQuestionAutoResolutionEnabled",
                fallback.ask_user_question_auto_resolution_enabled,
            ),
            native_search_enhancements_enabled: flag(
                "nativeSearchEnhancementsEnabled",
                fallback.native_search_enhancements_enabled,
            ),
            memory_enabled: flag("memoryEnabled", fallback.memory_enabled),
            model_context_budget_strategy: value
                .get("modelContextBudgetStrategy")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| fallback.model_context_budget_strategy.clone()),
            raw: value.clone(),
        }
    }
}

/// 反向请求的失败面（不携带宿主应答的敏感内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReverseRequestError {
    /// 出站通道关闭（连接已结束）。
    TransportClosed,
    /// 超过 [`REVERSE_REPLY_TIMEOUT`]（或构造时给定的超时）没有应答。
    Timeout,
    /// 宿主回了错误应答（`-32601` / `-32602` / …）。
    Rejected { code: i64, message: String },
}

impl ReverseRequestError {
    /// 供日志/诊断的稳定短语；不含凭据。
    pub fn reason(&self) -> String {
        match self {
            Self::TransportClosed => "reverse request transport closed".to_string(),
            Self::Timeout => "reverse request timed out".to_string(),
            Self::Rejected { code, message } => {
                format!("reverse request rejected ({code}): {message}")
            }
        }
    }
}

/// pending 映射的一格：等待方是阻塞线程（审批）还是异步任务（偏好/凭据）。
enum PendingReply {
    Blocking(std::sync::mpsc::Sender<Result<Value, ReverseRequestError>>),
    Async(tokio::sync::oneshot::Sender<Result<Value, ReverseRequestError>>),
}

/// 一条 stdio 连接的反向请求 hub：id 分配、pending 认领、单写者出站通道。
///
/// 生命周期与连接一致：[`crate::server::run_stdio`] 在启动时构造并注入
/// [`crate::dispatch::AgentServer`]，循环里把入向应答帧交给 [`Self::deliver`]，
/// 退出时 [`Self::close`] 失败所有 pending。多个会话共享同一实例
/// （id 由 [`ReverseRequestIds`] 保证连接级唯一）。
pub struct ReverseHub {
    ids: ReverseRequestIds,
    outbound: mpsc::UnboundedSender<Frame>,
    pending: Mutex<HashMap<RequestId, PendingReply>>,
    reply_timeout: Duration,
}

impl ReverseHub {
    /// 生产构造：出站通道的发送端交给 hub，接收端由 run_stdio 的单写者循环消费。
    pub fn new(outbound: mpsc::UnboundedSender<Frame>) -> Arc<Self> {
        Self::with_timeout(outbound, REVERSE_REPLY_TIMEOUT)
    }

    /// 测试/嵌入构造：显式超时（超时回退用例用短超时，不真的等 15s）。
    pub fn with_timeout(
        outbound: mpsc::UnboundedSender<Frame>,
        reply_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            ids: ReverseRequestIds::default(),
            outbound,
            pending: Mutex::new(HashMap::new()),
            reply_timeout,
        })
    }

    /// 入向应答帧认领：`Response` 按 id 投递 result，`ErrorResponse` 投递失败。
    /// 其余帧（请求 / 通知）不是应答，忽略（宿主请求走 [`crate::dispatch`]）。
    pub fn deliver(&self, frame: &Frame) {
        match frame {
            Frame::Response(response) => {
                self.resolve(&response.id, Ok(response.result.clone()));
            }
            Frame::ErrorResponse(error) => {
                self.resolve(
                    &error.id,
                    Err(ReverseRequestError::Rejected {
                        code: error.error.code,
                        message: error.error.message.clone(),
                    }),
                );
            }
            _ => {}
        }
    }

    /// 测试/嵌入投递入口：直接以宿主身份应答一个 pending 请求。
    pub fn deliver_response(&self, id: &RequestId, result: Value) {
        self.resolve(id, Ok(result));
    }

    /// 测试/嵌入投递入口：以宿主身份回错误应答（`-32601` 回退用例）。
    pub fn deliver_error(&self, id: &RequestId, code: i64, message: &str) {
        self.resolve(
            id,
            Err(ReverseRequestError::Rejected {
                code,
                message: message.to_string(),
            }),
        );
    }

    /// 连接结束：失败**所有** pending（掉 sender → 阻塞 recv 立即断开 / oneshot
    /// await 返回 Err），等待方按各自语义 fail-closed。
    pub fn close(&self) {
        self.pending
            .lock()
            .expect("reverse pending poisoned")
            .clear();
    }

    fn resolve(&self, id: &RequestId, outcome: Result<Value, ReverseRequestError>) {
        let pending = self
            .pending
            .lock()
            .expect("reverse pending poisoned")
            .remove(id);
        match pending {
            Some(PendingReply::Blocking(sender)) => {
                // 等待方已超时/放弃时 send 失败——忽略（应答迟到）。
                let _ = sender.send(outcome);
            }
            Some(PendingReply::Async(sender)) => {
                let _ = sender.send(outcome);
            }
            None => {}
        }
    }

    /// 审批（阻塞形态）：把 [`PermissionRequest`] 封成
    /// `interaction/requestPermission` 反向请求帧发出，阻塞等宿主应答。
    ///
    /// 失败一律 **deny**（fail-closed，见 [`denied`]）：序列化失败、通道关闭、
    /// 超时、错误应答、应答解不开。调用方运行在 `spawn_blocking` 线程上
    /// （[`crate::runtime`]），这里的阻塞不占 tokio worker。
    pub fn request_permission(&self, request: &PermissionRequest) -> PermissionResponse {
        let id = self.ids.next();
        let frame = match request.to_request_frame(id.clone()) {
            Ok(frame) => frame,
            Err(error) => return denied(&format!("permission request frame failed: {error}")),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        // 先注册再发送：应答可能在发送完成前到达，先注册才不会丢认领。
        self.pending
            .lock()
            .expect("reverse pending poisoned")
            .insert(id.clone(), PendingReply::Blocking(sender));
        if self.outbound.send(frame).is_err() {
            self.pending
                .lock()
                .expect("reverse pending poisoned")
                .remove(&id);
            return denied("permission transport closed");
        }
        match receiver.recv_timeout(self.reply_timeout) {
            Ok(Ok(result)) => match serde_json::from_value::<PermissionResponse>(result) {
                Ok(response) => response,
                Err(error) => denied(&format!("permission reply is invalid: {error}")),
            },
            Ok(Err(failure)) => denied(&format!("permission request failed: {}", failure.reason())),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.pending
                    .lock()
                    .expect("reverse pending poisoned")
                    .remove(&id);
                denied("permission reply timed out")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                denied("permission transport closed")
            }
        }
    }

    /// 运行时偏好（异步形态）：物化后发一次 `session/requestRuntimePreferences`
    /// （scope = `runtime-materialization`，控制器裁决 3：create/resume 各一次，
    /// 不随 send 重复）。`-32601` / 超时 / 关闭 / 应答解不开 → 登记保守默认。
    pub async fn request_runtime_preferences(&self, session_id: &str) -> RuntimePreferences {
        self.request_runtime_preferences_scoped(session_id, "runtime-materialization")
            .await
    }

    /// 运行时偏好的**带 scope** 形态（Task 10）。
    ///
    /// oracle 观察（`agent-server-c1-contract.md` §2.2）：每次 runtime materialize
    /// 是**一对**偏好请求——`runtime-materialization` 之后还有一次 `user-execution`
    /// （create 路径在首个 send 的 turn 开始前发出）。本方法按调用方给定的 scope
    /// 发请求并把应答解析进 [`RuntimePreferences`]；失败一律保守默认（与
    /// [`Self::request_runtime_preferences`] 同口径）。
    pub async fn request_runtime_preferences_scoped(
        &self,
        session_id: &str,
        scope: &str,
    ) -> RuntimePreferences {
        self.request_runtime_preferences_scoped_with_timeout(session_id, scope, self.reply_timeout)
            .await
    }

    /// 带显式上限的偏好请求：worker 首 turn 的偏好只能**短暂**借用 turn 启动路径，
    /// 宿主不回/回得慢时按保守默认继续（不能 15s×2 拖住建会话）。
    pub async fn request_runtime_preferences_scoped_with_timeout(
        &self,
        session_id: &str,
        scope: &str,
        reply_timeout: Duration,
    ) -> RuntimePreferences {
        let params = serde_json::json!({
            "sessionId": session_id,
            "scope": scope,
        });
        match self
            .request_async_with_timeout(RUNTIME_PREFERENCES_METHOD, params, reply_timeout)
            .await
        {
            Ok(value) => RuntimePreferences::from_reply(&value),
            Err(_) => RuntimePreferences::fallback(),
        }
    }

    /// provider 运行时头（阻塞形态；`HubProviderSource::resolve` 运行在
    /// `spawn_blocking` 线程上）：账号型 provider 的鉴权材料来自宿主。
    ///
    /// 返回 `Err(reason)` 时调用方落回「缺 `access.apiKey`」的显式错误——
    /// `headersApplied:false`、超时、错误应答、应答解不开都不再试一次。
    pub fn request_provider_runtime_headers(
        &self,
        params: ProviderRuntimeHeadersRequestParams,
    ) -> Result<ProviderRuntimeHeadersResponse, String> {
        let value = match serde_json::to_value(&params) {
            Ok(value) => value,
            Err(error) => return Err(format!("provider headers params failed: {error}")),
        };
        match self.request_blocking(PROVIDER_RUNTIME_HEADERS_METHOD, value) {
            Ok(reply) => serde_json::from_value::<ProviderRuntimeHeadersResponse>(reply)
                .map_err(|error| format!("provider headers reply is invalid: {error}")),
            Err(failure) => Err(failure.reason()),
        }
    }

    /// 异步请求（worker 任务用）：注册 oneshot → 发帧 → 超时等待。
    async fn request_async_with_timeout(
        &self,
        method: &str,
        params: Value,
        reply_timeout: Duration,
    ) -> Result<Value, ReverseRequestError> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let id = self.send_request(method, params, PendingReply::Async(sender))?;
        match tokio::time::timeout(reply_timeout, receiver).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_recv_closed)) => Err(ReverseRequestError::TransportClosed),
            Err(_elapsed) => {
                self.pending
                    .lock()
                    .expect("reverse pending poisoned")
                    .remove(&id);
                Err(ReverseRequestError::Timeout)
            }
        }
    }

    /// 阻塞请求（`spawn_blocking` 线程上的 hub 消费用）：与审批同一形态。
    fn request_blocking(&self, method: &str, params: Value) -> Result<Value, ReverseRequestError> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let id = self.send_request(method, params, PendingReply::Blocking(sender))?;
        match receiver.recv_timeout(self.reply_timeout) {
            Ok(outcome) => outcome,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.pending
                    .lock()
                    .expect("reverse pending poisoned")
                    .remove(&id);
                Err(ReverseRequestError::Timeout)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(ReverseRequestError::TransportClosed)
            }
        }
    }

    /// 分配 id、注册 pending、把请求帧交给出站通道（单写者循环负责落盘）。
    fn send_request(
        &self,
        method: &str,
        params: Value,
        pending: PendingReply,
    ) -> Result<RequestId, ReverseRequestError> {
        let id = self.ids.next();
        // 先注册再发送：应答可能在发送返回前被循环读走，先注册才不会丢认领。
        self.pending
            .lock()
            .expect("reverse pending poisoned")
            .insert(id.clone(), pending);
        let frame = Frame::Request(RequestFrame {
            id: id.clone(),
            method: method.to_string(),
            params: Some(params),
            trace: None,
        });
        if self.outbound.send(frame).is_err() {
            self.pending
                .lock()
                .expect("reverse pending poisoned")
                .remove(&id);
            return Err(ReverseRequestError::TransportClosed);
        }
        Ok(id)
    }
}

impl std::fmt::Debug for ReverseHub {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // pending 数量可暴露，内容不暴露：审批/凭据应答里可能有敏感值。
        let pending = self
            .pending
            .lock()
            .map(|pending| pending.len())
            .unwrap_or(0);
        formatter
            .debug_struct("ReverseHub")
            .field("pending", &pending)
            .finish_non_exhaustive()
    }
}

/// fail-closed 的审批答复：`deny` + 原因（不携带任何请求入参）。
fn denied(reason: &str) -> PermissionResponse {
    PermissionResponse {
        decision: PermissionDecision::Deny,
        reason: Some(reason.to_string()),
        modified_input: None,
        permission_updates: None,
        extra: agent_protocol::ExtraFields::new(),
    }
}

/// 审批接线：把 [`PermissionRequest`] 经 [`ReverseHub`] 发成
/// `interaction/requestPermission` 反向请求并等宿主答复。
///
/// 实现 [`PermissionHandler`]（同步 trait）：调用点在 turn 的 `spawn_blocking`
/// 线程上（[`crate::runtime`]），阻塞等待不占 tokio worker。宿主会话里
/// `PermissionRequested` / `PermissionResolved` 事件仍由 `agent-core` 的会话循环
/// 照常发出并进事件日志/总线——本结构只负责「问宿主、取答复」。
pub struct ProtocolPermissionHandler {
    hub: Arc<ReverseHub>,
}

impl ProtocolPermissionHandler {
    pub fn new(hub: Arc<ReverseHub>) -> Self {
        Self { hub }
    }
}

impl PermissionHandler for ProtocolPermissionHandler {
    fn request(&mut self, request: &PermissionRequest) -> PermissionResponse {
        self.hub.request_permission(request)
    }
}

/// 会话运行时的反向请求接线（Task 8 控制器裁决 1/2）。
///
/// `permission_handler`：审批走宿主（缺省 `None` = `agent-core` 的 `AutoAllow`，
/// 等价于驱动默认应答——不经 stdio）。`rule_db_path`：`allow_project` 的
/// `local_setting` 行落在**与 session-store 同一个库**（store 的锁不受影响，
/// 规则写入用短连接）。`reverse`：偏好/凭据钩子的连接级 hub。
#[derive(Default)]
pub struct RuntimeWiring {
    pub permission_handler: Option<Box<dyn PermissionHandler>>,
    pub rule_db_path: Option<PathBuf>,
    pub reverse: Option<Arc<ReverseHub>>,
    /// C4 T1：会话的 MCP 工具源（首 turn 才建连；失败 server 不阻断 turn）。
    pub mcp: Option<Arc<crate::mcp::McpSession>>,
    /// C4 T2：MCP 连接 hub（插件 MCP 与 `mcp/list` 共用同一张表）。
    pub mcp_hub: Arc<crate::mcp::McpHub>,
    /// C4 T5：子代理执行端口（`Agent` 工具）。
    pub subagent_runner: Option<Arc<dyn agent_core::SubagentRunner>>,
    /// LT-15 P5：sessions-index hub（phase/预览 live 投影用）。
    pub sessions_index: Option<Arc<crate::sessions_index::SessionsIndexHub>>,
}
