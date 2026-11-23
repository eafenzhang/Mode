//! 被控会话状态机（`remote-desktop` 会话面的可自研部分）。
//!
//! 真源：`packages/desktop/src/host/remoteDesktopServer.ts` 的「会话（单会话互斥 +
//! 同意流）」一节。规范见 `docs/specs/remote-desktop-session.md`。
//!
//! 这里只搬**纯状态机**：准入判定、同意等待、等待 WS 接线、TTL 过期、被控态通知。
//! 真实媒体/信令链路不在本模块（那是 transport 的事），所以状态机不持有 socket、
//! 不 spawn 任务、不读环境变量——时间也由调用方注入。
//!
//! 四态与一条不变量：
//!
//! ```text
//! Idle ──request_session(允许)──► PendingConsent{client} ──allow──► PendingSocket{client}
//!   ▲                                     │                              │
//!   │                                     └──deny/timeout(75s)──┐        │
//!   └──────────── pending socket TTL(35s) / stop_sessions ───────┴────────┘
//!                                                    Active{client}（媒体接线后）
//! ```
//!
//! **不变量**：任意时刻至多一个被控会话占用；`controlled` 仅在三个占用态之一时为 true，
//! 且只在**真的变化**时通知订阅者（TS `notifyControlled` 的 `lastControlled` 比较）。

use std::sync::{Arc, Mutex};

use rpc_host::{EventCallback, Subscription};
use rpc_wire::Value as WireValue;
use serde_json::{json, Value as Json};

/// 同意等待上限：产品规则 2 的 60s 由 main 的同意窗执行，这里是 main 失约时的兜底。
pub const CONSENT_SAFETY_TIMEOUT_MS: i64 = 75_000;
/// 同意通过后等待 WS 接线的上限（票据 TTL 同量级）；过期即释放单会话占用。
pub const PENDING_SOCKET_TTL_MS: i64 = 35_000;

/// 准入判定的结局（TS `evaluateRemoteDesktopRequest`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// 进入同意流。
    Consent,
    /// 直接拒绝（`disabled` / `busy`）。
    Reject(&'static str),
}

/// 会话占用态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlledSession {
    Idle,
    /// 已弹同意窗，等 main 回帖。
    PendingConsent {
        client_id: String,
        deadline_ms: i64,
    },
    /// 已同意，等该 client 的 WS 接线。
    PendingSocket {
        client_id: String,
        deadline_ms: i64,
    },
    /// 媒体已接线，会话进行中。
    Active {
        client_id: String,
    },
}

impl ControlledSession {
    pub fn client_id(&self) -> Option<&str> {
        match self {
            Self::Idle => None,
            Self::PendingConsent { client_id, .. }
            | Self::PendingSocket { client_id, .. }
            | Self::Active { client_id } => Some(client_id),
        }
    }

    /// 是否占用（TS `pendingConsent || pendingSocket || activeSession`）。
    pub fn is_occupied(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::PendingConsent { .. } => "pending_consent",
            Self::PendingSocket { .. } => "pending_socket",
            Self::Active { .. } => "active",
        }
    }
}

/// 同意结果（TS `settleConsent` 的两种结局）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentResult {
    Allow,
    Deny(&'static str),
}

type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;
type Listener = Arc<dyn Fn(bool) + Send + Sync>;

struct Inner {
    session: ControlledSession,
    listeners: Vec<(String, Listener)>,
    last_controlled: bool,
    /// 准入用的两个外部事实（由端口在每次请求时提供）。
    kill_switch_engaged: bool,
    server_enabled: bool,
    disposed: bool,
}

pub struct ControlledSessionMachine {
    inner: Mutex<Inner>,
    now_ms: Clock,
}

impl ControlledSessionMachine {
    pub fn new(now_ms: Clock) -> Self {
        Self {
            inner: Mutex::new(Inner {
                session: ControlledSession::Idle,
                listeners: Vec::new(),
                last_controlled: false,
                kill_switch_engaged: false,
                server_enabled: false,
                disposed: false,
            }),
            now_ms,
        }
    }

