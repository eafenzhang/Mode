//! LAN 被连端监督器（LT-16.4a）：监听生命周期 + 状态聚合 + `onDidChange`。
//!
//! 复用既有底座：
//! - `crate::lan::lan_router`（配对/管理端点，状态唯一所有者仍是 `LanPairHttp`）；
//! - `crate::ws_host::ws_host_router`（capability + `/ws/host` RPC 帧桥）；
//! - `crate::router`（健康/版本/server-info）。
//!
//! 语义（真源 `packages/desktop/src/host-standalone/lanAccessService.ts`）：
//! - `set_enabled(true)` → 绑定 `0.0.0.0:默认端口`（被占用回落系统分配）；
//!   成功后才持久化开关，启动失败抛错而不是返回假状态。
//! - `set_enabled(false)` → 停监听、清配对码，state 的 enabled/port/addresses 归零。
//! - clients/pairCode 由 `LanPairHttp` 从凭据读写；监督器只更新监听视图。
//! - `onDidChange` 是幂等快照事件：首个订阅者立即收到当前状态。
//!
//! 尚未迁移：静态 Web 资源（手机页面 `/`）、公网中继、逐设备远程桌面 hooks；
//! 当前监听只提供 `/api/*` 与 `/ws/host`（桌面↔桌面挂接可用）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use lan_http::LanAccessView;
use rpc_host::EventCallback;
use rpc_wire::Value as WireValue;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::lan::SharedLanHttp;
use crate::server_info::ServerInfoConfig;
use crate::ws_host::{ws_host_router, WsHostDeps};

/// 服务端首选监听端口（被占用时回落系统分配）。
pub const LAN_ACCESS_DEFAULT_PORT: u16 = 45_880;
/// 停止监听时等待 serve 任务收口的宽限。
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

