//! 离线端到端对拍（spec `docs/specs/bots-im-transport.md` §4c/§5）：
//! 本地假网关 + 本地假 WS 服务端，覆盖注册参数、首个 ping、pong 参数更新、
//! 分片事件合并派发与 ACK 回帧、非 event 帧不回 ACK、断线重连。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

use feishu_ws::{
    decode, encode, header_value, run_connection, EventDispatcher, FeishuConfig, FeishuDomain,
    Frame, Header, PullConfig, RuntimeTiming, SessionCallbacks, SessionParams, SessionTiming,
};

/// 让测试不依赖 reqwest：直接用闭包实现 PullConfig 需要一个类型——这里用 shim。
struct FakePull {
    endpoint: String,
    pulls: Arc<AtomicU32>,
    ping_interval: Duration,
}

impl PullConfig for FakePull {
    fn pull(
        &self,
        _config: &FeishuConfig,
    ) -> futures_util::future::BoxFuture<'static, Result<SessionParams, feishu_ws::GatewayError>>
    {
        let endpoint = self.endpoint.clone();
        let pulls = Arc::clone(&self.pulls);
        let ping_interval = self.ping_interval;
        Box::pin(async move {
            pulls.fetch_add(1, Ordering::Relaxed);
            Ok(SessionParams {
                connect_url: endpoint,
                device_id: "device-1".into(),
                service_id: "3".into(),
                ping_interval,
                reconnect_count: -1,
                reconnect_interval: Duration::from_millis(50),
                reconnect_nonce: Duration::ZERO,
            })
        })
    }
}

struct FakeServer {
    endpoint: String,
    sessions: mpsc::UnboundedReceiver<WebSocketStream<tokio::net::TcpStream>>,
}

impl FakeServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定端口");
        let port = listener.local_addr().expect("本地地址").port();
        let (sessions_tx, sessions_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => continue,
                };
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
            endpoint: format!("ws://127.0.0.1:{port}/connect?device_id=device-1&service_id=3"),
            sessions: sessions_rx,
        }
    }

    async fn next_session(&mut self) -> WebSocketStream<tokio::net::TcpStream> {
        tokio::time::timeout(Duration::from_secs(5), self.sessions.recv())
            .await
            .expect("等会话超时")
            .expect("会话通道关闭")
    }
}

async fn send_frame(ws: &mut WebSocketStream<tokio::net::TcpStream>, frame: &Frame) {
    ws.send(Message::Binary(encode(frame).into()))
        .await
        .expect("假服务端发送");
}

async fn recv_frame(ws: &mut WebSocketStream<tokio::net::TcpStream>) -> Frame {
    loop {
        match ws.next().await {
            Some(Ok(Message::Binary(bytes))) => return decode(&bytes).expect("可解码帧"),
            Some(Ok(Message::Ping(_))) => continue,
            Some(Ok(other)) => panic!("期望二进制帧，实际 {other:?}"),
            other => panic!("假服务端读到流结束：{other:?}"),
        }
    }
}

fn event_frame(message_id: &str, sum: &str, seq: &str, payload: &[u8]) -> Frame {
    Frame {
        seq_id: 0,
        log_id: 0,
        service: 3,
        method: 1, // data
        headers: vec![
            Header {
                key: "type".into(),
                value: "event".into(),
            },
            Header {
                key: "message_id".into(),
                value: message_id.into(),
            },
            Header {
                key: "sum".into(),
                value: sum.into(),
            },
            Header {
                key: "seq".into(),
                value: seq.into(),
            },
            Header {
                key: "trace_id".into(),
                value: "trace-1".into(),
            },
        ],
        payload_encoding: None,
        payload_type: None,
        payload: Some(payload.to_vec()),
        log_id_new: None,
    }
}

/// 收到客户端帧后跳过期间的 ping（客户端每 ping_interval 发一次），返回第一个非 ping 帧。
async fn recv_non_ping(ws: &mut WebSocketStream<tokio::net::TcpStream>) -> Frame {
    for _ in 0..20 {
        let frame = recv_frame(ws).await;
        if frame.method == 0 && header_value(&frame, "type") == Some("ping") {
            continue;
        }
        return frame;
    }
    panic!("连续 20 帧都是 ping");
}

