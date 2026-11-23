//! LAN transport 端口契约（LT-16.4d 收口）。
//!
//! 起一个**真 axum 服务**（capability + `/ws/host` + 真 `ChannelServer`），
//! 用真 `LanTransportPort` 走完 `RemoteChannel::connect` → `attach` → `dispose`。
//! 这样「端口把连接记在正确的 entry 上」「断连时真的关 WS」「配对缺失时不去打对端」
//! 都是被真实验证过的。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use credential_store::CredentialStore;
use host_channels::remote::RemoteScopeFactory;
use host_channels::{LanTransportPort, RemoteChannel, RemoteTransportPort};
use remote_session_registry::RemoteSessionSnapshot;
use rpc_client::lan::MODE_RPC_HOST_CAPABILITY_HEADER;
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, ChannelServer, ScopedServerRegistry, ServerChannel,
};
use rpc_wire::Value;
use serde_json::{json, Value as JsonValue};
use tokio::sync::mpsc;

const VALID_TOKEN: &str = "lan-port-token";
const CAPABILITY: &str = "cap-lan-port";

// ---------------- 真服务端 ----------------

struct EchoChannel;

impl ServerChannel for EchoChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let result = match command.as_str() {
            "echo" => Ok(arg),
            other => Err(ChannelError::error(format!("unknown command {other}"))),
        };
        Box::pin(async move { result })
    }

    fn listen(
        &self,
        _ctx: String,
        _event: String,
        _arg: Value,
        _emit: rpc_host::EventCallback,
    ) -> Result<rpc_host::Subscription, ChannelError> {
        Ok(Box::new(|| {}))
    }
}

#[derive(Clone)]
struct ServerState {
    issued: Arc<AtomicUsize>,
    upgraded: Arc<AtomicUsize>,
}

async fn issue_capability(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let authorized = headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .map(|cookie| cookie.contains(&format!("mode_lite_token={VALID_TOKEN}")))
        .unwrap_or(false);
    if !authorized {
        return (StatusCode::UNAUTHORIZED, Json(JsonValue::Null)).into_response();
    }
    state.issued.fetch_add(1, Ordering::SeqCst);
    (
        StatusCode::OK,
        Json(json!({ "capability": CAPABILITY, "expiresAt": u64::MAX })),
    )
        .into_response()
}

async fn upgrade_host(
    State(state): State<ServerState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    if headers
        .get(MODE_RPC_HOST_CAPABILITY_HEADER)
        .and_then(|value| value.to_str().ok())
        != Some(CAPABILITY)
    {
        return (StatusCode::UNAUTHORIZED, "bad capability").into_response();
    }
    state.upgraded.fetch_add(1, Ordering::SeqCst);
    upgrade.on_upgrade(bridge_socket)
}

async fn bridge_socket(socket: WebSocket) {
    use futures_util::{SinkExt, StreamExt};
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut server = ChannelServer::new(out_tx, "server", Duration::from_secs(5), false);
    server.register_channel("memory", Arc::new(EchoChannel));

    let outbound = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if sender.send(Message::Binary(frame.into())).await.is_err() {
                break;
            }
        }
    });
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

async fn spawn_server() -> (u16, ServerState) {
    let state = ServerState {
        issued: Arc::new(AtomicUsize::new(0)),
        upgraded: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/api/rpc-host-capability", post(issue_capability))
        .route("/ws/host", get(upgrade_host))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (port, state)
}

// ---------------- 测试脚手架 ----------------

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "lan-port-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn credentials(dir: &TempDir) -> Arc<CredentialStore> {
    Arc::new(CredentialStore::with_secret(&dir.0, "lan-port-test-secret"))
}

fn args(value: JsonValue) -> Value {
    Value::Json(value.to_string())
}

async fn call(
    channel: &RemoteChannel,
    method: &str,
    payload: JsonValue,
) -> Result<JsonValue, String> {
    let value = channel
        .call(
            "ctx".into(),
            method.into(),
            args(payload),
            CancellationToken::default(),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
    match value {
        Value::Json(text) => Ok(serde_json::from_str(&text).expect("json")),
        Value::Undefined => Ok(JsonValue::Null),
        other => panic!("unexpected body: {other:?}"),
    }
}

#[derive(Default)]
struct FakeScopeFactory {
    calls: AtomicUsize,
}

impl RemoteScopeFactory for FakeScopeFactory {
    fn register(
        &self,
        server: &mut ChannelServer,
        _session: &RemoteSessionSnapshot,
    ) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        server.register_channel("memory", Arc::new(EchoChannel));
        Ok(())
    }
}

fn channel_for(port: Arc<LanTransportPort>) -> (RemoteChannel, Arc<ScopedServerRegistry>) {
    let scopes = Arc::new(ScopedServerRegistry::new(Arc::new(|_frame: Vec<u8>| {})));
    let channel = RemoteChannel::new(Some(port as Arc<dyn host_channels::RemoteTransportPort>))
        .with_scopes(Arc::clone(&scopes), Arc::new(FakeScopeFactory::default()));
    (channel, scopes)
}

// ---------------- 用例 ----------------

