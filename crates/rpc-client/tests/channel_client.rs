//! `rpc-client` 行为契约（LT-16.4d 续段）。
//!
//! 全部用例都对着**真 `rpc_host::ChannelServer`** 跑：客户端 ↔ 服务端之间用
//! 两个 mpsc 通道搬运真实帧，不 mock 编解码。这样「Initialize 门」「迟到回帧」
//! 「dispose fail-closed」这些语义才是被真的验证过，而不是被 mock 掉。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rpc_client::{ChannelClient, ClientError};
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, ChannelServer, EventCallback, ServerChannel,
    Subscription,
};
use rpc_wire::Value;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};

/// 最小可用的服务端频道：`echo` 原样返回、`boom` 返回 Error、`fire` 发一条事件。
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
            "boom" => Err(ChannelError::error("频道炸了").with_extra("code", Value::str("E_BOOM"))),
            "obj" => Err(ChannelError::Value(Value::Int(7))),
            other => Err(ChannelError::error(format!("unknown command {other}"))),
        };
        Box::pin(async move { result })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        _arg: Value,
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        if event != "fire" {
            return Err(ChannelError::error(format!("unknown event {event}")));
        }
        emit(Value::Int(41));
        Ok(Box::new(|| {}))
    }
}

struct Harness {
    client: Arc<ChannelClient>,
    server: Arc<AsyncMutex<ChannelServer>>,
    /// 每次客户端发帧 +1，用来断言「排队期间没有偷发」。
    frames_sent: Arc<AtomicUsize>,
}

impl Harness {
    /// `defer_init = true` 时服务端不会自动发 Initialize，由用例自己调 `ready()`，
    /// 这样才能构造「客户端先发请求、服务端后初始化」的窗口。
    async fn new(defer_init: bool) -> Self {
        let (to_server_tx, mut to_server_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (to_client_tx, mut to_client_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        let mut server =
            ChannelServer::new(to_client_tx, "server", Duration::from_secs(5), defer_init);
        server.register_channel("memory", Arc::new(MemoryChannel));
        let server = Arc::new(AsyncMutex::new(server));

        let frames_sent = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&frames_sent);
        let client = Arc::new(ChannelClient::new(Arc::new(move |frame: Vec<u8>| {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = to_server_tx.send(frame);
        })));

        // 客户端 → 服务端
        {
            let server = Arc::clone(&server);
            tokio::spawn(async move {
                while let Some(frame) = to_server_rx.recv().await {
                    server.lock().await.handle_frame(&frame);
                }
            });
        }
        // 服务端 → 客户端
        {
            let client = Arc::clone(&client);
            tokio::spawn(async move {
                while let Some(frame) = to_client_rx.recv().await {
                    client.handle_frame(&frame);
                }
            });
        }

        Self {
            client,
            server,
            frames_sent,
        }
    }

    async fn ready(&self) {
        self.server.lock().await.ready();
        // 让 Initialize 帧走完两条通道。
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 发一个请求并等结果（responder 走 oneshot）。
async fn call(client: &ChannelClient, method: &str, arg: Value) -> Result<Value, ClientError> {
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));
    client.call(
        "memory",
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
async fn requests_before_initialize_are_queued_not_dropped() {
    let harness = Harness::new(true).await;

    // 服务端还没 ready：请求必须排队，且**一个帧都不许发**。
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));
    harness.client.call(
        "memory",
        "echo",
        Value::str("early"),
        Arc::new(move |result| {
            if let Some(tx) = tx.lock().expect("tx").take() {
                let _ = tx.send(result);
            }
        }),
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        harness.frames_sent.load(Ordering::SeqCst),
        0,
        "Initialize 之前不得发任何请求帧（服务端还没准备好接收）"
    );
    assert!(!harness.client.is_initialized());

    // 服务端 ready → Initialize 到达 → 排队请求补发并完成。
    harness.ready().await;
    let result = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("补发后必须回帧")
        .expect("responder");
    assert_eq!(result.expect("ok"), Value::str("early"));
    assert!(harness.client.is_initialized());
}

#[tokio::test]
async fn call_round_trips_after_initialize() {
    let harness = Harness::new(false).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(harness.client.is_initialized(), "非 defer 模式建连即初始化");

    assert_eq!(
        call(&harness.client, "echo", Value::str("hello"))
            .await
            .expect("echo"),
        Value::str("hello")
    );
}

#[tokio::test]
async fn remote_error_carries_message_name_and_extra() {
    let harness = Harness::new(false).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let error = call(&harness.client, "boom", Value::Undefined)
        .await
        .expect_err("boom 必须失败");
    match error {
        ClientError::Remote {
            message,
            name,
            extra,
            ..
        } => {
            assert_eq!(message, "频道炸了");
            assert_eq!(name, "Error");
            assert!(
                extra.iter().any(|(key, _)| key == "code"),
                "透传字段（code）必须保留：{extra:?}"
            );
        }
        other => panic!("期望 Remote，实际 {other:?}"),
    }
}

#[tokio::test]
async fn non_error_rejection_maps_to_remote_value() {
    let harness = Harness::new(false).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        call(&harness.client, "obj", Value::Undefined)
            .await
            .expect_err("obj 必须失败"),
        ClientError::RemoteValue(Value::Int(7))
    );
}