#[tokio::test]
async fn pings_merges_events_and_acks_but_ignores_non_events() {
    let mut server = FakeServer::start().await;
    let pulls = Arc::new(AtomicU32::new(0));
    let pull = Arc::new(FakePull {
        endpoint: server.endpoint.clone(),
        pulls: Arc::clone(&pulls),
        ping_interval: Duration::from_millis(80),
    });

    let (dispatched_tx, mut dispatched_rx) = mpsc::unbounded_channel::<serde_json::Value>();
    let on_event: EventDispatcher = Arc::new(move |event| {
        let tx = dispatched_tx.clone();
        Box::pin(async move {
            tx.send(event).expect("派发通道");
            Ok(None)
        })
    });
    let callbacks = SessionCallbacks { on_event };
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(run_connection(
        pull,
        FeishuConfig {
            app_id: "app".into(),
            app_secret: "secret".into(),
            domain: FeishuDomain::Feishu,
        },
        callbacks,
        RuntimeTiming {
            session: SessionTiming::default(),
            defaults: SessionParams::default(),
        },
        shutdown_rx,
    ));

    let mut ws = server.next_session().await;

    // 1) 建连后立即收到首个 ping：control 帧、service=3、SeqID=0（spec §5.3 第 1 条）。
    let ping = recv_frame(&mut ws).await;
    assert_eq!(ping.method, 0);
    assert_eq!(header_value(&ping, "type"), Some("ping"));
    assert_eq!(ping.service, 3);
    assert_eq!(ping.seq_id, 0);

    // 2) pong 更新会话参数（不改变 wire 形状，仅确认不崩且继续工作）。
    send_frame(
        &mut ws,
        &Frame {
            seq_id: 0,
            log_id: 0,
            service: 3,
            method: 0,
            headers: vec![Header {
                key: "type".into(),
                value: "pong".into(),
            }],
            payload_encoding: None,
            payload_type: None,
            payload: Some(
                serde_json::to_vec(&json!({
                    "PingInterval": 0.2,
                    "ReconnectCount": -1,
                    "ReconnectInterval": 0.05,
                    "ReconnectNonce": 0
                }))
                .unwrap(),
            ),
            log_id_new: None,
        },
    )
    .await;

    // 3) 分片事件（sum=2，乱序 seq=1 先到）→ 合并派发 + ACK（复用原帧 + biz_rt）。
    let body = br#"{"schema":"2.0","header":{"event_type":"im.message.receive_v1"},"event":{"message":{"message_id":"om_1"}}}"#;
    let split = 20;
    send_frame(&mut ws, &event_frame("m-1", "2", "1", &body[split..])).await;
    // 未齐不派发
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(dispatched_rx.try_recv().is_err(), "分片未齐不应派发");
    send_frame(&mut ws, &event_frame("m-1", "2", "0", &body[..split])).await;

    let dispatched = tokio::time::timeout(Duration::from_secs(2), dispatched_rx.recv())
        .await
        .expect("等派发超时")
        .expect("派发通道关闭");
    assert_eq!(
        dispatched["header"]["event_type"],
        json!("im.message.receive_v1")
    );

    // ACK：method 与原帧一致（data=1），带 biz_rt，payload = {"code":200}。
    let ack = recv_non_ping(&mut ws).await;
    assert_eq!(ack.method, 1);
    assert!(header_value(&ack, "biz_rt").is_some(), "ACK 必须带 biz_rt");
    let ack_payload: serde_json::Value =
        serde_json::from_slice(ack.payload.as_deref().expect("ACK payload")).expect("ACK JSON");
    assert_eq!(ack_payload["code"], json!(200));

    // 4) 非 event 帧（type=card）不回 ACK（SDK :85550-85556）。
    send_frame(
        &mut ws,
        &Frame {
            seq_id: 0,
            log_id: 0,
            service: 3,
            method: 1,
            headers: vec![
                Header {
                    key: "type".into(),
                    value: "card".into(),
                },
                Header {
                    key: "message_id".into(),
                    value: "m-2".into(),
                },
                Header {
                    key: "sum".into(),
                    value: "1".into(),
                },
                Header {
                    key: "seq".into(),
                    value: "0".into(),
                },
            ],
            payload_encoding: None,
            payload_type: None,
            payload: Some(b"{}".to_vec()),
            log_id_new: None,
        },
    )
    .await;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                let frame = decode(&bytes).expect("可解码帧");
                if header_value(&frame, "type") == Some("ping") {
                    continue;
                }
                saw_ack = true;
            }
            _ => break,
        }
    }
    assert!(!saw_ack, "card 帧不应收到 ACK");

    // 5) 服务端断开 → 重连（注册次数 ≥ 2，出现第二个会话）。
    let _ = ws.close(None).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while pulls.load(Ordering::Relaxed) < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("等重连注册超时");
    let mut ws2 = server.next_session().await;
    let ping = recv_frame(&mut ws2).await;
    assert_eq!(header_value(&ping, "type"), Some("ping"));
}