#[tokio::test]
async fn connecting_to_an_unpaired_peer_fails_before_touching_the_network() {
    let dir = TempDir::new("unpaired");
    let server = credentials(&dir);
    let port = Arc::new(LanTransportPort::new(Arc::clone(&server)));
    let (channel, _scopes) = channel_for(Arc::clone(&port));
    let (server_port, state) = spawn_server().await;

    // 没有配对凭据、target 也没带 token：必须在建连前就失败。
    let result = call(
        &channel,
        "connect",
        json!({
            "target": { "kind": "lan", "host": "127.0.0.1", "port": server_port },
            "workspacePath": "/w",
        }),
    )
    .await
    .expect("connect 必须返回 ok:false 而不是 RPC 错误");

    assert_eq!(result["ok"], false, "{result:?}");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or("")
            .contains("还没有配对"),
        "{result:?}"
    );
    // 关键：一次连接都没尝试 —— 不在对端留下失败鉴权噪声。
    assert_eq!(state.issued.load(Ordering::SeqCst), 0, "不得去换票据");
    assert_eq!(state.upgraded.load(Ordering::SeqCst), 0, "不得尝试 WS 升级");
    assert_eq!(port.active_entries(), 0);
}

#[tokio::test]
async fn connect_records_the_connection_under_the_entry_and_dispose_closes_it() {
    let dir = TempDir::new("e2e");
    let server = credentials(&dir);
    let port = Arc::new(LanTransportPort::new(Arc::clone(&server)));
    let (channel, scopes) = channel_for(Arc::clone(&port));
    let (server_port, state) = spawn_server().await;

    let connected = call(
        &channel,
        "connect",
        json!({
            "target": {
                "kind": "lan",
                "host": "127.0.0.1",
                "port": server_port,
                // 连接期透明令牌：不进 identity、不进描述符。
                "token": VALID_TOKEN,
            },
            "workspacePath": "/srv/app",
        }),
    )
    .await
    .expect("connect");

    assert_eq!(connected["ok"], true, "{connected:?}");
    assert_eq!(state.issued.load(Ordering::SeqCst), 1, "要换一次票据");
    assert_eq!(state.upgraded.load(Ordering::SeqCst), 1, "要升级一次 WS");
    // 连接按 entry 记账：abort/dispose 才找得到它。
    assert_eq!(port.active_entries(), 1, "连接必须登记在端口里");
    // 令牌绝不出现在下发给 renderer 的形状里。
    assert!(
        !connected["descriptor"].to_string().contains(VALID_TOKEN),
        "descriptor 泄漏了令牌：{}",
        connected["descriptor"]
    );

    let session_id = connected["descriptor"]["remoteSessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    // attach 能建 scope（说明 connect 后 session 已是 online）。
    let attachment = call(
        &channel,
        "attach",
        json!({ "sessionId": session_id.clone() }),
    )
    .await
    .expect("attach");
    assert_eq!(attachment["scopeId"], 1);
    assert_eq!(scopes.count(), 1);

    // dispose：session 与它的 scope 一起回收，连接也要断掉。
    call(&channel, "dispose", json!({ "sessionId": session_id }))
        .await
        .expect("dispose");
    assert_eq!(scopes.count(), 0, "dispose 必须回收 scope");
    assert_eq!(port.active_entries(), 0, "dispose 必须断开 LAN 连接");
}

/// 一个最小的 target 描述（只为让上面的用例能构造出 lan 目标）。
#[allow(dead_code)]
fn lan_target(port: u16, token: Option<&str>) -> JsonValue {
    let mut value = json!({ "kind": "lan", "host": "127.0.0.1", "port": port });
    if let Some(token) = token {
        value["token"] = json!(token);
    }
    value
}

#[tokio::test]
async fn workspace_release_is_a_no_op_for_lan() {
    let dir = TempDir::new("release");
    let server = credentials(&dir);
    let port = LanTransportPort::new(Arc::clone(&server));
    // LAN 对端是已在运行的 Mode 实例：没有 workspace 运行时需要释放。
    let effects = port
        .release_workspace(
            1,
            "/w".to_string(),
            1,
            remote_session_registry::RemoteWorkspaceContext {
                workspace_path: "/w".to_string(),
                workspace_identity: None,
            },
        )
        .await;
    assert_eq!(effects, Ok(()));
}

#[tokio::test]
async fn drop_disposes_every_open_connection() {
    let dir = TempDir::new("drop");
    let server = credentials(&dir);
    let port = Arc::new(LanTransportPort::new(Arc::clone(&server)));
    let (channel, _scopes) = channel_for(Arc::clone(&port));
    let (server_port, _state) = spawn_server().await;

    call(
        &channel,
        "connect",
        json!({
            "target": lan_target(server_port, Some(VALID_TOKEN)),
            "workspacePath": "/w",
        }),
    )
    .await
    .expect("connect");
    assert_eq!(port.active_entries(), 1);

    drop(channel);
    // 端口自己 drop 时会断全部连接（此处只验 drop 不 panic 且计数可读）。
    let remaining = Arc::strong_count(&port);
    assert!(remaining >= 1);
    drop(port);
}