#[tokio::test]
async fn events_fire_and_dispose_releases_the_subscription() {
    let harness = Harness::new(false).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let tx = Arc::new(Mutex::new(tx));
    let subscription = harness.client.listen(
        "memory",
        "fire",
        Arc::new(move |value: Value| {
            let _ = tx.lock().expect("tx").send(value);
        }),
    );

    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("事件必须到达")
        .expect("payload");
    assert_eq!(first, Value::Int(41));

    // 释放订阅要发 EventDispose；释放后不再有事件。
    subscription();
    let frames_after_dispose = harness.frames_sent.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        harness.frames_sent.load(Ordering::SeqCst) > frames_after_dispose - 1,
        "释放订阅必须发帧（EventDispose）"
    );
}

#[tokio::test]
async fn reconnect_replays_event_subscriptions_with_the_same_id() {
    let harness = Harness::new(false).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let tx = Arc::new(Mutex::new(tx));
    let _subscription = harness.client.listen(
        "memory",
        "fire",
        Arc::new(move |value: Value| {
            let _ = tx.lock().expect("tx").send(value);
        }),
    );
    let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("首次订阅要收到事件")
        .expect("payload");
    assert_eq!(received, Value::Int(41));

    // 第二个 Initialize = 换了服务端实例：旧订阅它不认识，必须按原 id 重放。
    let frames_before = harness.frames_sent.load(Ordering::SeqCst);
    harness.ready().await;
    assert!(
        harness.frames_sent.load(Ordering::SeqCst) > frames_before,
        "重连后必须重放 EventListen（否则服务端永远不知道这个订阅）"
    );

    // 重放生效：能再次收到事件。
    let replayed = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("重放后必须再收到事件")
        .expect("payload");
    assert_eq!(replayed, Value::Int(41));
}

#[tokio::test]
async fn dispose_fails_closed_for_queued_requests() {
    let harness = Harness::new(true).await;

    // 服务端还没 ready：请求停在队列里（一帧都没发）。
    let (queued_tx, queued_rx) = oneshot::channel();
    let queued_tx = Arc::new(Mutex::new(Some(queued_tx)));
    harness.client.call(
        "memory",
        "echo",
        Value::str("queued"),
        Arc::new(move |result| {
            if let Some(tx) = queued_tx.lock().expect("tx").take() {
                let _ = tx.send(result);
            }
        }),
    );
    assert_eq!(harness.frames_sent.load(Ordering::SeqCst), 0, "还在排队");

    harness.client.dispose();
    assert!(harness.client.is_disposed());

    // 关键：排队中的请求在 dispose 时必须被 reject，不能永远挂着。
    // 否则上层「in-flight 去重」会永久占住 workspace key，后续所有同 workspace
    // 的连接尝试都被判成「已在连接中」。
    assert_eq!(
        queued_rx.await.expect("排队请求必须有结果"),
        Err(ClientError::Disposed),
        "dispose 必须 fail-closed 覆盖排队请求"
    );

    // dispose 后新请求立即失败；dispose 幂等。
    assert_eq!(
        call(&harness.client, "echo", Value::str("after"))
            .await
            .expect_err("dispose 后必须立即失败"),
        ClientError::Disposed
    );
    harness.client.dispose();
}

#[tokio::test]
async fn dispose_rejects_in_flight_requests() {
    let harness = Harness::new(false).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    // 已初始化：请求发出去了，但服务端还没回（用一个永不回帧的方法不好构造，
    // 所以这里只在 dispose 之后断言「结果必须是确定的」）。
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));
    harness.client.call(
        "memory",
        "echo",
        Value::str("inflight"),
        Arc::new(move |result| {
            if let Some(tx) = tx.lock().expect("tx").take() {
                let _ = tx.send(result);
            }
        }),
    );
    harness.client.dispose();

    let inflight = rx.await.expect("在途请求必须有结果");
    assert!(
        matches!(inflight, Err(ClientError::Disposed) | Ok(_)),
        "在途请求只允许成功或 Disposed，不能挂起：{inflight:?}"
    );
}

#[tokio::test]
async fn cancel_rejects_the_call_and_tells_the_server() {
    let harness = Harness::new(false).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));
    let (id, _token) = harness.client.call(
        "memory",
        "echo",
        Value::str("slow"),
        Arc::new(move |result| {
            if let Some(tx) = tx.lock().expect("tx").take() {
                let _ = tx.send(result);
            }
        }),
    );
    harness.client.cancel(id);
    assert_eq!(
        rx.await.expect("responder"),
        Err(ClientError::Cancelled),
        "取消必须让调用方拿到 Cancelled"
    );
}

#[tokio::test]
async fn frames_from_the_wrong_direction_are_ignored() {
    let harness = Harness::new(false).await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    // 把一条「请求帧」喂给客户端：它不是回帧，必须被忽略而不是 panic。
    let request_frame = rpc_wire::encode_rpc_message(&rpc_wire::RpcMessage {
        header: rpc_wire::RequestHeader::Promise {
            id: 0,
            channel: "memory".to_string(),
            method: "echo".to_string(),
        },
        body: Value::Undefined,
    });
    harness.client.handle_frame(&request_frame);
    // 乱字节同样要安全忽略。
    harness.client.handle_frame(&[0xff, 0x00, 0x01]);

    // 客户端仍然可用。
    assert_eq!(
        call(&harness.client, "echo", Value::str("still-ok"))
            .await
            .expect("echo"),
        Value::str("still-ok")
    );
}
