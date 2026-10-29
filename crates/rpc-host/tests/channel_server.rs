//! `ChannelServer` 的行为对拍：分派、错误映射、事件、取消、pending 与释放。
//!
//! 语义真源是 `packages/rpc/src/channelServer.ts`；这里用假频道驱动，不依赖产品服务。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, ChannelServer, ServerChannel, Subscription,
};
use rpc_wire::{
    decode_rpc_frame, encode_rpc_message, serialize, RequestHeader, ResponseHeader, RpcFrame,
    RpcMessage, Value,
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

struct TestChannel {
    cancelled: Arc<AtomicBool>,
    disposed: Arc<AtomicBool>,
}

impl TestChannel {
    fn new() -> (Self, Arc<AtomicBool>, Arc<AtomicBool>) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let disposed = Arc::new(AtomicBool::new(false));
        (
            Self {
                cancelled: Arc::clone(&cancelled),
                disposed: Arc::clone(&disposed),
            },
            cancelled,
            disposed,
        )
    }
}

impl ServerChannel for TestChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let cancelled = Arc::clone(&self.cancelled);
        Box::pin(async move {
            match command.as_str() {
                "echo" => Ok(arg),
                "fail" => Err(ChannelError::error("boom")
                    .with_extra("code", Value::Str("E_FAIL".into()))),
                "fail_value" => Err(ChannelError::Value(Value::Str("raw".into()))),
                "slow" => {
                    for _ in 0..100 {
                        if cancel.is_cancelled() {
                            cancelled.store(true, Ordering::SeqCst);
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    Ok(Value::Int(1))
                }
                other => Err(ChannelError::error(format!("unknown command {other}"))),
            }
        })
    }

    fn listen(
        &self,
        _ctx: String,
        _event: String,
        _arg: Value,
        emit: rpc_host::EventCallback,
    ) -> Result<Subscription, ChannelError> {
        let disposed = Arc::clone(&self.disposed);
        tokio::spawn(async move {
            emit(Value::Int(1));
            emit(Value::Int(2));
        });
        Ok(Box::new(move || disposed.store(true, Ordering::SeqCst)))
    }
}

fn promise_frame(id: i32, channel: &str, method: &str, arg: Value) -> Vec<u8> {
    encode_rpc_message(&RpcMessage {
        header: RequestHeader::Promise {
            id,
            channel: channel.into(),
            method: method.into(),
        },
        body: arg,
    })
}

fn listen_frame(id: i32, channel: &str, event: &str) -> Vec<u8> {
    encode_rpc_message(&RpcMessage {
        header: RequestHeader::EventListen {
            id,
            channel: channel.into(),
            event: event.into(),
        },
        body: Value::Undefined,
    })
}

fn cancel_frame(id: i32) -> Vec<u8> {
    encode_rpc_message(&RpcMessage {
        header: RequestHeader::PromiseCancel { id },
        body: Value::Undefined,
    })
}

async fn recv_frame(rx: &mut UnboundedReceiver<Vec<u8>>) -> (ResponseHeader, Value) {
    let bytes = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("等待回帧超时")
        .expect("出站通道已关闭");
    match decode_rpc_frame(&bytes).expect("回帧必须可解码") {
        RpcFrame::Response(response) => (response.header, response.body),
        RpcFrame::Request(_) => panic!("服务端不应发出请求帧"),
    }
}

async fn expect_silence(rx: &mut UnboundedReceiver<Vec<u8>>, millis: u64) {
    assert!(
        tokio::time::timeout(Duration::from_millis(millis), rx.recv())
            .await
            .is_err(),
        "本不该有回帧"
    );
}

