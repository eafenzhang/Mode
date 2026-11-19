//! 单 bot 连接生命周期循环（TS `runConnection`，dingtalkChannelRuntime.ts:106-314 的对齐）。
//!
//! 外层循环：注册 → WS 会话 → 结束后按健康度重置/加倍退避；注册失败走固定 5s 重试
//! （不走指数退避）。会话内：读超时判死（120s 无任何帧）、[`crate::frame::handle_frame`]
//! 产出决策、回帧立即写、派发 fire-and-forget、去重持久化 500ms 防抖。
//! 所有外部作用（注册/状态/派发/持久化）都是注入的 boxed 回调，测试用本地假服务离线对拍。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::watch;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::dedupe::{DedupeEntry, DedupeSet, MESSAGE_DEDUPE_LIMIT, MESSAGE_DEDUPE_TTL_MS};
use crate::frame::{handle_frame, DownFrame};
use crate::open::{OpenError, OpenRequest, Registration, StreamOpen};

/// TS :29-36 的窗口常量（读超时/退避/健康线/注册重试/持久化防抖）。
pub const WS_READ_TIMEOUT_MS: u64 = 120_000;
pub const RECONNECT_BACKOFF_MIN_MS: u64 = 1_000;
pub const RECONNECT_BACKOFF_MAX_MS: u64 = 60_000;
pub const HEALTHY_CONNECTION_MS: u64 = 30_000;
pub const REGISTRATION_RETRY_MS: u64 = 5_000;
pub const DEDUPE_PERSIST_DEBOUNCE_MS: u64 = 500;

#[derive(Debug, Clone)]
pub struct StreamTiming {
    pub read_timeout: Duration,
    pub registration_retry: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    pub healthy_after: Duration,
    pub persist_debounce: Duration,
}

impl Default for StreamTiming {
    fn default() -> Self {
        Self {
            read_timeout: Duration::from_millis(WS_READ_TIMEOUT_MS),
            registration_retry: Duration::from_millis(REGISTRATION_RETRY_MS),
            backoff_min: Duration::from_millis(RECONNECT_BACKOFF_MIN_MS),
            backoff_max: Duration::from_millis(RECONNECT_BACKOFF_MAX_MS),
            healthy_after: Duration::from_millis(HEALTHY_CONNECTION_MS),
            persist_debounce: Duration::from_millis(DEDUPE_PERSIST_DEBOUNCE_MS),
        }
    }
}

/// 重连退避：健康连接（活过 `healthy_after`）重置到下限，否则指数加倍封顶
/// （TS :300-304）。首连失败从下限翻倍——与 TS 初始值语义一致。
pub fn next_backoff_ms(
    connected_ms: u64,
    current_ms: u64,
    healthy_after_ms: u64,
    min_ms: u64,
    max_ms: u64,
) -> u64 {
    if connected_ms > healthy_after_ms {
        min_ms
    } else {
        current_ms.saturating_mul(2).clamp(min_ms, max_ms)
    }
}

/// 状态面：对齐 TS `statusSink.setRuntimeStatus` 的状态与文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStatus {
    Connecting,
    Connected,
    Reconnecting,
    Error,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusUpdate {
    pub bot_id: String,
    pub provider: &'static str,
    pub status: RuntimeStatus,
    pub message: &'static str,
}

impl StatusUpdate {
    fn new(bot_id: &str, status: RuntimeStatus, message: &'static str) -> Self {
        Self {
            bot_id: bot_id.to_string(),
            provider: "dingtalk",
            status,
            message,
        }
    }
}

type BoxFuture<T> = futures_util::future::BoxFuture<'static, T>;

/// 注入面：全部 boxed 回调，生产实现见 [`HttpRuntimeCallbacks`]。
#[derive(Clone)]
pub struct RuntimeCallbacks {
    /// Stream 注册（生产 = `HttpStreamOpen`；测试 = 本地假网关）。
    pub open: Arc<dyn Fn(OpenRequest) -> BoxFuture<Result<Registration, OpenError>> + Send + Sync>,
    pub on_status: Arc<dyn Fn(StatusUpdate) + Send + Sync>,
    /// 派发回调：fire-and-forget（TS :220-229 的 `void … catch warn`）。
    pub on_callback: Arc<dyn Fn(String, Value) -> BoxFuture<()> + Send + Sync>,
    /// 去重持久化（TS `writeDingtalkDedup`，:128-135）。
    pub persist_dedupe: Arc<dyn Fn(String, Vec<DedupeEntry>) -> BoxFuture<()> + Send + Sync>,
    /// 去重加载（TS `readDingtalkDedup`，:109；失败按空表处理）。
    pub load_dedupe: Arc<dyn Fn(String) -> BoxFuture<Vec<DedupeEntry>> + Send + Sync>,
}

