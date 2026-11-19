//! 单个 WS 会话（spec §5.3；SDK `communicate/pingLoop/handleControlData/handleEventData`
//! :85483-85585 的对齐）：ping 循环、control/data 分派、分片重组派发、ACK 回帧。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::cache::{DataCache, CACHE_EXPIRY};
use crate::gateway::SessionParams;
use crate::pb::{self, Frame, Header};

/// 出站半连接（WS 二进制帧）。
type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

pub const METHOD_CONTROL: i32 = 0;
pub const METHOD_DATA: i32 = 1;
pub const TYPE_EVENT: &str = "event";
pub const TYPE_PING: &str = "ping";
pub const TYPE_PONG: &str = "pong";
pub const HEADER_TYPE: &str = "type";
pub const HEADER_MESSAGE_ID: &str = "message_id";
pub const HEADER_SUM: &str = "sum";
pub const HEADER_SEQ: &str = "seq";
pub const HEADER_BIZ_RT: &str = "biz_rt";

pub type BoxFuture<T> = futures_util::future::BoxFuture<'static, T>;

/// 事件派发：入参为分片合并后的 JSON；成功返回可选结果（回给服务端的 ACK data，
/// base64 编码后下发），失败记 code=500（SDK :85550-85585）。
pub type EventDispatcher =
    Arc<dyn Fn(Value) -> BoxFuture<Result<Option<Value>, String>> + Send + Sync>;

#[derive(Clone)]
pub struct SessionCallbacks {
    pub on_event: EventDispatcher,
}

#[derive(Debug, Clone)]
pub struct SessionTiming {
    /// 分片过期清理周期（SDK 的 10s magic number）。
    pub cache_sweep: Duration,
    pub cache_expiry: Duration,
}

impl Default for SessionTiming {
    fn default() -> Self {
        Self {
            cache_sweep: CACHE_EXPIRY,
            cache_expiry: CACHE_EXPIRY,
        }
    }
}

/// 会话结局：建连成功（含随后因关闭/错误结束） vs 建连失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutcome {
    Established,
    ConnectFailed,
}

/// 一次会话：建连 → 循环 → 结束（WS 关闭/错误/shutdown）。参数被 pong 更新
/// （ping 间隔等），由 runtime 在重连时读取最新值。
pub async fn run_session(
    params: Arc<Mutex<SessionParams>>,
    callbacks: SessionCallbacks,
    timing: &SessionTiming,
    mut shutdown: watch::Receiver<bool>,
) -> SessionOutcome {
    let (connect_url, service_id) = {
        let guard = params
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (guard.connect_url.clone(), guard.service_id.clone())
    };
    let socket = match connect_async(&connect_url).await {
        Ok((socket, _response)) => socket,
        Err(error) => {
            log_line(&format!("feishu WS dial failed: {error}"));
            return SessionOutcome::ConnectFailed;
        }
    };
    let (mut sink, mut stream) = socket.split();
    let cache = Arc::new(Mutex::new(DataCache::new()));
    let service: i32 = service_id.parse().unwrap_or_default();

    // 首个 ping 在建连后立即发（SDK open 时调用 pingLoop，:85381-85385）。
    send_frame(&mut sink, &ping_frame(service)).await;
    let mut ping_delay = current_ping_interval(&params);
    let mut ping_sleep = Box::pin(tokio::time::sleep(ping_delay));
    let mut sweep = tokio::time::interval(timing.cache_sweep);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    return SessionOutcome::Established;
                }
            }
            _ = &mut ping_sleep => {
                send_frame(&mut sink, &ping_frame(service)).await;
                ping_delay = current_ping_interval(&params);
                ping_sleep = Box::pin(tokio::time::sleep(ping_delay));
            }
            _ = sweep.tick() => {
                let expired = {
                    let mut guard = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard.expire(Instant::now(), timing.cache_expiry)
                };
                if expired > 0 {
                    log_line(&format!("feishu event cache expired {expired} pending message(s)"));
                }
            }
            incoming = stream.next() => {
                let message = match incoming {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => {
                        log_line(&format!("feishu WS error: {error}"));
                        return SessionOutcome::Established;
                    }
                    None => return SessionOutcome::Established,
                };
                match message {
                    Message::Binary(bytes) => {
                        let frame = match pb::decode(&bytes) {
                            Ok(frame) => frame,
                            Err(error) => {
                                log_line(&format!("feishu WS frame decode failed: {error}"));
                                continue;
                            }
                        };
                        if frame.method == METHOD_CONTROL {
                            handle_control(&params, &frame);
                        } else if frame.method == METHOD_DATA {
                            handle_data(&mut sink, &cache, &callbacks, &frame).await;
                        }
                    }
                    Message::Close(_) => return SessionOutcome::Established,
                    Message::Ping(payload) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn current_ping_interval(params: &Arc<Mutex<SessionParams>>) -> Duration {
    params
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .ping_interval
}

fn ping_frame(service: i32) -> Frame {
    Frame {
        seq_id: 0,
        log_id: 0,
        service,
        method: METHOD_CONTROL,
        headers: vec![Header {
            key: HEADER_TYPE.into(),
            value: TYPE_PING.into(),
        }],
        payload_encoding: None,
        payload_type: None,
        payload: None,
        log_id_new: None,
    }
}

/// control 帧：ping 忽略；pong 用 payload 的 JSON 更新会话参数（spec §5.3 第 2 条）。
fn handle_control(params: &Arc<Mutex<SessionParams>>, frame: &Frame) {
    let Some(kind) = pb::header_value(frame, HEADER_TYPE) else {
        return;
    };
    if kind != TYPE_PONG {
        return;
    }
    let Some(payload) = &frame.payload else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return;
    };
    let seconds = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_f64)
            .map(Duration::from_secs_f64)
    };
    let mut guard = params
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(interval) = seconds("PingInterval") {
        guard.ping_interval = interval;
    }
    if let Some(interval) = seconds("ReconnectInterval") {
        guard.reconnect_interval = interval;
    }
    if let Some(nonce) = seconds("ReconnectNonce") {
        guard.reconnect_nonce = nonce;
    }
    if let Some(count) = value.get("ReconnectCount").and_then(Value::as_i64) {
        guard.reconnect_count = count;
    }
}