    /// 更新准入事实（总开关变化时由端口调用）。
    pub fn set_admission_facts(&self, kill_switch_engaged: bool, server_enabled: bool) {
        let mut inner = self.inner.lock().expect("session machine poisoned");
        inner.kill_switch_engaged = kill_switch_engaged;
        inner.server_enabled = server_enabled;
    }

    /// 当前会话态（诊断与测试用）。
    pub fn session(&self) -> ControlledSession {
        self.inner
            .lock()
            .expect("session machine poisoned")
            .session
            .clone()
    }

    /// 被控态：三个占用态之一。
    pub fn is_controlled(&self) -> bool {
        self.inner
            .lock()
            .expect("session machine poisoned")
            .session
            .is_occupied()
    }

    /// 准入判定（TS `evaluateRemoteDesktopRequest` + 单会话互斥）。
    ///
    /// 顺序与 TS 一致：kill-switch 压死整链 → 总开关关 → 占用中 busy → 同意流。
    pub fn evaluate(&self) -> Admission {
        let inner = self.inner.lock().expect("session machine poisoned");
        if inner.disposed || inner.kill_switch_engaged || !inner.server_enabled {
            return Admission::Reject("disabled");
        }
        if inner.session.is_occupied() {
            return Admission::Reject("busy");
        }
        Admission::Consent
    }