impl RuntimeCallbacks {
    /// 生产装配：Stream 注册走 reqwest（ring provider 自装，见 [`crate::open::HttpStreamOpen`]）。
    pub fn with_http_open(
        on_status: Arc<dyn Fn(StatusUpdate) + Send + Sync>,
        on_callback: Arc<dyn Fn(String, Value) -> BoxFuture<()> + Send + Sync>,
        persist_dedupe: Arc<dyn Fn(String, Vec<DedupeEntry>) -> BoxFuture<()> + Send + Sync>,
        load_dedupe: Arc<dyn Fn(String) -> BoxFuture<Vec<DedupeEntry>> + Send + Sync>,
    ) -> Result<Self, OpenError> {
        let http = crate::open::HttpStreamOpen::new()?;
        Ok(Self {
            open: Arc::new(move |request| {
                let http = http.clone();
                Box::pin(async move { http.open(&request).await })
            }),
            on_status,
            on_callback,
            persist_dedupe,
            load_dedupe,
        })
    }
}

#[derive(Debug, Clone)]
pub struct BotConnection {
    pub bot_id: String,
    pub api_base: String,
    pub client_id: String,
    pub client_secret: String,
}

/// 连接主循环：直到 `shutdown` 置真才返回；网络错误永远重试。
pub async fn run_connection(
    connection: BotConnection,
    timing: StreamTiming,
    callbacks: RuntimeCallbacks,
    mut shutdown: watch::Receiver<bool>,
) {
    (callbacks.on_status)(StatusUpdate::new(
        &connection.bot_id,
        RuntimeStatus::Connecting,
        "DingTalk stream is connecting.",
    ));

    // 去重表：从磁盘恢复（跨重启 + 服务端重投窗口），失败按空表——TS :107-114。
    let loaded = (callbacks.load_dedupe)(connection.bot_id.clone()).await;
    let dedupe = Arc::new(Mutex::new(DedupeSet::from_entries(
        loaded,
        MESSAGE_DEDUPE_LIMIT,
        MESSAGE_DEDUPE_TTL_MS,
        now_ms(),
    )));

    let mut backoff_ms = timing.backoff_min.as_millis() as u64;
    loop {
        if *shutdown.borrow_and_update() {
            return;
        }
        let registration = match (callbacks.open)(OpenRequest {
            api_base: connection.api_base.clone(),
            client_id: connection.client_id.clone(),
            client_secret: connection.client_secret.clone(),
        })
        .await
        {
            Ok(registration)
                if !registration.endpoint.is_empty() && !registration.ticket.is_empty() =>
            {
                registration
            }
            Ok(_) | Err(_) => {
                (callbacks.on_status)(StatusUpdate::new(
                    &connection.bot_id,
                    RuntimeStatus::Error,
                    "DingTalk stream registration failed.",
                ));
                if !sleep_or_shutdown(&mut shutdown, timing.registration_retry).await {
                    return;
                }
                continue;
            }
        };
        if *shutdown.borrow_and_update() {
            return;
        }

        let connection_started = Instant::now();

        session(
            &registration,
            &connection,
            &timing,
            &callbacks,
            &mut shutdown,
            Arc::clone(&dedupe),
        )
        .await;

        if *shutdown.borrow_and_update() {
            return;
        }
        // 健康连接（活过 30s）重置退避；否则指数退避到 60s 上限——TS :300-304。
        backoff_ms = next_backoff_ms(
            connection_started.elapsed().as_millis() as u64,
            backoff_ms,
            timing.healthy_after.as_millis() as u64,
            timing.backoff_min.as_millis() as u64,
            timing.backoff_max.as_millis() as u64,
        );
        (callbacks.on_status)(StatusUpdate::new(
            &connection.bot_id,
            RuntimeStatus::Reconnecting,
            "DingTalk stream is reconnecting.",
        ));
        if !sleep_or_shutdown(&mut shutdown, Duration::from_millis(backoff_ms)).await {
            return;
        }
    }
}

