//! `/ws/host` 行为契约（LT-15 P2 切二）。
//!
//! capability 一次性/TTL、401 拒绝、WS 升级后 RPC 帧往返（Initialize +
//! memory 频道 Promise）。HTTP 语义用 oneshot 直测；WS 用 tokio-tungstenite
//! 真连接。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::{SinkExt, StreamExt};
use rpc_wire::{
    decode_rpc_response, encode_rpc_message, RequestHeader, ResponseHeader, RpcMessage, Value,
};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tower::util::ServiceExt;

use mode_server::server_info::ServerInfoConfig;
use mode_server::ws_host::{
    extract_workspace_path_from_frame, HostCapabilityStore, WsHostDeps,
    MODE_RPC_HOST_CAPABILITY_HEADER,
};
use mode_server::{router, ws_host};

fn test_config() -> ServerInfoConfig {
    ServerInfoConfig::from_parts(&HashMap::new(), Path::new("."), "test")
}

/// 返回 (router, store)：帧桥测试需要与真实 server 共享同一个 capability store
/// （进程内单例语义；POST 端点的 JSON 形状由 issue_endpoint 测试覆盖）。
fn ws_app(now: Arc<dyn Fn() -> Instant + Send + Sync>) -> (axum::Router, Arc<HostCapabilityStore>) {
    let deps = WsHostDeps {
        agent_control: None,
        host_capabilities: Arc::new(HostCapabilityStore::with_injectables(
            Duration::from_millis(30_000),
            now,
            Arc::new(|| format!("cap-{}", uuid::Uuid::new_v4().simple())),
        )),
        data_root: std::env::temp_dir(),
        lan_pairing: None,
    };
    let store = Arc::clone(&deps.host_capabilities);
    (router(test_config()).merge(ws_host::ws_host_router(deps)), store)
}

fn clock_at(start: Instant) -> Arc<dyn Fn() -> Instant + Send + Sync> {
    let current = Arc::new(std::sync::Mutex::new(start));
    Arc::new(move || *current.lock().unwrap())
}

#[tokio::test]
async fn capability_consume_is_single_use_and_ttl_gated() {
    let store = Arc::new(HostCapabilityStore::with_injectables(
        Duration::from_millis(30_000),
        clock_at(Instant::now()),
        Arc::new(|| "cap-fixed".to_string()),
    ));

    let issued = store.issue().await;
    assert_eq!(issued.capability, "cap-fixed");
    assert!(issued.expires_at > 0);
    assert!(store.consume(Some("cap-fixed")).await, "首次消费应成功");
    assert!(!store.consume(Some("cap-fixed")).await, "重放应被拒绝");
    assert!(!store.consume(None).await, "缺 header 应被拒绝");
    assert!(!store.consume(Some("  ")).await, "空串应被拒绝");

    // TTL 过期：签发后快进再消费 → 失败。
    let start = Instant::now();
    let current = Arc::new(std::sync::Mutex::new(start));
    let now = {
        let current = Arc::clone(&current);
        Arc::new(move || *current.lock().unwrap()) as Arc<dyn Fn() -> Instant + Send + Sync>
    };
    let store = Arc::new(HostCapabilityStore::with_injectables(
        Duration::from_millis(30_000),
        now,
        Arc::new(|| "cap-ttl".to_string()),
    ));
    let issued = store.issue().await;
    *current.lock().unwrap() = start + Duration::from_millis(30_001);
    assert!(!store.consume(Some(issued.capability.as_str())).await);
}