struct Running {
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

/// LAN server 监督器；实现 `LanAccessHostPort` 供 `lan-access` 频道注入。
pub struct LanHostSupervisor {
    handle: Handle,
    pairing: SharedLanHttp,
    info: ServerInfoConfig,
    ws_host: WsHostDeps,
    default_port: u16,
    /// 开关持久化（桌面 = setting `lanAccessEnabled`；server-mode = 凭据键）。
    persist_enabled: Arc<dyn Fn(bool) + Send + Sync>,
    running: Mutex<Option<Running>>,
    /// start/stop 串行化，避免并发开关产生双监听。
    transition: tokio::sync::Mutex<()>,
    listeners: Arc<Mutex<HashMap<String, EventCallback>>>,
}

impl LanHostSupervisor {
    pub fn new(
        handle: Handle,
        pairing: SharedLanHttp,
        info: ServerInfoConfig,
        ws_host: WsHostDeps,
        default_port: u16,
        persist_enabled: Arc<dyn Fn(bool) + Send + Sync>,
    ) -> Self {
        Self {
            handle,
            pairing,
            info,
            ws_host,
            default_port,
            persist_enabled,
            running: Mutex::new(None),
            transition: tokio::sync::Mutex::new(()),
            listeners: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 当前监听状态（`LanAccessState` 形状；clients 来自凭据）。
    pub async fn state(&self) -> serde_json::Value {
        self.pairing.get_state().await
    }

    pub async fn is_running(&self) -> bool {
        self.running.lock().expect("lan host running poisoned").is_some()
    }

    /// 启动/切换监听模式；`expose_lan=false` 时只绑回环。
    pub async fn start(&self, expose_lan: bool) -> Result<serde_json::Value, String> {
        let _transition = self.transition.lock().await;
        self.stop_locked().await;
        let (listener, port) = self.bind(expose_lan).await?;
        let addresses = if expose_lan {
            lan_addresses()
        } else {
            Vec::new()
        };
        // 状态所有者仍是 LanPairHttp：只更新监听视图。
        self.pairing.set_view(LanAccessView {
            enabled: expose_lan,
            port: Some(port),
            addresses: addresses.clone(),
            active_web_sessions: 0,
            public_relay_url: None,
        });
        let router = self.router();
        let (shutdown, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await;
        });
        *self.running.lock().expect("lan host running poisoned") = Some(Running {
            shutdown: Some(shutdown),
            task,
        });
        let state = self.state().await;
        self.emit_state(&state);
        Ok(state)
    }

    /// 停止监听（幂等）；清配对码并把状态归零。
    pub async fn stop(&self) {
        let _transition = self.transition.lock().await;
        self.stop_locked().await;
        let state = self.state().await;
        self.emit_state(&state);
    }

    /// 开关入口：持久化发生在启动成功之后（启动失败不写开关）。
    pub async fn set_enabled(&self, enabled: bool) -> Result<serde_json::Value, String> {
        if enabled {
            self.start(true).await?;
        } else {
            self.stop().await;
        }
        (self.persist_enabled)(enabled);
        Ok(self.state().await)
    }

    pub async fn create_pair_code(&self) -> Result<serde_json::Value, String> {
        let state = self
            .pairing
            .create_pair_code()
            .await
            .map_err(str::to_string)?;
        self.emit_state(&state);
        Ok(state)
    }

    pub async fn remove_client(&self, client_id: &str) -> Result<serde_json::Value, String> {
        let state = self
            .pairing
            .remove_client(client_id)
            .await
            .map_err(str::to_string)?;
        self.emit_state(&state);
        Ok(state)
    }

    pub async fn reset_tokens(&self) -> Result<serde_json::Value, String> {
        let state = self
            .pairing
            .reset_tokens()
            .await
            .map_err(str::to_string)?;
        self.emit_state(&state);
        Ok(state)
    }

    /// `onDidChange`：注册监听者并立即补发当前快照。
    ///
    /// `listen` 由 ChannelServer 在异步上下文调用，不能 `block_on`；快照通过
    /// `handle.spawn` 投递到同一事件循环（幂等事实语义不变）。
    pub fn subscribe_changes(&self, emit: EventCallback) -> rpc_host::Subscription {
        let id = uuid::Uuid::new_v4().to_string();
        self.listeners
            .lock()
            .expect("lan host listeners poisoned")
            .insert(id.clone(), Arc::clone(&emit));
        let pairing = Arc::clone(&self.pairing);
        self.handle.spawn(async move {
            let state = pairing.get_state().await;
            if let Ok(text) = serde_json::to_string(&state) {
                emit(WireValue::Json(text));
            }
        });
        let listeners = Arc::clone(&self.listeners);
        Box::new(move || {
            listeners
                .lock()
                .expect("lan host listeners poisoned")
                .remove(&id);
        })
    }

    fn router(&self) -> Router {
        crate::router_with_lan(self.info.clone(), Arc::clone(&self.pairing))
            .merge(ws_host_router(self.ws_host.clone()))
    }

    async fn stop_locked(&self) {
        let running = self.running.lock().expect("lan host running poisoned").take();
        if let Some(mut running) = running {
            if let Some(shutdown) = running.shutdown.take() {
                let _ = shutdown.send(());
            }
            let _ = tokio::time::timeout(STOP_GRACE, running.task).await;
        }
        // 服务器不再监听：清配对码并把视图归零（状态所有者仍是 LanPairHttp）。
        self.pairing.set_enabled(false).await;
    }

    async fn bind(&self, expose_lan: bool) -> Result<(TcpListener, u16), String> {
        let host = if expose_lan { "0.0.0.0" } else { "127.0.0.1" };
        let preferred = if expose_lan { self.default_port } else { 0 };
        let listener = match TcpListener::bind((host, preferred)).await {
            Ok(listener) => listener,
            // 首选端口被占用 → 回落系统分配（TS 同）。
            Err(_) if preferred != 0 => TcpListener::bind((host, 0))
                .await
                .map_err(|error| format!("LAN listen failed: {error}"))?,
            Err(error) => return Err(format!("LAN listen failed: {error}")),
        };
        let port = listener
            .local_addr()
            .map_err(|error| format!("LAN listen address unavailable: {error}"))?
            .port();
        Ok((listener, port))
    }

    fn emit_state(&self, state: &serde_json::Value) {
        let Ok(text) = serde_json::to_string(state) else {
            return;
        };
        let listeners: Vec<EventCallback> = self
            .listeners
            .lock()
            .expect("lan host listeners poisoned")
            .values()
            .cloned()
            .collect();
        for emit in listeners {
            emit(WireValue::Json(text.clone()));
        }
    }
}

impl Drop for LanHostSupervisor {
    fn drop(&mut self) {
        if let Some(mut running) = self.running.lock().expect("lan host running poisoned").take() {
            if let Some(shutdown) = running.shutdown.take() {
                let _ = shutdown.send(());
            }
            running.task.abort();
        }
    }
}

/// 对外暴露的 LAN 地址（排除回环；`0.0.0.0` 绑定后客户端需要具体地址）。
fn lan_addresses() -> Vec<String> {
    let mut addresses: Vec<String> = lan_discovery::local_ipv4_addresses()
        .into_iter()
        .filter(|address| address != "127.0.0.1")
        .collect();
    addresses.sort();
    addresses
}

/// 供 `lan-access` 频道注入的同步端口实现：宿主运行时由构造时的 `Handle` 承接。
impl host_channels::lan_access::LanAccessHostPort for LanHostSupervisor {
    fn state(&self) -> Result<serde_json::Value, String> {
        Ok(self.handle.block_on(self.pairing.get_state()))
    }

    fn set_enabled(&self, enabled: bool) -> Result<serde_json::Value, String> {
        self.handle.block_on(LanHostSupervisor::set_enabled(self, enabled))
    }

    fn create_pair_code(&self) -> Result<serde_json::Value, String> {
        self.handle
            .block_on(LanHostSupervisor::create_pair_code(self))
    }

    fn remove_client(&self, client_id: &str) -> Result<serde_json::Value, String> {
        self.handle
            .block_on(LanHostSupervisor::remove_client(self, client_id))
    }

    fn reset_tokens(&self) -> Result<serde_json::Value, String> {
        self.handle.block_on(LanHostSupervisor::reset_tokens(self))
    }

    fn subscribe_changes(&self, emit: EventCallback) -> rpc_host::Subscription {
        LanHostSupervisor::subscribe_changes(self, emit)
    }
}
