//! 连接生命周期（spec §6.1/6.2/6.3/6.7；SDK :272-541 的对齐）：
//! 建连 → 认证 → 心跳 → 入站分派 → 断开重连（两套计数器）。

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::frame::{
    classify, generate_req_id, outbound_frame, Frame, Inbound, CMD_HEARTBEAT, CMD_SUBSCRIBE,
};
use crate::reply::{OutboundTx, ReplyBroker};

pub const DEFAULT_WS_URL: &str = "wss://openws.work.weixin.qq.com";
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
pub const DEFAULT_RECONNECT_BASE: Duration = Duration::from_secs(1);
pub const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);
pub const DEFAULT_MAX_RECONNECT_ATTEMPTS: i64 = 10;
pub const DEFAULT_MAX_AUTH_FAILURE_ATTEMPTS: i64 = 5;
pub const MAX_MISSED_PONG: u32 = 2;

#[derive(Debug, Clone)]
pub struct WeComConfig {
    pub bot_id: String,
    pub secret: String,
    pub ws_url: String,
    /// 认证帧 body 的额外参数（SDK `scene` / `plug_version`）。
    pub scene: Option<i64>,
    pub plug_version: Option<String>,
    pub heartbeat_interval: Duration,
    pub reconnect_base: Duration,
    pub max_reconnect_attempts: i64,
    pub max_auth_failure_attempts: i64,
}

impl WeComConfig {
    pub fn new(bot_id: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            bot_id: bot_id.into(),
            secret: secret.into(),
            ws_url: DEFAULT_WS_URL.to_string(),
            scene: None,
            plug_version: None,
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
            reconnect_base: DEFAULT_RECONNECT_BASE,
            max_reconnect_attempts: DEFAULT_MAX_RECONNECT_ATTEMPTS,
            max_auth_failure_attempts: DEFAULT_MAX_AUTH_FAILURE_ATTEMPTS,
        }
    }
}

/// 状态面：对齐 SDK 的 connected/authenticated/reconnecting/error 回调（:941-963）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusUpdate {
    Connected,
    Authenticated,
    Reconnecting(i64),
    AuthFailed(String),
    ServerDisconnected,
    Error(String),
}

#[derive(Clone)]
pub struct ConnectionCallbacks {
    pub on_frame: Arc<dyn Fn(Frame) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync>,
    pub on_status: Arc<dyn Fn(StatusUpdate) + Send + Sync>,
}

/// 运行结局（手动 disconnect 或重试耗尽）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    ServerDisconnected,
    Exhausted(String),
    Shutdown,
}

/// 启动连接任务：返回 (broker, 任务句柄)。任务在 `shutdown` 置真时结束。
pub fn spawn_connection(
    config: Arc<WeComConfig>,
    callbacks: ConnectionCallbacks,
    shutdown: watch::Receiver<bool>,
) -> (ReplyBroker, tokio::task::JoinHandle<RunOutcome>, OutboundTx) {
    let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
    let broker = ReplyBroker::new(outbound_tx.clone());
    let task = tokio::spawn(run_connection(
        config,
        broker.clone(),
        outbound_rx,
        callbacks,
        shutdown,
    ));
    (broker, task, outbound_tx)
}