/// 单次 WS 会话：建连 → 帧循环 → 结束（网络错误/读超时/服务端 disconnect/shutdown）。
async fn session(
    registration: &Registration,
    connection: &BotConnection,
    timing: &StreamTiming,
    callbacks: &RuntimeCallbacks,
    shutdown: &mut watch::Receiver<bool>,
    dedupe: Arc<Mutex<DedupeSet>>,
) {
    let url = format!("{}?ticket={}", registration.endpoint, registration.ticket);
    let socket = match connect_async(url).await {
        Ok((socket, _response)) => socket,
        Err(error) => {
            // TS 的 socket 'error' 只记日志、'close' 收口——等价为会话结束重连。
            // 「连接已建立」状态只在真正 open 之后上报（TS :276-284）。
            log_line(&format!(
                "dingtalk WS dial failed bot={}: {error}",
                connection.bot_id
            ));
            return;
        }
    };
    (callbacks.on_status)(StatusUpdate::new(
        &connection.bot_id,
        RuntimeStatus::Connected,
        "DingTalk stream is connected.",
    ));
    let (mut sink, mut stream) = socket.split();

    // 持久化防抖状态（每会话独立；TS 的 timer 挂在连接闭包上，:115-138）。
    let dirty = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let persist_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> =
        Arc::new(Mutex::new(None));

    loop {
        let incoming = tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    return;
                }
                continue;
            }
            message = tokio::time::timeout(timing.read_timeout, stream.next()) => {
                match message {
                    Err(_elapsed) => {
                        // 120s 无任何帧判死重连——TS :264-273。
                        log_line(&format!(
                            "dingtalk WS read timeout (no frame for {}s) bot={}",
                            timing.read_timeout.as_secs(),
                            connection.bot_id
                        ));
                        return;
                    }
                    Ok(None) => return,
                    Ok(Some(Err(error))) => {
                        log_line(&format!("dingtalk WS error bot={}: {error}", connection.bot_id));
                        return;
                    }
                    Ok(Some(Ok(message))) => message,
                }
            }
        };

        match incoming {
            Message::Text(text) => {
                let Some(down) = DownFrame::parse(&text) else {
                    continue;
                };
                let outcome = {
                    let mut guard = match dedupe.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    handle_frame(&down, &mut guard, now_ms())
                };
                if let Some(reply) = outcome.reply {
                    if let Err(error) = sink.send(Message::Text(reply.to_text().into())).await {
                        log_line(&format!(
                            "dingtalk WS send failed bot={}: {error}",
                            connection.bot_id
                        ));
                        return;
                    }
                }
                if let Some(dispatched) = outcome.dispatch {
                    schedule_persist(
                        connection.bot_id.clone(),
                        Arc::clone(&dedupe),
                        Arc::clone(&dirty),
                        Arc::clone(&persist_handle),
                        timing.persist_debounce,
                        Arc::clone(&callbacks.persist_dedupe),
                    );
                    let bot_id = connection.bot_id.clone();
                    let raw = dispatched.raw;
                    let on_callback = callbacks.on_callback.clone();
                    // fire-and-forget：失败只在回调内部处理（TS :220-229）。
                    tokio::spawn(async move {
                        on_callback(bot_id, raw).await;
                    });
                }
                if outcome.close {
                    return;
                }
            }
            Message::Ping(payload) => {
                // tungstenite 不自动回 Pong：协议层必须显式应答。
                let _ = sink.send(Message::Pong(payload)).await;
            }
            Message::Close(_) => return,
            _ => {}
        }
    }
}

/// 去重持久化防抖：脏标记置位后，若无在飞任务则起一个 500ms 合并任务
/// （TS :115-138 的 `scheduleDedupePersist`；快照在任务内取，覆盖防抖窗内的后续写入）。
#[allow(clippy::too_many_arguments)]
fn schedule_persist(
    bot_id: String,
    dedupe: Arc<Mutex<DedupeSet>>,
    dirty: Arc<std::sync::atomic::AtomicBool>,
    persist_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    debounce: Duration,
    persist: Arc<dyn Fn(String, Vec<DedupeEntry>) -> BoxFuture<()> + Send + Sync>,
) {
    dirty.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut handle = match persist_handle.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if handle.is_some() {
        return;
    }
    *handle = Some(tokio::spawn(async move {
        tokio::time::sleep(debounce).await;
        let entries = {
            let mut guard = match dedupe.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.snapshot()
        };
        if !entries.is_empty() {
            persist(bot_id, entries).await;
        }
        dirty.store(false, std::sync::atomic::Ordering::Relaxed);
    }));
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// 可取消 sleep：返回 false 表示 shutdown 置真。
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

/// 诊断出口：TS 用 logger；库层默认 stderr，宿主可用 [`set_log_hook`] 接结构化日志。
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
        None => eprintln!("[dingtalk-stream] {message}"),
    }
}

/// 重连退避纯函数的单测（行为表见 spec §3 第 9 条）。
#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = RECONNECT_BACKOFF_MIN_MS;
    const MAX: u64 = RECONNECT_BACKOFF_MAX_MS;
    const HEALTHY: u64 = HEALTHY_CONNECTION_MS;

    #[test]
    fn healthy_connection_resets_backoff_to_min() {
        assert_eq!(next_backoff_ms(HEALTHY + 1, 32_000, HEALTHY, MIN, MAX), MIN);
    }

    #[test]
    fn unhealthy_connection_doubles_with_cap() {
        assert_eq!(next_backoff_ms(100, MIN, HEALTHY, MIN, MAX), 2 * MIN);
        let mut current = MIN;
        for _ in 0..10 {
            current = next_backoff_ms(100, current, HEALTHY, MIN, MAX);
        }
        assert_eq!(current, MAX, "指数退避封顶 60s");
        assert_eq!(next_backoff_ms(100, MAX, HEALTHY, MIN, MAX), MAX);
    }

    #[test]
    fn exactly_at_healthy_threshold_is_not_healthy() {
        // TS :301 的判定是严格大于（30s 活过才算健康）。
        assert_eq!(next_backoff_ms(HEALTHY, MIN, HEALTHY, MIN, MAX), 2 * MIN);
    }
}