fn json_body(body: &Value) -> serde_json::Value {
    match body {
        Value::Json(text) => serde_json::from_str(text).expect("错误回帧 body 必须是合法 JSON"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn sends_initialize_on_construction() {
    let (tx, mut rx) = unbounded_channel();
    let _server = ChannelServer::new(tx, "ctx", Duration::from_millis(200), false);
    let (header, body) = recv_frame(&mut rx).await;
    assert_eq!(header, ResponseHeader::Initialize);
    assert_eq!(body, Value::Undefined);
}

#[tokio::test]
async fn promise_success_echoes_argument() {
    let (tx, mut rx) = unbounded_channel();
    let mut server = ChannelServer::new(tx, "ctx", Duration::from_millis(200), true);
    let (channel, _, _) = TestChannel::new();
    server.register_channel("test", Arc::new(channel));

    server.handle_frame(&promise_frame(5, "test", "echo", Value::Str("hi".into())));

    let (header, body) = recv_frame(&mut rx).await;
    assert_eq!(header, ResponseHeader::PromiseSuccess { id: 5 });
    assert_eq!(body, Value::Str("hi".into()));
}

#[tokio::test]
async fn promise_error_maps_message_name_and_extras() {
    let (tx, mut rx) = unbounded_channel();
    let mut server = ChannelServer::new(tx, "ctx", Duration::from_millis(200), true);
    let (channel, _, _) = TestChannel::new();
    server.register_channel("test", Arc::new(channel));

    server.handle_frame(&promise_frame(6, "test", "fail", Value::Undefined));

    let (header, body) = recv_frame(&mut rx).await;
    assert_eq!(header, ResponseHeader::PromiseError { id: 6 });
    let payload = json_body(&body);
    assert_eq!(payload["message"], "boom");
    assert_eq!(payload["name"], "Error");
    assert_eq!(payload["code"], "E_FAIL");
    assert!(payload.get("stack").is_none(), "stack 未设置时不应写键");
}

#[tokio::test]
async fn promise_error_obj_returns_raw_value() {
    let (tx, mut rx) = unbounded_channel();
    let mut server = ChannelServer::new(tx, "ctx", Duration::from_millis(200), true);
    let (channel, _, _) = TestChannel::new();
    server.register_channel("test", Arc::new(channel));

    server.handle_frame(&promise_frame(7, "test", "fail_value", Value::Undefined));

    let (header, body) = recv_frame(&mut rx).await;
    assert_eq!(header, ResponseHeader::PromiseErrorObj { id: 7 });
    assert_eq!(body, Value::Str("raw".into()));
}

#[tokio::test]
async fn event_listen_fires_each_value_and_dispose_calls_subscription() {
    let (tx, mut rx) = unbounded_channel();
    let mut server = ChannelServer::new(tx, "ctx", Duration::from_millis(200), true);
    let (channel, _, disposed) = TestChannel::new();
    server.register_channel("test", Arc::new(channel));

    server.handle_frame(&listen_frame(9, "test", "onEvent"));

    for expected in [1, 2] {
        let (header, body) = recv_frame(&mut rx).await;
        assert_eq!(header, ResponseHeader::EventFire { id: 9 });
        assert_eq!(body, Value::Int(expected));
    }

    let dispose_frame = encode_rpc_message(&RpcMessage {
        header: RequestHeader::EventDispose { id: 9 },
        body: Value::Undefined,
    });
    server.handle_frame(&dispose_frame);
    assert!(disposed.load(Ordering::SeqCst), "EventDispose 必须调用订阅释放");
}

#[tokio::test]
async fn promise_cancel_marks_token_and_still_returns() {
    let (tx, mut rx) = unbounded_channel();
    let mut server = ChannelServer::new(tx, "ctx", Duration::from_millis(200), true);
    let (channel, cancelled, _) = TestChannel::new();
    server.register_channel("test", Arc::new(channel));

    server.handle_frame(&promise_frame(3, "test", "slow", Value::Undefined));
    tokio::time::sleep(Duration::from_millis(10)).await;
    server.handle_frame(&cancel_frame(3));

    let (header, body) = recv_frame(&mut rx).await;
    assert_eq!(header, ResponseHeader::PromiseSuccess { id: 3 });
    assert_eq!(body, Value::Int(1));
    assert!(cancelled.load(Ordering::SeqCst), "取消令牌必须被置位");
}

#[tokio::test]
async fn unknown_channel_pends_until_registered() {
    let (tx, mut rx) = unbounded_channel();
    let mut server = ChannelServer::new(tx, "ctx", Duration::from_millis(200), true);

    server.handle_frame(&promise_frame(11, "late", "echo", Value::Str("x".into())));
    expect_silence(&mut rx, 20).await;

    let (channel, _, _) = TestChannel::new();
    server.register_channel("late", Arc::new(channel));

    let (header, body) = recv_frame(&mut rx).await;
    assert_eq!(header, ResponseHeader::PromiseSuccess { id: 11 });
    assert_eq!(body, Value::Str("x".into()));
}

#[tokio::test]
async fn unknown_channel_times_out_with_error_payload() {
    let (tx, mut rx) = unbounded_channel();
    let mut server = ChannelServer::new(tx, "ctx", Duration::from_millis(30), true);

    server.handle_frame(&promise_frame(12, "missing", "echo", Value::Undefined));

    let (header, body) = recv_frame(&mut rx).await;
    assert_eq!(header, ResponseHeader::PromiseError { id: 12 });
    let payload = json_body(&body);
    assert_eq!(payload["name"], "Unknown channel");
    assert_eq!(
        payload["message"],
        "Channel name 'missing' timed out after 30ms"
    );
}

#[tokio::test]
async fn dispose_releases_subscriptions() {
    let (tx, mut rx) = unbounded_channel();
    let mut server = ChannelServer::new(tx, "ctx", Duration::from_millis(200), true);
    let (channel, _, disposed) = TestChannel::new();
    server.register_channel("test", Arc::new(channel));

    server.handle_frame(&listen_frame(13, "test", "onEvent"));
    let _ = recv_frame(&mut rx).await;
    server.dispose();
    assert!(disposed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn ignores_frames_that_are_not_requests() {
    let (tx, mut rx) = unbounded_channel();
    let mut server = ChannelServer::new(tx, "ctx", Duration::from_millis(200), true);

    let mut unknown = serialize(&Value::Array(vec![Value::Int(999), Value::Int(1)]));
    unknown.extend(serialize(&Value::Undefined));
    server.handle_frame(&unknown);

    expect_silence(&mut rx, 20).await;
}