async fn run_connection(
    config: Arc<WeComConfig>,
    broker: ReplyBroker,
    mut outbound: mpsc::UnboundedReceiver<Frame>,
    callbacks: ConnectionCallbacks,
    mut shutdown: watch::Receiver<bool>,
) -> RunOutcome {
    let mut reconnect_attempts: i64 = 0;
    let mut auth_failures: i64 = 0;
    loop {
        if *shutdown.borrow_and_update() {
            return RunOutcome::Shutdown;
        }
        let (end, was_authenticated) =
            run_session(&config, &broker, &mut outbound, &callbacks, &mut shutdown).await;
        broker.clear("WebSocket connection closed");
        if *shutdown.borrow_and_update() {
            return RunOutcome::Shutdown;
        }
        match end {
            SessionEnd::ServerDisconnected => {
                (callbacks.on_status)(StatusUpdate::ServerDisconnected);
                return RunOutcome::ServerDisconnected;
            }
            SessionEnd::Shutdown => return RunOutcome::Shutdown,
            SessionEnd::AuthFailed(reason) => {
                (callbacks.on_status)(StatusUpdate::AuthFailed(reason));
                if was_authenticated {
                    // 认证成功过再失败：两个计数器都从 0 起（SDK :421-426）。
                    reconnect_attempts = 0;
                    auth_failures = 0;
                }
                auth_failures += 1;
                if config.max_auth_failure_attempts >= 0
                    && auth_failures > config.max_auth_failure_attempts
                {
                    return RunOutcome::Exhausted(format!(
                        "Max auth failure attempts exceeded ({})",
                        config.max_auth_failure_attempts
                    ));
                }
                let delay = backoff(config.reconnect_base, auth_failures);
                (callbacks.on_status)(StatusUpdate::Reconnecting(auth_failures));
                if !sleep_or_shutdown(&mut shutdown, delay).await {
                    return RunOutcome::Shutdown;
                }
            }
            SessionEnd::Dropped => {
                if was_authenticated {
                    reconnect_attempts = 0;
                }
                reconnect_attempts += 1;
                if config.max_reconnect_attempts >= 0
                    && reconnect_attempts > config.max_reconnect_attempts
                {
                    return RunOutcome::Exhausted(format!(
                        "Max reconnect attempts exceeded ({})",
                        config.max_reconnect_attempts
                    ));
                }
                let delay = backoff(config.reconnect_base, reconnect_attempts);
                (callbacks.on_status)(StatusUpdate::Reconnecting(reconnect_attempts));
                if !sleep_or_shutdown(&mut shutdown, delay).await {
                    return RunOutcome::Shutdown;
                }
            }
        }
    }
}

/// 指数退避：`base × 2^(attempts-1)`，上限 30s（SDK :513, :531）。
fn backoff(base: Duration, attempts: i64) -> Duration {
    let exponent = attempts.saturating_sub(1).clamp(0, 30) as u32;
    let factor = 2u32.saturating_pow(exponent);
    base.saturating_mul(factor).min(RECONNECT_MAX_DELAY)
}

enum SessionEnd {
    Dropped,
    AuthFailed(String),
    ServerDisconnected,
    Shutdown,
}

