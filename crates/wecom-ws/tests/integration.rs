//! 离线端到端对拍（spec `docs/specs/bots-im-transport.md` §4d/§6）：
//! 本地假 WS 服务端覆盖认证、心跳、入站派发、串行回复与 ack、非阻塞流式、
//! 三步分片上传、断开重连、disconnected_event 不重连。

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};

use wecom_ws::{
    generate_req_id, outbound_frame, ConnectionCallbacks, Frame, RunOutcome, StatusUpdate,
    WeComClient, WeComConfig,
};

type Ws = WebSocketStream<tokio::net::TcpStream>;

struct FakeServer {
    port: u16,
    sessions: mpsc::UnboundedReceiver<Ws>,
}

impl FakeServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定端口");
        let port = listener.local_addr().expect("本地地址").port();
        let (tx, rx) = mpsc::unbounded_channel();
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
                if tx.send(ws).is_err() {
                    return;
                }
            }
        });
        Self { port, sessions: rx }
    }

    async fn next_session(&mut self) -> Ws {
        tokio::time::timeout(Duration::from_secs(5), self.sessions.recv())
            .await
            .expect("等会话超时")
            .expect("会话通道关闭")
    }
}

fn config(port: u16) -> WeComConfig {
    WeComConfig {
        ws_url: format!("ws://127.0.0.1:{port}"),
        // 默认 2s：普通用例不依赖心跳，避免未 ack 的 ping 触发死连接判定。
        heartbeat_interval: Duration::from_secs(2),
        reconnect_base: Duration::from_millis(30),
        max_reconnect_attempts: -1,
        max_auth_failure_attempts: 5,
        ..WeComConfig::new("bot-1", "secret-1")
    }
}

fn fast_heartbeat_config(port: u16) -> WeComConfig {
    WeComConfig {
        heartbeat_interval: Duration::from_millis(100),
        ..config(port)
    }
}

fn callbacks(frame_tx: mpsc::UnboundedSender<Frame>) -> ConnectionCallbacks {
    ConnectionCallbacks {
        on_frame: Arc::new(move |frame| {
            let tx = frame_tx.clone();
            Box::pin(async move {
                let _ = tx.send(frame);
            })
        }),
        on_status: Arc::new(|_update: StatusUpdate| {}),
    }
}

async fn read_frame(ws: &mut Ws) -> Frame {
    loop {
        match ws.next().await {
            Some(Ok(Message::Text(text))) => {
                return serde_json::from_str(&text).expect("合法 JSON 帧");
            }
            Some(Ok(Message::Ping(_))) => continue,
            other => panic!("假服务端读到非文本帧：{other:?}"),
        }
    }
}

async fn send_json(ws: &mut Ws, value: Value) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .expect("假服务端发送");
}

async fn ack(ws: &mut Ws, req_id: &str, body: Option<Value>) {
    let mut value = json!({ "headers": { "req_id": req_id }, "errcode": 0, "errmsg": "ok" });
    if let Some(body) = body {
        value["body"] = body;
    }
    send_json(ws, value).await;
}

/// 等待认证帧并回成功；返回认证 req_id。
async fn authenticate(ws: &mut Ws) -> String {
    let auth = read_frame(ws).await;
    assert_eq!(auth.cmd.as_deref(), Some("aibot_subscribe"));
    assert_eq!(auth.body.as_ref().unwrap()["bot_id"], json!("bot-1"));
    assert_eq!(auth.body.as_ref().unwrap()["secret"], json!("secret-1"));
    let req_id = auth.req_id().to_string();
    ack(ws, &req_id, None).await;
    req_id
}

/// 跳过心跳帧，等待 `cmd` 匹配的帧（期望的就是 ping 时直接返回）。
async fn read_cmd(ws: &mut Ws, cmd: &str) -> Frame {
    for _ in 0..50 {
        let frame = read_frame(ws).await;
        if frame.cmd.as_deref() == Some(cmd) {
            return frame;
        }
        if frame.cmd.as_deref() == Some("ping") {
            ack(ws, frame.req_id(), None).await;
            continue;
        }
        assert_eq!(frame.cmd.as_deref(), Some(cmd), "意外的 cmd");
    }
    panic!("等 {cmd} 超时");
}

#[tokio::test]
async fn authenticates_heartbeats_and_dispatches_inbound() {
    let mut server = FakeServer::start().await;
    let (frame_tx, mut frame_rx) = mpsc::unbounded_channel();
    let (client, task) =
        WeComClient::start(fast_heartbeat_config(server.port), callbacks(frame_tx));
    let mut ws = server.next_session().await;
    authenticate(&mut ws).await;

    // 心跳：短间隔配置下应收到 ping 帧，ack 后保持连接。
    let ping = read_cmd(&mut ws, "ping").await;
    assert!(ping.req_id().starts_with("ping_"));

    // 入站消息：aibot_msg_callback → on_frame。
    send_json(
        &mut ws,
        json!({
            "cmd": "aibot_msg_callback",
            "headers": { "req_id": "cb-1" },
            "body": { "msgid": "m-1", "msgtype": "text", "text": { "content": "hi" } },
        }),
    )
    .await;
    let frame = tokio::time::timeout(Duration::from_secs(2), frame_rx.recv())
        .await
        .expect("等派发超时")
        .expect("派发通道关闭");
    assert_eq!(frame.cmd.as_deref(), Some("aibot_msg_callback"));
    assert_eq!(frame.req_id(), "cb-1");

    client.disconnect();
    let _ = task.await;
}