    /// 受理一次会话请求（LAN server 的 hook 调到这里）。
    ///
    /// 返回 `Ok(deadline_ms)` 表示进入同意等待；`Err(code)` 是可直接回给主控的
    /// 错误码（`disabled` / `busy`）。
    pub fn request_session(&self, client_id: &str) -> Result<i64, &'static str> {
        match self.evaluate() {
            Admission::Reject(code) => Err(code),
            Admission::Consent => {
                let deadline = (self.now_ms)() + CONSENT_SAFETY_TIMEOUT_MS;
                let changed = {
                    let mut inner = self.inner.lock().expect("session machine poisoned");
                    inner.session = ControlledSession::PendingConsent {
                        client_id: client_id.to_string(),
                        deadline_ms: deadline,
                    };
                    Self::refresh_controlled(&mut inner)
                };
                self.notify_if_changed(changed);
                Ok(deadline)
            }
        }
    }

    /// main 回帖（TS `respondConsent` + `settleConsent`）。
    ///
    /// `clientId` 不匹配是调用方 bug：TS 抛「没有待处理的同意请求」，这里同。
    pub fn respond_consent(&self, client_id: &str, result: ConsentResult) -> Result<(), String> {
        let mut inner = self.inner.lock().expect("session machine poisoned");
        let matches_pending = matches!(
            &inner.session,
            ControlledSession::PendingConsent { client_id: pending, .. } if pending == client_id
        );
        if !matches_pending {
            return Err("没有待处理的同意请求".to_string());
        }
        inner.session = match result {
            ConsentResult::Allow => ControlledSession::PendingSocket {
                client_id: client_id.to_string(),
                deadline_ms: (self.now_ms)() + PENDING_SOCKET_TTL_MS,
            },
            ConsentResult::Deny(_) => ControlledSession::Idle,
        };
        let changed = Self::refresh_controlled(&mut inner);
        drop(inner);
        self.notify_if_changed(changed);
        Ok(())
    }

    /// WS 接线成功（媒体面就绪）。
    ///
    /// 只有该 client 的 pending socket 才允许推进——迟到的接线不能顶掉别人的会话。
    pub fn attach_socket(&self, client_id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().expect("session machine poisoned");
        let matches_pending = matches!(
            &inner.session,
            ControlledSession::PendingSocket { client_id: pending, .. } if pending == client_id
        );
        if !matches_pending {
            return Err("没有等待接线的会话".to_string());
        }
        inner.session = ControlledSession::Active {
            client_id: client_id.to_string(),
        };
        // active 仍是占用态，controlled 不变（不通知）。
        Ok(())
    }

    /// 推进时间：处理两个 TTL。
    ///
    /// 返回被过期清理掉的会话（供端口去关对应的 WS / 记录日志）。
    pub fn tick(&self) -> Option<ControlledSession> {
        let now = (self.now_ms)();
        let mut inner = self.inner.lock().expect("session machine poisoned");
        let expired = match &inner.session {
            ControlledSession::PendingConsent { deadline_ms, .. } if *deadline_ms <= now => {
                Some(inner.session.clone())
            }
            ControlledSession::PendingSocket { deadline_ms, .. } if *deadline_ms <= now => {
                Some(inner.session.clone())
            }
            _ => None,
        };
        let expired = expired?;
        inner.session = ControlledSession::Idle;
        let changed = Self::refresh_controlled(&mut inner);
        drop(inner);
        self.notify_if_changed(changed);
        Some(expired)
    }

    /// 终止会话（关开关 / peer bye / dispose 共用）。
    ///
    /// 返回被终止的会话（`None` = 本来就没有会话）。
    pub fn stop(&self, reason: &str) -> Option<ControlledSession> {
        let _ = reason;
        let mut inner = self.inner.lock().expect("session machine poisoned");
        let previous = std::mem::replace(&mut inner.session, ControlledSession::Idle);
        let changed = Self::refresh_controlled(&mut inner);
        drop(inner);
        self.notify_if_changed(changed);
        previous.is_occupied().then_some(previous)
    }

    /// 释放整个状态机（Host 退出）。
    pub fn dispose(&self) -> Option<ControlledSession> {
        let stopped = self.stop("dispose");
        let mut inner = self.inner.lock().expect("session machine poisoned");
        inner.disposed = true;
        inner.listeners.clear();
        stopped
    }

    /// 订阅被控态变化；**首个订阅者立即收到当前值**（幂等事实）。
    pub fn subscribe(self: &Arc<Self>, emit: EventCallback) -> Subscription {
        let id = format!(
            "controlled-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        );
        let current = {
            let mut inner = self.inner.lock().expect("session machine poisoned");
            inner.listeners.push((
                id.clone(),
                Arc::new(move |controlled: bool| {
                    emit(WireValue::Json(
                        json!({ "controlled": controlled }).to_string(),
                    ));
                }),
            ));
            inner.last_controlled
        };
        // 幂等事实：补发当前值（`onDidChangeControlledState` 的语义）。
        let listener = {
            let inner = self.inner.lock().expect("session machine poisoned");
            inner
                .listeners
                .iter()
                .find(|(key, _)| key == &id)
                .map(|(_, listener)| Arc::clone(listener))
        };
        if let Some(listener) = listener {
            listener(current);
        }
        let machine = Arc::clone(self);
        Box::new(move || {
            machine
                .inner
                .lock()
                .expect("session machine poisoned")
                .listeners
                .retain(|(key, _)| key != &id);
        })
    }

    /// 重算 `controlled` 并返回是否变化（TS `notifyControlled` 的比较）。
    fn refresh_controlled(inner: &mut Inner) -> bool {
        let controlled = inner.session.is_occupied();
        if controlled == inner.last_controlled {
            return false;
        }
        inner.last_controlled = controlled;
        true
    }

    fn notify_if_changed(&self, changed: bool) {
        if !changed {
            return;
        }
        let (controlled, listeners) = {
            let inner = self.inner.lock().expect("session machine poisoned");
            (
                inner.last_controlled,
                inner
                    .listeners
                    .iter()
                    .map(|(_, listener)| Arc::clone(listener))
                    .collect::<Vec<_>>(),
            )
        };
        for listener in listeners {
            // 订阅方异常不得影响会话状态机（TS 同）。
            listener(controlled);
        }
    }
}

/// 便捷：把 `ControlledSession` 转成线上事件形状（诊断/日志用）。
pub fn session_to_json(session: &ControlledSession) -> Json {
    json!({
        "state": session.as_str(),
        "clientId": session.client_id(),
    })
}

/// 仅供测试：把「已 dispose」标记读出来。
pub fn is_disposed(machine: &ControlledSessionMachine) -> bool {
    machine
        .inner
        .lock()
        .expect("session machine poisoned")
        .disposed
}
