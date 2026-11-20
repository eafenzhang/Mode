//! `/ws/host`：trusted-host WS RPC 帧桥（LT-15 P2 切二）。
//!
//! 真源：`packages/server/src/http.ts` 的 `/ws/host` 与 `hostCapability.ts`。
//! 语义：升级前校验一次性 host capability（`x-mode-rpc-host-capability` header，
//! 30s TTL、先删后比——重放/过期一律 401）；升级后每连接一个
//! `rpc_host::ChannelServer`（defer_init，与 inprocess 同款：Initialize 由对端
//! renderer 发起），WS 帧双向转发，频道清单复用 `host_channels::assembly`。
//!
//! 差异记录 spec：LAN 客户端录制（`resolveLanClientByToken`/workspace 记录）依赖
//! Node Host 录制面，未迁；`connectionScope`（mode-agent 连接域）依赖 Agent
//! 控制面，server-mode 首切不注册 agent 频道（`agent_control: None`）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use host_channels::ChannelAssemblyOptions;
use rpc_host::ChannelServer;
use serde::Serialize;
use serde_json::json;
use tokio::sync::{mpsc, Mutex};

/// TS `MODE_RPC_HOST_CAPABILITY_HEADER`（packages/shared/src/channels.ts）。
pub const MODE_RPC_HOST_CAPABILITY_HEADER: &str = "x-mode-rpc-host-capability";
/// TS `DEFAULT_HOST_CAPABILITY_TTL_MS`。
pub const DEFAULT_HOST_CAPABILITY_TTL_MS: u64 = 30_000;

/// TS `ServerRemoteHostCapability`。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerRemoteHostCapability {
    pub capability: String,
    pub expires_at: u64,
}

type InstantClock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// 短期、一次性 desktop host capability；只在 server 进程内存中存在。
///
/// TS `createHostCapabilityStore` 同构：issue 进 TTL 池；consume 无论成败先删除
/// （旧 header 是可重放的长期提权声明，只有首次且 TTL 内的消费有效）。
pub struct HostCapabilityStore {
    ttl: Duration,
    expires_by_capability: Mutex<HashMap<String, Instant>>,
    now: InstantClock,
    create_capability: Arc<dyn Fn() -> String + Send + Sync>,
}

impl HostCapabilityStore {
    /// 默认装配：30s TTL + 真实时钟 + `randomBytes(32).toString("base64url")`
    /// 等价（`lan_pairing::mint_client_token`）。
    pub fn new() -> Self {
        Self::with_injectables(
            Duration::from_millis(DEFAULT_HOST_CAPABILITY_TTL_MS),
            Arc::new(Instant::now),
            // workspace 无 rand/getrandom；uuid v4 × 2 拼接（122bit×2 熵），
            // capability 是不透明一次性 token，格式与 TS base64url 的差异无语义影响
            // （spec 已知差异）。
            Arc::new(|| {
                format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                )
            }),
        )
    }

    pub fn with_injectables(
        ttl: Duration,
        now: InstantClock,
        create_capability: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Self {
        Self {
            ttl,
            expires_by_capability: Mutex::new(HashMap::new()),
            now,
            create_capability,
        }
    }

    pub async fn issue(&self) -> ServerRemoteHostCapability {
        let issued_at = (self.now)();
        self.purge_expired(issued_at).await;
        let capability = (self.create_capability)();
        let expires_instant = issued_at + self.ttl;
        self.expires_by_capability
            .lock()
            .await
            .insert(capability.clone(), expires_instant);
        // 对外 expiresAt 用墙钟毫秒（TS Date.now() 口径）；池内比较用单调 Instant。
        let now_wall_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0);
        ServerRemoteHostCapability {
            capability,
            expires_at: now_wall_ms + self.ttl.as_millis() as u64,
        }
    }

    pub async fn consume(&self, capability: Option<&str>) -> bool {
        let Some(capability) = capability.map(str::trim).filter(|value| !value.is_empty()) else {
            return false;
        };
        let consumed_at = (self.now)();
        let mut pool = self.expires_by_capability.lock().await;
        // 先删后比：无论成功、过期还是重放都一次性消费。
        let expires_at = pool.remove(capability);
        drop(pool);
        self.purge_expired(consumed_at).await;
        expires_at.is_some_and(|expires| expires > consumed_at)
    }

    async fn purge_expired(&self, at: Instant) {
        self.expires_by_capability
            .lock()
            .await
            .retain(|_, expires| *expires > at);
    }
}

impl Default for HostCapabilityStore {
    fn default() -> Self {
        Self::new()
    }
}

/// `/ws/host` 面的装配依赖。
#[derive(Clone)]
pub struct WsHostDeps {
    pub host_capabilities: Arc<HostCapabilityStore>,
    /// 频道装配的数据根（memory / conversation-mirror / usage-stats 等）。
    pub data_root: PathBuf,
}

/// `/api/rpc-host-capability`（TS 同名端点）：签发一次性 host capability。
async fn issue_capability(State(deps): State<WsHostDeps>) -> Json<ServerRemoteHostCapability> {
    Json(deps.host_capabilities.issue().await)
}

/// capability 校验中间件：在 `WebSocketUpgrade` extractor 之前跑
/// （extractor 对非升级请求直接 426，会吞掉 401 JSON 响应——TS 语义是
/// 「无效 capability → JSON 响应、不升级连接」）。
async fn require_host_capability(
    State(deps): State<WsHostDeps>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let capability = headers
        .get(MODE_RPC_HOST_CAPABILITY_HEADER)
        .and_then(|value| value.to_str().ok());
    if !deps.host_capabilities.consume(capability).await {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Invalid or expired host capability" })),
        )
            .into_response();
    }
    next.run(request).await
}

/// `/ws/host` 升级 handler（capability 已由中间件消费）。
async fn upgrade_trusted_host(
    State(deps): State<WsHostDeps>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| bridge_host_socket(socket, deps))
}

/// 每连接一个 `ChannelServer`（TS `new ChannelServer(protocol, "server")` 同款：
/// deferInit 缺省 = 构造即向对端发 Initialize）；入站 WS 帧 → `handle_frame`，
/// 出站帧 → WS binary。
async fn bridge_host_socket(socket: WebSocket, deps: WsHostDeps) {
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut server =
        ChannelServer::new(out_tx, "server", Duration::from_millis(1000), false);
    host_channels::register_service_channels(
        &mut server,
        &ChannelAssemblyOptions {
            data_root: deps.data_root,
            agent_control: None,
            on_shell_log: None,
        },
    );
    let outbound = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if sender.send(Message::Binary(frame.into())).await.is_err() {
                break;
            }
        }
    });
    use futures_util::{SinkExt, StreamExt};
    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Binary(data) => server.handle_frame(&data),
            Message::Text(text) => server.handle_frame(text.as_bytes()),
            Message::Close(_) => break,
            _ => {}
        }
    }
    server.dispose();
    outbound.abort();
}

/// `/ws/host` 路由组：capability 签发 + trusted-host 升级。
pub fn ws_host_router(deps: WsHostDeps) -> Router {
    Router::new()
        .route("/api/rpc-host-capability", post(issue_capability))
        .route(
            "/ws/host",
            get(upgrade_trusted_host).layer(axum::middleware::from_fn_with_state(
                deps.clone(),
                require_host_capability,
            )),
        )
        .with_state(deps)
}