async fn run_session(
    config: &WeComConfig,
    broker: &ReplyBroker,
    outbound: &mut mpsc::UnboundedReceiver<Frame>,
    callbacks: &ConnectionCallbacks,
    shutdown: &mut watch::Receiver<bool>,
) -> (SessionEnd, bool) {
    let socket = match connect_async(&config.ws_url).await {
        Ok((socket, _response)) => socket,
        Err(error) => {
            log_line(&format!("wecom WS dial failed: {error}"));
            return (SessionEnd::Dropped, false);
        }
    };
    (callbacks.on_status)(StatusUpdate::Connected);
    let (mut sink, mut stream) = socket.split();

    // 认证帧（SDK :345-357, :928-931）。
    let mut auth_body = json!({
        "bot_id": config.bot_id,
        "secret": config.secret,
    });
    if let Some(scene) = config.scene {
        auth_body["scene"] = json!(scene);
    }
    if let Some(version) = &config.plug_version {
        auth_body["plug_version"] = json!(version);
    }
    let auth_frame = outbound_frame(
        CMD_SUBSCRIBE,
        &generate_req_id(CMD_SUBSCRIBE, now_ms()),
        auth_body,
    );
    if send_frame(&mut sink, &auth_frame).await.is_err() {
        return (SessionEnd::Dropped, false);
    }

    let mut authenticated = false;
    let mut missed_pongs: u32 = 0;
    let mut heartbeat = tokio::time::interval(config.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    return (SessionEnd::Shutdown, authenticated);
                }
            }
            frame = outbound.recv() => {
                let Some(frame) = frame else {
                    return (SessionEnd::Shutdown, authenticated);
                };
                if send_frame(&mut sink, &frame).await.is_err() {
                    return (SessionEnd::Dropped, authenticated);
                }
            }
            _ = heartbeat.tick() => {
                // 认证前不心跳（SDK 认证成功后才 startHeartbeat，:425, :449-454）。
                if !authenticated {
                    continue;
                }
                if missed_pongs >= MAX_MISSED_PONG {
                    // 连续未获 pong → 判定死连接（SDK :475-483）。
                    log_line("wecom heartbeat: no pong, treating connection as dead");
                    return (SessionEnd::Dropped, authenticated);
                }
                missed_pongs += 1;
                let ping = outbound_frame(
                    CMD_HEARTBEAT,
                    &generate_req_id(CMD_HEARTBEAT, now_ms()),
                    serde_json::Value::Null,
                );
                if send_frame(&mut sink, &ping).await.is_err() {
                    return (SessionEnd::Dropped, authenticated);
                }
            }
            incoming = stream.next() => {
                let message = match incoming {
                    Some(Ok(message)) => message,
                    Some(Err(error)) => {
                        log_line(&format!("wecom WS error: {error}"));
                        return (SessionEnd::Dropped, authenticated);
                    }
                    None => return (SessionEnd::Dropped, authenticated),
                };
                match message {
                    Message::Text(text) => {
                        // SDK 先剥控制字符再 JSON.parse（:314）。
                        let cleaned: String = text
                            .chars()
                            .filter(|c| {
                                !matches!(*c, '\u{0000}'..='\u{0008}' | '\u{000B}'..='\u{000D}' | '\u{000E}'..='\u{001F}')
                            })
                            .collect();
                        let frame: Frame = match serde_json::from_str(&cleaned) {
                            Ok(frame) => frame,
                            Err(error) => {
                                log_line(&format!("wecom frame parse failed: {error}"));
                                continue;
                            }
                        };
                        match classify(&frame, |req_id| broker.has_pending(req_id)) {
                            Inbound::AuthAck { errcode, errmsg } => {
                                if errcode != 0 {
                                    return (SessionEnd::AuthFailed(format!(
                                        "Authentication failed: {errmsg} (code: {errcode})"
                                    )), false);
                                }
                                authenticated = true;
                                missed_pongs = 0;
                                (callbacks.on_status)(StatusUpdate::Authenticated);
                            }
                            Inbound::HeartbeatAck { errcode, .. } => {
                                if errcode == 0 {
                                    missed_pongs = 0;
                                }
                            }
                            Inbound::ReplyAck { req_id, .. } => {
                                broker.handle_ack(&req_id, frame);
                            }
                            Inbound::Message { .. } | Inbound::Event { .. } => {
                                let callbacks = callbacks.clone();
                                let frame = frame.clone();
                                tokio::spawn(async move {
                                    (callbacks.on_frame)(frame).await;
                                });
                            }
                            Inbound::ServerDisconnected { .. } => {
                                let callbacks = callbacks.clone();
                                let frame = frame.clone();
                                tokio::spawn(async move {
                                    (callbacks.on_frame)(frame).await;
                                });
                                return (SessionEnd::ServerDisconnected, authenticated);
                            }
                            Inbound::Unknown => {
                                log_line("wecom unknown frame ignored");
                            }
                        }
                    }
                    Message::Close(_) => return (SessionEnd::Dropped, authenticated),
                    Message::Ping(payload) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                    }
                    _ => {}
                }
            }
        }
    }
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

async fn send_frame(sink: &mut WsSink, frame: &Frame) -> Result<(), String> {
    let text = serde_json::to_string(frame).map_err(|error| error.to_string())?;
    sink.send(Message::Text(text.into()))
        .await
        .map_err(|error| error.to_string())
}

async fn sleep_or_shutdown(shutdown: &mut watch::Receiver<bool>, duration: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => true,
        changed = shutdown.changed() => {
            if changed.is_err() {
                return false;
            }
            !(*shutdown.borrow_and_update())
        }
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

/// 诊断出口：库层默认 stderr，宿主可用 [`crate::set_log_hook`] 接结构化日志。
type LogHook = Arc<dyn Fn(&str) + Send + Sync>;
static LOG_HOOK: std::sync::Mutex<Option<LogHook>> = std::sync::Mutex::new(None);

pub fn set_log_hook(hook: LogHook) {
    if let Ok(mut guard) = LOG_HOOK.lock() {
        *guard = Some(hook);
    }
}

pub(crate) fn log_line(message: &str) {
    let hook = LOG_HOOK.lock().ok().and_then(|guard| guard.clone());
    match hook {
        Some(hook) => hook(message),
        None => eprintln!("[wecom-ws] {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps_at_30s() {
        assert_eq!(backoff(Duration::from_secs(1), 1), Duration::from_secs(1));
        assert_eq!(backoff(Duration::from_secs(1), 2), Duration::from_secs(2));
        assert_eq!(backoff(Duration::from_secs(1), 3), Duration::from_secs(4));
        assert_eq!(backoff(Duration::from_secs(1), 10), Duration::from_secs(30));
        assert_eq!(backoff(Duration::from_secs(1), 40), Duration::from_secs(30));
    }
}
