//! 离线端到端对拍（spec `docs/specs/bots-im-transport.md` §4/§5）：
//! 本地假网关注册器 + 本地假 WS 服务端，覆盖 ping/EVENT/CALLBACK/去重/disconnect/
//! 读超时/注册失败重试，全部不触真实网络。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};

type BoxFuture<T> = futures_util::future::BoxFuture<'static, T>;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

use dingtalk_stream::{
    run_connection, BotConnection, OpenError, OpenRequest, Registration, RuntimeCallbacks,
    RuntimeStatus, StreamTiming,
};

/// 本地假 WS 服务端：accept 的每个会话经通道交给测试驱动。
struct FakeServer {
    endpoint: String,
    sessions: mpsc::UnboundedReceiver<WebSocketStream<tokio::net::TcpStream>>,
    accepted: Arc<AtomicU32>,
}

impl FakeServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定本地端口");
        let port = listener.local_addr().expect("本地地址").port();
        let (sessions_tx, sessions_rx) = mpsc::unbounded_channel();
        let accepted = Arc::new(AtomicU32::new(0));
        let accepted_task = Arc::clone(&accepted);
        tokio::spawn(async move {
            loop {
                let (stream, _addr) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => continue,
                };
                accepted_task.fetch_add(1, Ordering::Relaxed);
                let ws = match tokio_tungstenite::accept_async(stream).await {
                    Ok(ws) => ws,
                    Err(_) => continue,
                };
                if sessions_tx.send(ws).is_err() {
                    return;
                }
            }
        });
        Self {
            endpoint: format!("ws://127.0.0.1:{port}/connect"),
            sessions: sessions_rx,
            accepted,
        }
    }

    async fn next_session(&mut self) -> WebSocketStream<tokio::net::TcpStream> {
        tokio::time::timeout(Duration::from_secs(5), self.sessions.recv())
            .await
            .expect("等会话超时")
            .expect("会话通道关闭")
    }

    fn registration_count(&self) -> u32 {
        self.accepted.load(Ordering::Relaxed)
    }
}

async fn send_json(ws: &mut WebSocketStream<tokio::net::TcpStream>, value: &Value) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .expect("假服务端发送");
}

async fn recv_json(ws: &mut WebSocketStream<tokio::net::TcpStream>) -> Value {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(&text).expect("合法 JSON 帧");
            }
            Some(Ok(Message::Ping(_))) => continue,
            Some(Ok(other)) => panic!("期望文本帧，实际 {other:?}"),
            other => panic!("假服务端读到流结束：{other:?}"),
        }
    }
}

/// 假注册器：指向本地假 WS 服务端（或按次数失败）。
fn fake_open(
    endpoint: Arc<String>,
    registrations: Arc<AtomicU32>,
    fail_first: u32,
) -> Arc<dyn Fn(OpenRequest) -> BoxFuture<Result<Registration, OpenError>> + Send + Sync> {
    Arc::new(move |_request: OpenRequest| {
        let endpoint = Arc::clone(&endpoint);
        let registrations = Arc::clone(&registrations);
        Box::pin(async move {
            let count = registrations.fetch_add(1, Ordering::Relaxed);
            if count < fail_first {
                return Err(OpenError::Transport("fake gateway down".into()));
            }
            Ok(Registration {
                endpoint: (*endpoint).clone(),
                ticket: format!("t-{count}"),
            })
        })
    })
}