/// data 帧：仅 `type=event` 处理；按 `message_id/sum/seq` 分片重组后派发并回 ACK。
async fn handle_data(
    sink: &mut WsSink,
    cache: &Arc<Mutex<DataCache>>,
    callbacks: &SessionCallbacks,
    frame: &Frame,
) {
    let Some(kind) = pb::header_value(frame, HEADER_TYPE) else {
        return;
    };
    if kind != TYPE_EVENT {
        // MessageType.card 有定义但此路径不处理，也不回 ACK（SDK :85550-85556）。
        return;
    }
    let (Some(message_id), Some(sum), Some(seq)) = (
        pb::header_value(frame, HEADER_MESSAGE_ID),
        pb::header_value(frame, HEADER_SUM),
        pb::header_value(frame, HEADER_SEQ),
    ) else {
        log_line("feishu event frame missing message_id/sum/seq; dropped");
        return;
    };
    let (Ok(sum), Ok(seq)) = (sum.parse::<usize>(), seq.parse::<usize>()) else {
        log_line("feishu event frame has invalid sum/seq; dropped");
        return;
    };
    let Some(chunk) = &frame.payload else {
        return;
    };
    let merged = {
        let mut guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.merge(message_id, sum, seq, chunk.clone(), Instant::now())
    };
    let Some(event) = merged else {
        return;
    };

    // ACK：复制原帧，headers 追加 biz_rt，payload = {"code":200|500}[,"data":base64]。
    let start_ms = now_ms();
    let result = (callbacks.on_event)(event).await;
    let end_ms = now_ms();
    let mut ack = frame.clone();
    // SDK :85578 的 biz_rt 是 `startTime - endTime`（负值）——按真源保真。
    ack.headers.push(Header {
        key: HEADER_BIZ_RT.into(),
        value: (start_ms as i64 - end_ms as i64).to_string(),
    });
    let payload = match result {
        Ok(Some(value)) => {
            let encoded =
                base64_encode(serde_json::to_string(&value).unwrap_or_default().as_bytes());
            json!({ "code": 200, "data": encoded })
        }
        Ok(None) => json!({ "code": 200 }),
        Err(error) => {
            log_line(&format!("feishu event dispatch failed: {error}"));
            json!({ "code": 500 })
        }
    };
    ack.payload = Some(serde_json::to_vec(&payload).unwrap_or_else(|_| b"{\"code\":500}".to_vec()));
    send_frame(sink, &ack).await;
}

async fn send_frame(sink: &mut WsSink, frame: &Frame) {
    let bytes = pb::encode(frame);
    if let Err(error) = sink.send(Message::Binary(bytes.into())).await {
        log_line(&format!("feishu WS send failed: {error}"));
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// 诊断出口：库层默认 stderr，宿主可用 [`crate::set_log_hook`] 接结构化日志。
type LogHook = Arc<dyn Fn(&str) + Send + Sync>;
static LOG_HOOK: Mutex<Option<LogHook>> = Mutex::new(None);

pub fn set_log_hook(hook: LogHook) {
    if let Ok(mut guard) = LOG_HOOK.lock() {
        *guard = Some(hook);
    }
}

pub(crate) fn log_line(message: &str) {
    let hook = LOG_HOOK.lock().ok().and_then(|guard| guard.clone());
    match hook {
        Some(hook) => hook(message),
        None => eprintln!("[feishu-ws] {message}"),
    }
}
