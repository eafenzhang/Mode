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
//! 控制面：由 `WsHostDeps.agent_control` 注入（桌面 Host 传入，server-mode 为 None→
//! agent 频道构造留痕跳过）。

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
    /// Agent 控制面：`Some` 时 LAN 客户端获得 mode-agent/mode-task 等频道
    /// （桌面 Host 注入）；server-mode 没有 Agent 面时为 `None`（频道构造留痕跳过）。
    pub agent_control: Option<Arc<agent_process::AgentControlClient>>,
    /// 频道装配的数据根（memory / conversation-mirror / usage-stats 等）。
    pub data_root: PathBuf,
    /// LAN 配对面（可选）：`None` 时该连接不参与「最近打开目录」录制
    /// （TS resolveLanClientByToken 未提供时行为不变，同口径）。
    pub lan_pairing: Option<crate::lan::SharedLanHttp>,
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
///
/// 该连接对应哪台已配对客户端：cookie/query 令牌 → clientId（与鉴权同一校验
/// 路径），解析成功才挂「最近打开目录」录制；普通或无 LAN 配置的连接行为不变。
async fn upgrade_trusted_host(
    State(deps): State<WsHostDeps>,
    headers: HeaderMap,
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
    upgrade: WebSocketUpgrade,
) -> Response {
    let client_id = resolve_lan_client_id(&deps, headers, raw_query.as_deref()).await;
    upgrade.on_upgrade(move |socket| bridge_host_socket(socket, deps, client_id))
}

/// TS `readPresentedLiteToken` + `resolveLanClientByToken` 的合并：
/// query `token` 优先，cookie 两种 lite token 名兜底（`lan_http::presented_token`）。
async fn resolve_lan_client_id(
    deps: &WsHostDeps,
    headers: HeaderMap,
    raw_query: Option<&str>,
) -> Option<String> {
    let pairing = deps.lan_pairing.as_ref()?;
    let query_token = raw_query.and_then(|query| {
        form_urlencoded_parse(query)
            .into_iter()
            .find(|(key, _)| key == "token")
            .map(|(_, value)| value)
    });
    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok());
    let presented = lan_http::presented_token(None, cookie, query_token.as_deref())?;
    pairing.resolve_client_by_token(&presented).await
}

fn form_urlencoded_parse(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (
                urldecode(key),
                urldecode(value),
            )
        })
        .collect()
}

fn urldecode(value: &str) -> String {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() + 1 && index + 2 < bytes.len() + 1 => {
                let hex = bytes.get(index + 1..index + 3);
                if let Some(hex) = hex {
                    if let Ok(byte) = u8::from_str_radix(&String::from_utf8_lossy(hex), 16) {
                        out.push(byte);
                        index += 3;
                        continue;
                    }
                }
                out.push(b'%');
                index += 1;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// TS `extractWorkspacePathArg`：只有第一参数对象顶层的 workspacePath 才算
/// 「打开工作区」信号；readdir/resolvePath 用 path 字段，不录。
pub fn extract_workspace_path_from_frame(frame: &[u8]) -> Option<String> {
    let request = rpc_wire::decode_rpc_message(frame).ok()?;
    match request.header {
        rpc_wire::RequestHeader::Promise { .. }
        | rpc_wire::RequestHeader::EventListen { .. } => {}
        _ => return None,
    }
    let rpc_wire::Value::Array(items) = request.body else {
        return None;
    };
    let rpc_wire::Value::Json(text) = items.first()? else {
        return None;
    };
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let path = json.get("workspacePath")?.as_str()?;
    (!path.trim().is_empty()).then(|| path.to_string())
}

/// 每连接一个 `ChannelServer`（TS `new ChannelServer(protocol, "server")` 同款：
/// deferInit 缺省 = 构造即向对端发 Initialize）；入站 WS 帧 → `handle_frame`，
/// 出站帧 → WS binary；`client_id` 解析成功的连接同步录制最近打开目录。
async fn bridge_host_socket(socket: WebSocket, deps: WsHostDeps, client_id: Option<String>) {
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut server =
        ChannelServer::new(out_tx, "server", Duration::from_millis(1000), false);
    host_channels::register_service_channels(
        &mut server,
        &ChannelAssemblyOptions {
            data_root: deps.data_root,
            agent_control: deps.agent_control.clone(),
            on_shell_log: None,
            // 每连接一条：单连接内 mode-task → window-controller 投影刷新。
            task_events: std::sync::Arc::new(host_channels::mode_task::TaskEventBus::new()),
            // server-mode 的 lan-access 由宿主监督器注入（本入口 None）。
            lan_host: None,
            mode_builtin_provider_config_path: None,
            // server-mode 的 /ws/host 不承载 remote attach（那是桌面 Host 的事）。
            remote_scope_out: None,
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
            Message::Binary(data) => {
                if let Some(workspace_path) = extract_workspace_path_from_frame(&data) {
                    if let Some(client_id) = &client_id {
                        if let Some(pairing) = &deps.lan_pairing {
                            pairing
                                .record_client_workspace_for_client(client_id, &workspace_path)
                                .await;
                        }
                    }
                }
                server.handle_frame(&data);
            }
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
    // （路由组本身不注入 LAN 依赖；`WsHostDeps.lan_pairing` 由装配方决定。）
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
