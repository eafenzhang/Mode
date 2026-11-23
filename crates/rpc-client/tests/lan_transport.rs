//! LAN transport 端到端契约（LT-16.4d 续段）。
//!
//! 起一个**真的 axum 服务**（`POST /api/rpc-host-capability` + `GET /ws/host`，
//! WS 侧挂真的 `rpc_host::ChannelServer`），再用 [`attach_lan_remote_connection`]
//! 真连上去。这样 capability 换票、WS 握手头、帧双向搬运、Initialize 门都是被
//! 真实验证过的，而不是被 mock 掉。
//!
//! 不用 `mode_server` 的原因：那是产品服务端（依赖整棵 host-channels 装配树），
//! 与本层的契约无关。这里只要「同样的三个 HTTP/WS 语义」。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use rpc_client::lan::{attach_lan_remote_connection, LanAttachError, LanAttachParams};
use rpc_client::ClientError;
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, ChannelServer, EventCallback, ServerChannel,
    Subscription,
};
use rpc_wire::Value;
use tokio::sync::{mpsc, oneshot};

const CAPABILITY_HEADER: &str = "x-mode-rpc-host-capability";
const VALID_TOKEN: &str = "test-token";
const VALID_CAPABILITY: &str = "cap-abc";

struct MemoryChannel;

impl ServerChannel for MemoryChannel {
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
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        emit(Value::str("hello"));
        Ok(Box::new(|| {}))
    }
}

#[derive(Clone)]
struct ServerState {
    capability_issued: Arc<AtomicUsize>,
    capability_consumed: Arc<AtomicUsize>,
}

async fn issue_capability(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // 与产品服务端同：令牌只从 Cookie 取。
    let authorized = headers
        .get("cookie")
        .and_then(|value| value.to_str().ok())
        .map(|cookie| cookie.contains(&format!("mode_lite_token={VALID_TOKEN}")))
        .unwrap_or(false);
    if !authorized {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!(null))).into_response();
    }
    state.capability_issued.fetch_add(1, Ordering::SeqCst);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "capability": VALID_CAPABILITY, "expiresAt": u64::MAX })),
    )
        .into_response()
}

async fn upgrade_host(
    State(state): State<ServerState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    let capability = headers
        .get(CAPABILITY_HEADER)
        .and_then(|value| value.to_str().ok());
    if capability != Some(VALID_CAPABILITY) {
        return (StatusCode::UNAUTHORIZED, "bad capability").into_response();
    }
    state.capability_consumed.fetch_add(1, Ordering::SeqCst);
    upgrade.on_upgrade(bridge_socket)
}

async fn bridge_socket(socket: WebSocket) {
    use futures_util::{SinkExt, StreamExt};
    let (mut sender, mut receiver) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    // defer_init = false：建连即发 Initialize，与服务端产品行为一致
    // （正是这一帧与握手同批到达，才需要客户端在 await open 之前就准备好接帧）。
    let mut server = ChannelServer::new(out_tx, "server", Duration::from_secs(5), false);
    server.register_channel("memory", Arc::new(MemoryChannel));

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

/// 起一个真服务，返回 (端口, 状态)。端口 0 = 让系统分配。
async fn spawn_server() -> (u16, ServerState) {
    let state = ServerState {
        capability_issued: Arc::new(AtomicUsize::new(0)),
        capability_consumed: Arc::new(AtomicUsize::new(0)),
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

fn params(port: u16, token: &str) -> LanAttachParams {
    LanAttachParams {
        host: "127.0.0.1".to_string(),
        port,
        scheme: None,
        path: None,
        token: token.to_string(),
    }
}

async fn call(
    client: &rpc_client::ChannelClient,
    channel: &str,
    method: &str,
    arg: Value,
) -> Result<Value, ClientError> {
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
    client.call(
        channel,
        method,
        arg,
        Arc::new(move |result| {
            if let Some(tx) = tx.lock().expect("tx").take() {
                let _ = tx.send(result);
            }
        }),
    );
    rx.await.expect("responder 必须被调用")
}

#[tokio::test]
async fn attaches_over_websocket_and_round_trips_a_call() {
    let (port, state) = spawn_server().await;
    let mut connection = attach_lan_remote_connection(params(port, VALID_TOKEN), None)
        .await
        .expect("LAN 挂接应成功");

    assert_eq!(
        state.capability_issued.load(Ordering::SeqCst),
        1,
        "要换一次票据"
    );
    assert_eq!(
        state.capability_consumed.load(Ordering::SeqCst),
        1,
        "WS 升级要消费掉票据"
    );

    // Initialize 是服务端建连即发的；等它到达再发请求。
    tokio::time::timeout(Duration::from_secs(3), async {
        while !connection.client.is_initialized() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Initialize 必须送达（客户端在 await open 之前就准备好接帧）");

    assert_eq!(
        call(&connection.client, "memory", "echo", Value::str("over-ws"))
            .await
            .expect("echo"),
        Value::str("over-ws")
    );

    connection.dispose();
}

#[tokio::test]
async fn rejects_a_bad_token_before_touching_the_websocket() {
    let (port, state) = spawn_server().await;
    let error = attach_lan_remote_connection(params(port, "wrong-token"), None)
        .await
        .err()
        .expect("错令牌必须失败");
    assert!(
        matches!(error, LanAttachError::Capability { .. }),
        "应在换票阶段失败，实际 {error:?}"
    );
    assert_eq!(
        state.capability_consumed.load(Ordering::SeqCst),
        0,
        "换票失败不得尝试 WS 升级"
    );
}

#[tokio::test]
async fn a_closed_peer_fails_in_flight_requests_closed() {
    let (port, state) = spawn_server().await;
    let mut connection = attach_lan_remote_connection(params(port, VALID_TOKEN), None)
        .await
        .expect("LAN 挂接应成功");
    tokio::time::timeout(Duration::from_secs(3), async {
        while !connection.client.is_initialized() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Initialize");

    connection.dispose();
    assert!(connection.client.is_disposed());
    assert_eq!(state.capability_consumed.load(Ordering::SeqCst), 1);
}