#[tokio::test]
async fn serial_replies_with_ack_and_non_blocking_streaming() {
    let mut server = FakeServer::start().await;
    let (frame_tx, _frame_rx) = mpsc::unbounded_channel();
    let (client, task) = WeComClient::start(config(server.port), callbacks(frame_tx));
    let mut ws = server.next_session().await;
    authenticate(&mut ws).await;

    // 第一条流式：不立刻 ack，制造 pending。
    let first = {
        let client = client.clone();
        tokio::spawn(async move {
            client
                .reply_stream("cb-2", "stream-1", "partial", false, None, None)
                .await
        })
    };
    let sent = read_cmd(&mut ws, "aibot_respond_msg").await;
    assert_eq!(sent.req_id(), "cb-2");
    assert_eq!(sent.body.as_ref().unwrap()["msgtype"], json!("stream"));

    // 非阻塞中间帧：pending 未 ack → 跳过。
    let skipped = client
        .reply_stream_non_blocking("cb-2", "stream-1", "more", false, None, None)
        .await
        .expect("跳过不是错误");
    assert!(skipped.is_none(), "pending 时中间帧应跳过");

    // finish=true 必须排队（等 ack 后才发）。
    let finish = {
        let client = client.clone();
        tokio::spawn(async move {
            client
                .reply_stream_non_blocking("cb-2", "stream-1", "done", true, None, None)
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    // 队列里还没有第二条出站：服务端下一帧只可能是心跳（2s 间隔下通常没有）。
    // 为不阻塞，这里直接 ack 首帧验证顺序推进。
    ack(&mut ws, "cb-2", Some(json!({}))).await;
    assert!(first.await.expect("任务").is_ok(), "首帧 ack 后 resolve");
    let finish_frame = read_cmd(&mut ws, "aibot_respond_msg").await;
    assert_eq!(
        finish_frame.body.as_ref().unwrap()["stream"]["finish"],
        json!(true)
    );
    ack(&mut ws, "cb-2", Some(json!({}))).await;
    let result = finish.await.expect("任务").expect("终帧成功");
    assert!(result.is_some());

    client.disconnect();
    let _ = task.await;
}

#[tokio::test]
async fn uploads_media_in_three_steps() {
    let mut server = FakeServer::start().await;
    let (frame_tx, _frame_rx) = mpsc::unbounded_channel();
    let (client, task) = WeComClient::start(config(server.port), callbacks(frame_tx));
    let mut ws = server.next_session().await;
    authenticate(&mut ws).await;

    let upload = {
        let client = client.clone();
        tokio::spawn(async move {
            client
                .upload_media(b"hello wecom".to_vec(), "image", "a.png")
                .await
        })
    };

    let init = read_cmd(&mut ws, "aibot_upload_media_init").await;
    assert_eq!(init.body.as_ref().unwrap()["total_chunks"], json!(1));
    assert_eq!(
        init.body.as_ref().unwrap()["md5"],
        json!("83b57178e9c49925c9ff8f16722ac2ad")
    );
    ack(&mut ws, init.req_id(), Some(json!({ "upload_id": "u-1" }))).await;

    let chunk = read_cmd(&mut ws, "aibot_upload_media_chunk").await;
    assert_eq!(chunk.body.as_ref().unwrap()["chunk_index"], json!(0));
    assert_eq!(chunk.body.as_ref().unwrap()["upload_id"], json!("u-1"));
    assert!(chunk.body.as_ref().unwrap()["base64_data"].is_string());
    ack(&mut ws, chunk.req_id(), None).await;

    let finish = read_cmd(&mut ws, "aibot_upload_media_finish").await;
    assert_eq!(finish.body.as_ref().unwrap()["upload_id"], json!("u-1"));
    ack(
        &mut ws,
        finish.req_id(),
        Some(json!({ "media_id": "media-1", "type": "image", "created_at": "t" })),
    )
    .await;

    let result = upload.await.expect("任务").expect("上传成功");
    assert_eq!(result.media_id, "media-1");
    assert_eq!(result.media_type, "image");

    client.disconnect();
    let _ = task.await;
}

#[tokio::test]
async fn reconnects_after_close_but_not_on_disconnected_event() {
    // (a) 服务端关闭 → 重连并重新认证。
    let mut server = FakeServer::start().await;
    let (frame_tx, _frame_rx) = mpsc::unbounded_channel();
    let (client, task) = WeComClient::start(config(server.port), callbacks(frame_tx));
    let mut ws = server.next_session().await;
    authenticate(&mut ws).await;
    let _ = ws.close(None).await;
    let mut ws2 = server.next_session().await;
    authenticate(&mut ws2).await;

    // (b) disconnected_event → 任务以 ServerDisconnected 结束且不再重连。
    send_json(
        &mut ws2,
        json!({
            "cmd": "aibot_event_callback",
            "headers": { "req_id": "ev-1" },
            "body": { "event": { "eventtype": "disconnected_event" } },
        }),
    )
    .await;
    let outcome = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("等任务结束超时")
        .expect("任务不可 panic");
    assert_eq!(outcome, RunOutcome::ServerDisconnected);
    // 无第二个会话：在超时窗口内拿不到新连接。
    assert!(
        tokio::time::timeout(Duration::from_millis(300), server.next_session())
            .await
            .is_err(),
        "disconnected_event 后不应重连"
    );
    let _ = client;
}

#[tokio::test]
async fn request_ids_are_unique_across_generated_frames() {
    // 生成器冒烟：同一毫秒内也不重复（随机后缀）。
    let ids: Vec<String> = (0..32)
        .map(|_| generate_req_id("aibot_send_msg", 1_700_000_000_000))
        .collect();
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len());
    let frame = outbound_frame("aibot_send_msg", &ids[0], json!({"chatid":"c"}));
    assert_eq!(frame.req_id(), ids[0]);
}