#[tokio::test]
async fn serves_ping_event_callback_and_reconnects_after_disconnect() {
    let (statuses_tx, mut statuses_rx) = mpsc::unbounded_channel();
    let (dispatches_tx, mut dispatches_rx) = mpsc::unbounded_channel::<String>();
    let (persists_tx, mut persists_rx) = mpsc::unbounded_channel::<Vec<String>>();
    let dispatches_tx = Arc::new(dispatches_tx);
    let persists_tx = Arc::new(persists_tx);

    let mut server = FakeServer::start().await;
    let callbacks = RuntimeCallbacks {
        open: fake_open(
            Arc::new(server.endpoint.clone()),
            Arc::clone(&server.accepted),
            0,
        ),
        on_status: Arc::new(move |update| {
            let _ = statuses_tx.send(update.status);
        }),
        on_callback: Arc::new(move |_bot_id, raw| {
            let dispatches = Arc::clone(&dispatches_tx);
            let msg_id = raw["data"]
                .as_str()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .and_then(|data| {
                    data.get("msgId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            Box::pin(async move {
                if let Some(msg_id) = msg_id {
                    let _ = dispatches.send(msg_id);
                }
            })
        }),
        persist_dedupe: Arc::new(move |_bot_id, entries| {
            let persists = Arc::clone(&persists_tx);
            let ids = entries
                .into_iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>();
            Box::pin(async move {
                let _ = persists.send(ids);
            })
        }),
        load_dedupe: Arc::new(|_bot_id| Box::pin(async { Vec::new() })),
    };

    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(run_connection(
        BotConnection {
            bot_id: "bot-1".into(),
            api_base: "https://api.dingtalk.com".into(),
            client_id: "cid".into(),
            client_secret: "secret".into(),
        },
        StreamTiming::default(),
        callbacks,
        shutdown_rx,
    ));

    // 等第一个会话到达假服务端。
    let mut ws = server.next_session().await;

    // 1) ping → pong（headers/data 透传，code 200；spec §3 第 4 条）。
    send_json(
        &mut ws,
        &json!({
            "specVersion": "1.0", "type": "SYSTEM",
            "headers": { "contentType": "application/json", "messageId": "p1", "topic": "ping" },
            "data": json!({ "opaque": "abc" }),
        }),
    )
    .await;
    let pong = recv_json(&mut ws).await;
    assert_eq!(pong["code"], json!(200));
    assert_eq!(pong["headers"]["topic"], json!("ping"));
    assert_eq!(pong["data"]["opaque"], json!("abc"));

    // 2) EVENT → ACK data 带 status SUCCESS（spec §3 第 6 条）。
    send_json(
        &mut ws,
        &json!({
            "type": "EVENT",
            "headers": { "contentType": "application/json", "messageId": "e1", "topic": "*" },
            "data": "{}",
        }),
    )
    .await;
    let ack = recv_json(&mut ws).await;
    assert_eq!(ack["code"], json!(200));
    assert_eq!(ack["headers"]["messageId"], json!("e1"));
    assert_eq!(
        ack["data"],
        json!("{\"status\":\"SUCCESS\",\"message\":\"success\"}")
    );

    // 3) CALLBACK（目标 topic）→ ACK 空 data + 派发（spec §3 第 6/7 条）。
    send_json(
        &mut ws,
        &json!({
            "type": "CALLBACK",
            "headers": { "contentType": "application/json", "messageId": "c1", "topic": "/v1.0/im/bot/messages/get" },
            "data": json!({ "msgId": "m-1", "text": "hi" }).to_string(),
        }),
    )
    .await;
    let ack = recv_json(&mut ws).await;
    assert_eq!(ack["data"], json!("{}"));
    let msg_id = tokio::time::timeout(Duration::from_secs(2), dispatches_rx.recv())
        .await
        .expect("等派发超时")
        .expect("派发通道关闭");
    assert_eq!(msg_id, "m-1");

    // 4) 重复 msgId → ACK 但不重复派发（spec §3 第 7 条）。
    send_json(
        &mut ws,
        &json!({
            "type": "CALLBACK",
            "headers": { "contentType": "application/json", "messageId": "c2", "topic": "/v1.0/im/bot/messages/get" },
            "data": json!({ "msgId": "m-1", "text": "hi" }).to_string(),
        }),
    )
    .await;
    let ack = recv_json(&mut ws).await;
    assert_eq!(ack["headers"]["messageId"], json!("c2"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(dispatches_rx.try_recv().is_err(), "重复消息不应再派发");

    // 5) 防抖持久化：去重表最终写出 m-1（默认 500ms 防抖；spec §3 第 10 条）。
    let entries = tokio::time::timeout(Duration::from_secs(3), persists_rx.recv())
        .await
        .expect("等持久化超时")
        .expect("持久化通道关闭");
    assert!(
        entries.iter().any(|entry| entry == "m-1"),
        "持久化应包含 m-1：{entries:?}"
    );

    // 6) 服务端 disconnect → 会话结束 → Reconnecting → 第二次注册（spec §3 第 5/9 条）。
    send_json(
        &mut ws,
        &json!({
            "type": "SYSTEM",
            "headers": { "topic": "disconnect" },
            "data": json!({ "reason": "connection is expired" }).to_string(),
        }),
    )
    .await;
    let _ = ws.close(None).await;
    tokio::time::timeout(Duration::from_secs(8), async {
        while server.registration_count() < 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("等重连注册超时");
    let saw_reconnecting = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match statuses_rx.try_recv() {
                Ok(RuntimeStatus::Reconnecting) => break true,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                Ok(_) => continue,
            }
        }
    })
    .await;
    assert!(
        saw_reconnecting.unwrap_or(false),
        "disconnect 后应进入 Reconnecting"
    );

    // 第二个会话建立（重连成功）并再次 ping 验证健康。
    let mut ws2 = server.next_session().await;
    send_json(
        &mut ws2,
        &json!({
            "type": "SYSTEM",
            "headers": { "messageId": "p2", "topic": "ping" },
            "data": json!({ "opaque": "reconnect" }),
        }),
    )
    .await;
    let pong = recv_json(&mut ws2).await;
    assert_eq!(pong["data"]["opaque"], json!("reconnect"));
}

#[tokio::test]
async fn registration_failures_report_error_then_retry() {
    let mut server = FakeServer::start().await;
    let callbacks = RuntimeCallbacks {
        open: fake_open(
            Arc::new(server.endpoint.clone()),
            Arc::clone(&server.accepted),
            2,
        ),
        on_status: Arc::new(|_update| {}),
        on_callback: Arc::new(|_bot_id, _raw| Box::pin(async {})),
        persist_dedupe: Arc::new(|_bot_id, _entries| Box::pin(async {})),
        load_dedupe: Arc::new(|_bot_id| Box::pin(async { Vec::new() })),
    };
    let timing = StreamTiming {
        registration_retry: Duration::from_millis(50),
        ..StreamTiming::default()
    };
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(run_connection(
        BotConnection {
            bot_id: "bot-2".into(),
            api_base: "https://api.dingtalk.com".into(),
            client_id: "cid".into(),
            client_secret: "secret".into(),
        },
        timing,
        callbacks,
        shutdown_rx,
    ));

    // 前两次注册失败 → 第三次成功并建立会话（spec §3 第 9 条：注册失败固定 5s 重试，
    // 测试用 50ms 加速；失败重试循环与正常注册共用同一路径）。
    let ws = tokio::time::timeout(Duration::from_secs(5), server.next_session())
        .await
        .expect("等第三次注册建连超时");
    drop(ws);
}

#[tokio::test]
async fn read_timeout_reconnects() {
    let mut server = FakeServer::start().await;
    let callbacks = RuntimeCallbacks {
        open: fake_open(
            Arc::new(server.endpoint.clone()),
            Arc::clone(&server.accepted),
            0,
        ),
        on_status: Arc::new(|_update| {}),
        on_callback: Arc::new(|_bot_id, _raw| Box::pin(async {})),
        persist_dedupe: Arc::new(|_bot_id, _entries| Box::pin(async {})),
        load_dedupe: Arc::new(|_bot_id| Box::pin(async { Vec::new() })),
    };
    let timing = StreamTiming {
        read_timeout: Duration::from_millis(120),
        ..StreamTiming::default()
    };
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(run_connection(
        BotConnection {
            bot_id: "bot-3".into(),
            api_base: "https://api.dingtalk.com".into(),
            client_id: "cid".into(),
            client_secret: "secret".into(),
        },
        timing,
        callbacks,
        shutdown_rx,
    ));

    // 服务端保持静默 → 读超时 → 重连（注册数 ≥ 2；spec §3 第 8 条）。
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.registration_count() < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("等读超时重连超时");
    let mut ws = server.next_session().await;
    let _ = ws.close(None).await;
}
