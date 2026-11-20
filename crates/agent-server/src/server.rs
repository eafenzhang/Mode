//! stdio 分帧与线上四类帧（请求 / 通知 / 应答 / 错误应答）。
//!
//! 帧类型与严格度由 `agent-protocol` 拥有（`Frame` / `RequestFrame` /
//! `NotificationFrame` / `ResponseFrame` / `ErrorResponseFrame` / `RequestId`）；
//! 本模块只负责分帧、把 `serde_json::Value` 映射成帧，以及把分派结果写回。
//!
//! 帧边界只认 `\n`：U+2028/U+2029 是 JSON 字符串里的普通字符，不能当换行；
//! CRLF 在解析前去掉行尾 `\r`；空行忽略；读到 EOF 时残留缓冲区仍按一帧处理。
//! 分帧口径与 `docs/specs/mode-protocol-client.md`「帧」一致（客户端与运行时
//! 共享同一套线上规则）。

use std::fmt;
use std::io;
use std::sync::Arc;

use agent_protocol::v4_transport::{
    ConversationFramePayload, ConversationTopicFrame, SnapshotFramePayload,
    V4_WIRE_PROTOCOL_VERSION,
};
use agent_protocol::{
    ErrorObject, ErrorResponseFrame, Frame, NotificationFrame, RequestFrame, ResponseFrame,
};
use serde_json::{Number, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::dispatch::{AgentServer, PendingLiveForward};
use crate::reverse::ReverseHub;

/// 单帧上限（防御异常刷屏）；与 `mode-protocol-client` 同口径，正常协议帧远小于此。
///
/// **实现边界**：上限在整行读入后检查（`read_until`），超限帧仍会先占用缓冲区；
/// 有界读取未实现。正常协议帧远小于上限，且这是本进程唯一的入站连接（宿主 stdio）；
/// 残余风险登记在 `tauri-agent-runtime.md` §C1.5-9，不在本波改成流式帧解析。
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// 把已解析的 JSON 值转成 `agent-protocol` 的帧；形状不合法映射为协议错误。
///
/// `agent-protocol` 的帧变体带 `deny_unknown_fields`：信封上多一个未知键会被整体
/// 拒绝。这是与 Task 1 初版本地宽松解析器的已知差异（见任务报告）。
pub fn frame_from_value(value: Value) -> Result<Frame, ServerError> {
    serde_json::from_value::<Frame>(value)
        .map_err(|error| ServerError::invalid_frame(format!("invalid frame: {error}")))
}

/// Agent Server 的失败面。`code` 是 JSON-RPC 错误码，直接写进错误应答。
#[derive(Debug)]
pub enum ServerError {
    /// `-32601`：未知/未实现的方法（C1 未支持的方法显式走这里，不返回空结果）。
    MethodNotFound { method: String },
    /// `-32602`：参数不合法（后续方法 handler 使用）。
    InvalidParams { message: String },
    /// `-32700`：非法 JSON 或帧形状不合法，按协议错误关闭连接。
    InvalidFrame { message: String },
    /// `-32603`：内部错误。
    Internal { message: String },
    /// 自定义 JSON-RPC 错误码 + 可选 `data`（C2 compact 的观察形状：
    /// `-32009` revision 冲突带 `{actualRevision, expectedRevision}`、
    /// `-32010` prompt 运行中拒绝，见 `agent-c2-contract.md` §1.1）。
    Rpc {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    /// stdio 传输错误。
    Io(io::Error),
}

impl ServerError {
    pub fn method_not_found(method: impl Into<String>) -> Self {
        Self::MethodNotFound {
            method: method.into(),
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::InvalidParams {
            message: message.into(),
        }
    }

    pub fn invalid_frame(message: impl Into<String>) -> Self {
        Self::InvalidFrame {
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    /// 自定义错误码 + `data`（compact 的 `-32009` / `-32010` 观察形状）。
    pub fn rpc(code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        Self::Rpc {
            code,
            message: message.into(),
            data,
        }
    }

    pub fn code(&self) -> i64 {
        match self {
            Self::MethodNotFound { .. } => -32601,
            Self::InvalidParams { .. } => -32602,
            Self::InvalidFrame { .. } => -32700,
            Self::Internal { .. } | Self::Io(_) => -32603,
            Self::Rpc { code, .. } => *code,
        }
    }

    /// 写进线上错误应答的 `message`。未知方法固定 `Method not found`（spec §5.1）。
    pub fn message(&self) -> String {
        match self {
            Self::MethodNotFound { .. } => "Method not found".to_string(),
            Self::InvalidParams { message }
            | Self::InvalidFrame { message }
            | Self::Internal { message }
            | Self::Rpc { message, .. } => message.clone(),
            Self::Io(error) => format!("stdio transport error: {error}"),
        }
    }

    /// 错误应答的可选 `data`（`Rpc` 携带结构化细节，其余为 `None`）。
    pub fn data(&self) -> Option<Value> {
        match self {
            Self::Rpc { data, .. } => data.clone(),
            _ => None,
        }
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MethodNotFound { method } => {
                write!(formatter, "Method not found: {method}")
            }
            Self::InvalidParams { message }
            | Self::InvalidFrame { message }
            | Self::Internal { message }
            | Self::Rpc { message, .. } => formatter.write_str(message),
            Self::Io(error) => write!(formatter, "stdio transport error: {error}"),
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ServerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// 在任意异步双向流上跑 `app-server --stdio` 循环。
///
/// 结构（Task 7）：主循环在「当前入站帧的读取」与「出站通知通道」之间 select。
/// `session/subscribe` 的 live 事件由转发任务写进出站通道，主循环统一落盘，
/// 应答与事件不会在 write 端交错。**读取 future 按帧持有**（`Pin<&mut …>` 一直
/// 存活到帧完成）：select 被出站帧打断时不会重建读取状态——`read_until` 已追加
/// 进缓冲的字节与扫描进度都在，下一次 poll 从断点继续（取消安全）。
///
/// 反向请求（Task 8）：启动时构造连接级 [`ReverseHub`] 并注入 server（此后物化的
/// 会话接审批/偏好/凭据钩子）；入向 `Response` / `ErrorResponse` 帧交给 hub 认领
/// pending。退出（EOF / 错误）时 `close()` 失败所有 pending——等待方按各自语义
/// fail-closed（审批 deny、偏好回退默认），不会悬挂。
///
/// 启动顺序：**先跑存储门**（`startup/storageState` 通知直接写出，保证它们是本
/// 连接的第一批帧），再进循环。EOF 干净返回 `Ok(())`；非法 JSON / 非法帧返回
/// 错误并关闭连接。
pub async fn run_stdio<R, W>(
    reader: R,
    mut writer: W,
    server: Arc<AgentServer>,
) -> Result<(), ServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    server.run_storage_gate(&mut writer).await?;

    let (outbound, mut outbound_rx) = mpsc::unbounded_channel::<Frame>();
    let reverse = ReverseHub::new(outbound.clone());
    server.set_reverse_hub(Arc::clone(&reverse));
    let result = stdio_loop(
        reader,
        &mut writer,
        Arc::clone(&server),
        outbound,
        &mut outbound_rx,
        &reverse,
    )
    .await;
    // 连接结束（EOF/错误/非法帧）：失败所有 pending 反向请求（审批 deny 等），
    // 并把 hub 从 server 摘除——同一 server 复用给新连接时不残留旧通道。
    reverse.close();
    server.clear_reverse_hub();
    result
}

/// stdio 主循环（[`run_stdio`] 的主体；hub 的生命周期由外层管理）。
async fn stdio_loop<R, W>(
    reader: R,
    writer: &mut W,
    server: Arc<AgentServer>,
    outbound: mpsc::UnboundedSender<Frame>,
    outbound_rx: &mut mpsc::UnboundedReceiver<Frame>,
    reverse: &Arc<ReverseHub>,
) -> Result<(), ServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        // 每帧一个读取 future；被出站分支打断不重建（见函数注释的取消安全说明）。
        line.clear();
        let mut read_frame = std::pin::pin!(reader.read_until(b'\n', &mut line));
        let outcome = loop {
            tokio::select! {
                biased;
                read = &mut read_frame => break read,
                Some(frame) = outbound_rx.recv() => write_frame(writer, &frame).await?,
            }
        };
        // `read_frame` 在此作用域结束自然释放（Pin<&mut> 不需要显式 drop）。

        let read = outcome.map_err(ServerError::Io)?;
        if read == 0 {
            // EOF：`read_until` 已先把残留缓冲区交回（没有换行也会返回一帧）。
            return Ok(());
        }
        if line.len() > MAX_FRAME_BYTES {
            return Err(ServerError::invalid_frame(format!(
                "frame exceeds {MAX_FRAME_BYTES} bytes"
            )));
        }
        // 只认 `\n` 为边界；CRLF 在解析前去掉行尾 `\r`。U+2028/U+2029 是多字节
        // UTF-8 字符，不会命中上面的 `\n`。
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_slice(&line)
            .map_err(|error| ServerError::invalid_frame(format!("invalid JSON frame: {error}")))?;
        let frame = match frame_from_value(value) {
            Ok(frame) => frame,
            Err(error) => {
                eprintln!(
                    "[dbg] invalid frame line: {}",
                    String::from_utf8_lossy(&line)
                );
                return Err(error);
            }
        };
        line.clear();

        match frame {
            Frame::Request(request) => {
                let RequestFrame {
                    id, method, params, ..
                } = request;
                let response = match server.dispatch(&method, params.unwrap_or(Value::Null)) {
                    Ok(result) => Frame::Response(ResponseFrame { id, result }),
                    Err(error) => Frame::ErrorResponse(ErrorResponseFrame {
                        id,
                        error: ErrorObject {
                            code: error.code(),
                            message: error.message(),
                            data: error.data(),
                        },
                    }),
                };
                write_frame(writer, &response).await?;
                // response 已落盘才 spawn live 转发：订阅的 replay（response 内）
                // 永远先于 live 通知。
                for forward in server.take_pending_forwards() {
                    tokio::spawn(run_live_forward(forward, outbound.clone()));
                }
                // C4 T4c：v4 帧 live 转发（初始快照帧由任务先发，再接增量）。
                for forward in server.take_pending_v4_forwards() {
                    tokio::spawn(run_v4_forward(
                        Arc::clone(&server),
                        forward,
                        outbound.clone(),
                    ));
                }
            }
            // 通知（宿主→server）暂无订阅者。
            Frame::Notification(_) => {}
            // 反向请求的应答：交给 hub 按 id 认领 pending（审批/偏好/凭据）。
            Frame::Response(_) | Frame::ErrorResponse(_) => reverse.deliver(&frame),
        }
    }
}

/// 从 DB + 当前 runtime 构造 conversation 快照（初始帧与 recovery 帧共用）。
fn conversation_snapshot(
    server: &AgentServer,
    session_id: &str,
    log_epoch: &str,
) -> Option<(agent_protocol::v4_snapshot::ConversationSnapshot, u64)> {
    let store = server.store().ok()?;
    let messages = store.messages(session_id, None, None).ok()?;
    let loaded: Vec<crate::methods::v4_frames::LoadedMessage> = messages
        .into_iter()
        .map(|with_parts| {
            (
                with_parts.message.id.clone(),
                with_parts
                    .message
                    .data
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                with_parts.message.sequence,
                with_parts.message.time_created,
                with_parts.message.data.clone(),
                with_parts
                    .parts
                    .into_iter()
                    .map(|part| (part.id, part.sequence, part.time_created, part.data))
                    .collect(),
            )
        })
        .collect();
    let projected: Vec<crate::v4_projection::ProjectedMessage<'_>> = loaded
        .iter()
        .map(
            |(id, role, sequence, time, data, parts)| crate::v4_projection::ProjectedMessage {
                id,
                role,
                sequence: *sequence,
                time_created: *time,
                data,
                parts,
            },
        )
        .collect();
    let rows = crate::v4_projection::project_rows(&projected);
    let (seq, revision, running) = match server.find_runtime(session_id) {
        Some(runtime) => (runtime.event_seq(), runtime.state_revision(), true),
        None => (0, 0, false),
    };
    let snapshot = crate::v4_projection::build_snapshot(crate::v4_projection::SnapshotStateFacts {
        session_id,
        log_epoch,
        seq,
        revision,
        title: "",
        title_source: "default",
        mode: "build",
        provider: "",
        model: "",
        running,
        goal: None,
        subagents: agent_protocol::v4_snapshot::SubagentProjectionState {
            revision: 1,
            child_session_ids: Vec::new(),
            running: Vec::new(),
            ended_total: 0,
        },
        workflow_runs: None,
        rows,
        context_window: None,
    });
    Some((snapshot, seq))
}

/// C4 T4c：v4 live 转发。conversation 先发初始快照帧，随后把会话事件映射为
/// 行/状态增量帧；sessions-index 只发初始快照帧（live 增量未接，登记）。
async fn run_v4_forward(
    server: Arc<AgentServer>,
    forward: crate::dispatch::PendingV4Forward,
    outbound: mpsc::UnboundedSender<Frame>,
) {
    use crate::dispatch::PendingV4Forward;
    fn v4_notification(method: &str, params: Value) -> Frame {
        Frame::Notification(NotificationFrame {
            method: method.to_string(),
            params: Some(params),
            trace: None,
        })
    }
    match forward {
        PendingV4Forward::ConversationRecovery {
            session_id,
            topic,
            subscription_id,
            log_epoch,
            from_seq,
        } => {
            // resync 的恢复帧：同一 subscription 上补一帧 deliveryKind:"recovery" 的全量
            // 快照；现有 live 转发循环继续服务后续 online 帧。
            let Some((snapshot, seq)) = conversation_snapshot(&server, &session_id, &log_epoch)
            else {
                return;
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as u64)
                .unwrap_or(0);
            let frame = agent_protocol::v4_transport::CompleteWireFrame {
                wire_version: V4_WIRE_PROTOCOL_VERSION,
                kind: agent_protocol::v4_transport::WireFrameKindComplete::Complete,
                delivery_kind: agent_protocol::v4_transport::WireDeliveryKind::Recovery,
                logical_frame_id: format!("{subscription_id}-rf-1"),
                logical_frame_ordinal: 1,
                topic: topic.clone(),
                subscription_id: subscription_id.clone(),
                frame: ConversationTopicFrame {
                    topic: topic.clone(),
                    subscription_id: subscription_id.clone(),
                    from_seq: Number::from(from_seq),
                    to_seq: Number::from(seq),
                    sent_at: Number::from(now),
                    payload: ConversationFramePayload::Snapshot(Box::new(SnapshotFramePayload {
                        kind: agent_protocol::v4_transport::FramePayloadKindSnapshot::Snapshot,
                        snapshot,
                    })),
                    ttft: None,
                    ttft_related: None,
                },
            };
            if let Ok(params) = serde_json::to_value(&frame) {
                let _ = outbound.send(v4_notification(
                    agent_protocol::v4_transport::V4_CONVERSATION_FRAME_NOTIFICATION,
                    params,
                ));
            }
        }
        PendingV4Forward::SessionsIndex {
            topic,
            subscription_id,
            log_epoch: _,
            snapshot,
        } => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as u64)
                .unwrap_or(0);
            let frame = agent_protocol::v4_sessions_index::SessionsIndexCompleteWireFrame {
                wire_version: V4_WIRE_PROTOCOL_VERSION,
                kind: agent_protocol::v4_transport::WireFrameKindComplete::Complete,
                delivery_kind: agent_protocol::v4_transport::WireDeliveryKind::Initial,
                logical_frame_id: format!("{subscription_id}-lf-1"),
                logical_frame_ordinal: 1,
                topic: topic.clone(),
                subscription_id: subscription_id.clone(),
                frame: agent_protocol::v4_sessions_index::SessionsIndexTopicFrame {
                    topic,
                    subscription_id,
                    from_seq: Number::from(0u64),
                    to_seq: Number::from(0u64),
                    sent_at: Number::from(now),
                    payload: agent_protocol::v4_sessions_index::SessionsIndexFramePayload::Snapshot(
                        Box::new(
                            agent_protocol::v4_sessions_index::SessionsIndexSnapshotPayload {
                                kind:
                                    agent_protocol::v4_transport::FramePayloadKindSnapshot::Snapshot,
                                snapshot,
                            },
                        ),
                    ),
                },
            };
            if let Ok(params) = serde_json::to_value(&frame) {
                let _ = outbound.send(v4_notification(
                    agent_protocol::v4_transport::V4_CONVERSATION_FRAME_NOTIFICATION,
                    params,
                ));
            }
        }
        PendingV4Forward::Conversation {
            session_id,
            topic,
            subscription_id,
            client_mode: _,
            log_epoch,
            last_seq,
        } => {
            // 初始快照帧：从 store/runtime 现状投影（会话行缺省标题等事实随 delta 修）。
            let Some((snapshot, seq)) = conversation_snapshot(&server, &session_id, &log_epoch)
            else {
                return;
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_millis() as u64)
                .unwrap_or(0);
            let initial = agent_protocol::v4_transport::CompleteWireFrame {
                wire_version: V4_WIRE_PROTOCOL_VERSION,
                kind: agent_protocol::v4_transport::WireFrameKindComplete::Complete,
                delivery_kind: agent_protocol::v4_transport::WireDeliveryKind::Initial,
                logical_frame_id: format!("{subscription_id}-lf-1"),
                logical_frame_ordinal: 1,
                topic: topic.clone(),
                subscription_id: subscription_id.clone(),
                frame: ConversationTopicFrame {
                    topic: topic.clone(),
                    subscription_id: subscription_id.clone(),
                    from_seq: Number::from(last_seq),
                    to_seq: Number::from(seq),
                    sent_at: Number::from(now),
                    payload: ConversationFramePayload::Snapshot(Box::new(SnapshotFramePayload {
                        kind: agent_protocol::v4_transport::FramePayloadKindSnapshot::Snapshot,
                        snapshot,
                    })),
                    ttft: None,
                    ttft_related: None,
                },
            };
            if let Ok(params) = serde_json::to_value(&initial) {
                let _ = outbound.send(v4_notification(
                    agent_protocol::v4_transport::V4_CONVERSATION_FRAME_NOTIFICATION,
                    params,
                ));
            }
            // 增量：broadcast 事件 → deltas 帧（Lagged 跳过、Closed 退出）。
            let (_, mut receiver) = match server.find_runtime(&session_id) {
                Some(runtime) => runtime.subscribe(None),
                None => return,
            };
            let mut live = crate::v4_projection::LiveDeltaState::default();
            let mut ordinal: u64 = 1;
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        let deltas = crate::v4_projection::event_to_deltas(&event, &mut live);
                        if deltas.is_empty() {
                            continue;
                        }
                        ordinal += 1;
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|elapsed| elapsed.as_millis() as u64)
                            .unwrap_or(0);
                        let frame = agent_protocol::v4_transport::CompleteWireFrame {
                            wire_version: V4_WIRE_PROTOCOL_VERSION,
                            kind: agent_protocol::v4_transport::WireFrameKindComplete::Complete,
                            delivery_kind: agent_protocol::v4_transport::WireDeliveryKind::Online,
                            logical_frame_id: format!("{subscription_id}-lf-{ordinal}"),
                            logical_frame_ordinal: ordinal,
                            topic: topic.clone(),
                            subscription_id: subscription_id.clone(),
                            frame: ConversationTopicFrame {
                                topic: topic.clone(),
                                subscription_id: subscription_id.clone(),
                                from_seq: Number::from(event.seq),
                                to_seq: Number::from(event.seq),
                                sent_at: Number::from(now),
                                payload: ConversationFramePayload::Deltas(
                                    agent_protocol::v4_transport::DeltasFramePayload {
                                        kind: agent_protocol::v4_transport::FramePayloadKindDeltas::Deltas,
                                        deltas,
                                    },
                                ),
                                ttft: None,
                                ttft_related: None,
                            },
                        };
                        if let Ok(params) = serde_json::to_value(&frame) {
                            let _ = outbound.send(v4_notification(
                                agent_protocol::v4_transport::V4_CONVERSATION_FRAME_NOTIFICATION,
                                params,
                            ));
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}

/// `session/subscribe` 的 live 转发：broadcast 事件 → `session/event` 通知帧。
///
/// `afterSeq` 之前的迟到事件不转发（replay 已带）；广播 Lagged 时跳过被丢弃的
/// 通知继续消费（`docs/specs/session-write-group.md` §2c 的口径）；runtime 关闭
/// （广播通道 Closed）或出站通道失败（连接关闭）时退出。
async fn run_live_forward(forward: PendingLiveForward, outbound: mpsc::UnboundedSender<Frame>) {
    let PendingLiveForward {
        after_seq,
        mut receiver,
    } = forward;
    loop {
        match receiver.recv().await {
            Ok(event) => {
                if let Some(after) = after_seq {
                    if event.seq <= after {
                        continue;
                    }
                }
                let Ok(params) = serde_json::to_value(&event) else {
                    continue;
                };
                let frame = Frame::Notification(NotificationFrame {
                    method: "session/event".to_string(),
                    params: Some(params),
                    trace: None,
                });
                if outbound.send(frame).is_err() {
                    return;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

pub(crate) async fn write_frame<W>(writer: &mut W, frame: &Frame) -> Result<(), ServerError>
where
    W: AsyncWrite + Unpin,
{
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|error| ServerError::internal(format!("serialize frame failed: {error}")))?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await.map_err(ServerError::Io)?;
    writer.flush().await.map_err(ServerError::Io)?;
    Ok(())
}