async fn issue_capability(app: axum::Router) -> String {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/rpc-host-capability")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["capability"].is_string());
    assert!(body["expiresAt"].is_u64());
    body["capability"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn issue_endpoint_returns_capability_json() {
    let (app, _) = ws_app(clock_at(Instant::now()));
    let capability = issue_capability(app).await;
    assert!(capability.starts_with("cap-"));
}

fn upgrade_request(capability: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri("/ws/host")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header(
            "sec-websocket-key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        );
    if let Some(capability) = capability {
        builder = builder.header(MODE_RPC_HOST_CAPABILITY_HEADER, capability);
    }
    builder.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn ws_host_rejects_without_or_with_replayed_capability() {
    let (app, _) = ws_app(clock_at(Instant::now()));
    let capability = issue_capability(app.clone()).await;

    // 无 header → 401 JSON（不升级）。
    let response = app
        .clone()
        .oneshot(upgrade_request(None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // 带有效 header → 通过 capability 层（oneshot 环境无法完成真正的 WS 升级，
    // extractor 返回 426 Upgrade Required；101 由真实连接测试覆盖）。
    // 关键语义：有效 capability 不被 401 拒绝。
    let response = app
        .clone()
        .oneshot(upgrade_request(Some(&capability)))
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);

    // 同一 capability 已被消费：再次升级 → 401。
    let response = app.oneshot(upgrade_request(Some(&capability))).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// 真实 WS 连接的端到端帧往返：Initialize 先到，memory 频道 Promise 有应答。
#[tokio::test]
async fn ws_host_bridges_rpc_frames_over_real_websocket() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (app, store) = ws_app(clock_at(Instant::now()));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // capability 必须来自同一个真实 server 的 store（进程内单例）。
    let capability = store.issue().await.capability;

    // 无 capability → 握手被 401 拒绝。
    let request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{addr}/ws/host"),
    )
    .expect("request build");
    let refused = tokio_tungstenite::connect_async(request).await;
    assert!(refused.is_err(), "无 capability 应拒绝升级");

    // 带 capability → 101 + Initialize 响应帧先到（deferInit=false）。
    let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{addr}/ws/host"),
    )
    .expect("request build");
    let header_name: axum::http::HeaderName = MODE_RPC_HOST_CAPABILITY_HEADER
        .parse()
        .expect("header name");
    let header_value: axum::http::HeaderValue = capability.parse().expect("header value");
    request.headers_mut().insert(header_name, header_value);
    let (mut ws, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("带 capability 升级应成功");
    assert_eq!(response.status(), 101);

    let initialize = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("initialize in time")
        .expect("socket ok")
        .expect("frame ok");
    let frame = match initialize {
        WsMessage::Binary(data) => data,
        other => panic!("期望 binary 帧，实际 {other:?}"),
    };
    let decoded = decode_rpc_response(&frame).expect("decode");
    assert_eq!(decoded.header, ResponseHeader::Initialize);

    // memory 频道 Promise：listProjectMemories（空 data_root → 空列表应答）。
    let request = encode_rpc_message(&RpcMessage {
        header: RequestHeader::Promise {
            id: 1,
            channel: "memory".into(),
            method: "listProjectMemories".into(),
        },
        body: Value::Array(Vec::new()),
    });
    ws.send(WsMessage::Binary(request.into())).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("response in time")
        .expect("socket ok")
        .expect("frame ok");
    let frame = match response {
        WsMessage::Binary(data) => data,
        other => panic!("期望 binary 帧，实际 {other:?}"),
    };
    let decoded = decode_rpc_response(&frame).expect("decode");
    assert!(
        matches!(decoded.header, ResponseHeader::PromiseSuccess { id: 1 }),
        "期望 PromiseSuccess(id=1)，实际 {:?}",
        decoded.header
    );
}

/// 录制链路 e2e：lan pairing（临时 credential-store）→ WS 连接带 cookie token →
/// 发携带 workspacePath 的 memory 请求 → 客户端记录的最近打开目录更新。
#[tokio::test]
async fn ws_host_records_workspace_path_for_paired_client() {
    use mode_server::lan::lan_pairing_from_store;

    let dir = std::env::temp_dir().join(format!(
        "ws-host-record-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    let pairing = lan_pairing_from_store(&dir, None).expect("pairing from store");
    let pairing_for_clients = std::sync::Arc::clone(&pairing);
    // 造配对码并兑换出 token（真实随机源）。
    let code = pairing.mint_pair_code().await;
    let paired = pairing
        .pair_redeem(&code.code, Some("e2e-client"))
        .await
        .expect("redeem")
        .expect("paired");
    let token = paired.token;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let deps = WsHostDeps {
        agent_control: None,
        host_capabilities: Arc::new(HostCapabilityStore::new()),
        data_root: dir.join("data"),
        lan_pairing: Some(pairing),
    };
    let store = Arc::clone(&deps.host_capabilities);
    let app = router(test_config()).merge(ws_host::ws_host_router(deps));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let capability = store.issue().await.capability;

    let mut request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{addr}/ws/host"),
    )
    .expect("request build");
    let header_name: axum::http::HeaderName = MODE_RPC_HOST_CAPABILITY_HEADER
        .parse()
        .expect("header name");
    let header_value: axum::http::HeaderValue = capability.parse().expect("header value");
    request.headers_mut().insert(header_name, header_value);
    // cookie 带 lite token（升级时解析 clientId）。
    request.headers_mut().insert(
        axum::http::header::COOKIE,
        axum::http::HeaderValue::from_str(&format!("mode_lite_token={token}")).expect("cookie"),
    );
    let (mut ws, _response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("paired upgrade");

    // Initialize。
    let initialize = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("initialize in time")
        .expect("socket ok")
        .expect("frame ok");
    let frame = match initialize {
        WsMessage::Binary(data) => data,
        other => panic!("期望 binary 帧，实际 {other:?}"),
    };
    let _ = rpc_wire::decode_rpc_response(&frame).expect("decode");

    // 发携带 workspacePath 的 memory 请求 → 录制更新。
    // wire body = 位置参数数组，第一参数（对象）顶层带 workspacePath。
    let body = serde_json::json!({ "workspacePath": "D:/work/demo", "limit": 10 });
    let request = rpc_wire::encode_rpc_message(&rpc_wire::RpcMessage {
        header: rpc_wire::RequestHeader::Promise {
            id: 1,
            channel: "memory".into(),
            method: "listProjectMemories".into(),
        },
        body: rpc_wire::Value::Array(vec![rpc_wire::Value::Json(
            body.to_string(),
        )]),
    });
    ws.send(WsMessage::Binary(request.into())).await.unwrap();
    // 应答帧（PromiseSuccess/Error 都行——录制发生在入口，成败不影响记录）。
    let _response = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("response in time");

    // 客户端记录的 last_workspace_path 已更新（从 store 重新 load 验证持久化）。
    let clients = pairing_for_clients.clients().await;
    assert!(
        clients
            .iter()
            .any(|client| client.last_workspace_path.as_deref() == Some("D:/work/demo")),
        "录制应落盘: {clients:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 帧解析单测：只有第一参数对象顶层 workspacePath 才录制。
#[test]
fn workspace_path_extraction_scope() {
    let promise_with = rpc_wire::encode_rpc_message(&rpc_wire::RpcMessage {
        header: rpc_wire::RequestHeader::Promise {
            id: 1,
            channel: "memory".into(),
            method: "listProjectMemories".into(),
        },
        body: rpc_wire::Value::Array(vec![rpc_wire::Value::Json(
            r#"{"workspacePath":"D:/work/demo"}"#.to_string(),
        )]),
    });
    assert_eq!(
        extract_workspace_path_from_frame(&promise_with).as_deref(),
        Some("D:/work/demo")
    );

    // path 字段（readdir/resolvePath）不录。
    let promise_with_path = rpc_wire::encode_rpc_message(&rpc_wire::RpcMessage {
        header: rpc_wire::RequestHeader::Promise {
            id: 2,
            channel: "file".into(),
            method: "readFile".into(),
        },
        body: rpc_wire::Value::Array(vec![rpc_wire::Value::Json(
            r#"{"path":"D:/work/demo/a.txt"}"#.to_string(),
        )]),
    });
    assert_eq!(extract_workspace_path_from_frame(&promise_with_path), None);

    // EventListen 也录。
    let listen = rpc_wire::encode_rpc_message(&rpc_wire::RpcMessage {
        header: rpc_wire::RequestHeader::EventListen {
            id: 3,
            channel: "file-watcher".into(),
            event: "watch".into(),
        },
        body: rpc_wire::Value::Array(vec![rpc_wire::Value::Json(
            r#"{"workspacePath":"D:/work/watch"}"#.to_string(),
        )]),
    });
    assert_eq!(
        extract_workspace_path_from_frame(&listen).as_deref(),
        Some("D:/work/watch")
    );

    // 请求尾帧（PromiseCancel）不解析。
    let cancel = rpc_wire::encode_rpc_message(&rpc_wire::RpcMessage {
        header: rpc_wire::RequestHeader::PromiseCancel { id: 4 },
        body: rpc_wire::Value::Array(vec![rpc_wire::Value::Json(
            r#"{"workspacePath":"D:/work/cancel"}"#.to_string(),
        )]),
    });
    assert_eq!(extract_workspace_path_from_frame(&cancel), None);
}
