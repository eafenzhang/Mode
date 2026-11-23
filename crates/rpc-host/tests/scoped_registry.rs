//! scoped 服务面登记表契约（LT-16.4d 底座）。
//!
//! 真源 `packages/desktop/src/host-standalone/scopedAttachments.ts`。四条语义：
//! attach 不 ready、start 才发 Initialize、detach 回收并通知、register 失败不留悬空 id。
//!
//! 全部用**真 `ChannelServer`**：帧是真的编码/解码过的，转发器也是真的跑在 tokio 上。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, ChannelServer, EventCallback, ScopedServerRegistry,
    ServerChannel, Subscription,
};
use rpc_wire::{
    decode_rpc_response, decode_scope_frame, encode_rpc_message, encode_scoped_frame,
    RequestHeader, ResponseHeader, RpcMessage, Value,
};

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
        _emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        Ok(Box::new(|| {}))
    }
}

/// 收集出站帧（已是带 scope 前缀的完整帧）。
type FrameBuf = Arc<Mutex<Vec<Vec<u8>>>>;
type SinkFn = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

fn sink() -> (FrameBuf, SinkFn) {
    let frames = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::clone(&frames);
    let sink: SinkFn = Arc::new(move |frame| {
        store.lock().expect("frames").push(frame);
    });
    (frames, sink)
}

fn request_frame(id: i32, channel: &str, method: &str, arg: Value) -> Vec<u8> {
    encode_rpc_message(&RpcMessage {
        header: RequestHeader::Promise {
            id,
            channel: channel.to_string(),
            method: method.to_string(),
        },
        body: arg,
    })
}

/// 等转发器把帧送到 sink（tokio 任务，不是同步的）。
async fn wait_for_frames(
    frames: &FrameBuf,
    expected: usize,
    budget: Duration,
) -> Vec<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        {
            let current = frames.lock().expect("frames");
            if current.len() >= expected {
                return current.clone();
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return frames.lock().expect("frames").clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn attach_does_not_emit_initialize_until_start() {
    let (frames, out) = sink();
    let registry = ScopedServerRegistry::new(out);
    let attachment = registry
        .attach(|server: &mut ChannelServer| {
            server.register_channel("memory", Arc::new(EchoChannel));
            Ok::<(), String>(())
        })
        .expect("attach");
    assert_eq!(attachment.scope_id(), 1);
    assert_eq!(registry.count(), 1);

    // 没 start 之前**一帧都不该出**：页面端口还没注册，发了会被按未知 scope 丢弃。
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        frames.lock().expect("frames").is_empty(),
        "attach 阶段不得发 Initialize：{:?}",
        frames.lock().expect("frames")
    );

    attachment.start();
    let sent = wait_for_frames(&frames, 1, Duration::from_secs(2)).await;
    assert_eq!(sent.len(), 1, "start 之后必须发一帧 Initialize");
    let decoded = decode_scope_frame(&sent[0]).expect("scoped frame");
    let rpc_wire::DecodedFrame::Scoped { scope_id, payload } = decoded else {
        panic!("必须是带 scope 前缀的帧");
    };
    assert_eq!(scope_id, 1);
    let response = decode_rpc_response(payload).expect("Initialize");
    assert_eq!(response.header, ResponseHeader::Initialize);
}

#[tokio::test]
async fn register_failure_recycles_the_scope_id() {
    let (frames, out) = sink();
    let registry = ScopedServerRegistry::new(out);
    let result =
        registry.attach(|_server: &mut ChannelServer| Err::<(), String>("注册失败".into()));
    assert_eq!(result.err(), Some("注册失败".to_string()));
    // 失败不得留下悬空 scope：否则 count() 对不上、scope id 也被占着。
    assert_eq!(registry.count(), 0);
    assert!(registry.active_scope_ids().is_empty());
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(frames.lock().expect("frames").is_empty());
}

#[tokio::test]
async fn scoped_frames_route_to_the_right_scope_and_get_the_prefix_back() {
    let (frames, out) = sink();
    let registry = ScopedServerRegistry::new(out);
    let first = registry
        .attach(|server: &mut ChannelServer| {
            server.register_channel("memory", Arc::new(EchoChannel));
            Ok::<(), String>(())
        })
        .expect("scope 1");
    let second = registry
        .attach(|server: &mut ChannelServer| {
            server.register_channel("memory", Arc::new(EchoChannel));
            Ok::<(), String>(())
        })
        .expect("scope 2");
    first.start();
    second.start();
    assert_eq!(first.scope_id(), 1);
    assert_eq!(second.scope_id(), 2);
    wait_for_frames(&frames, 2, Duration::from_secs(2)).await;
    frames.lock().expect("frames").clear();

    // 往 scope 2 发：回帧也必须带 scope 2 的前缀。
    let payload = request_frame(7, "memory", "echo", Value::str("hi-scope-2"));
    let wrapped = encode_scoped_frame(2, &payload).expect("wrap");
    assert!(registry.handle_frame(&wrapped), "已登记的 scope 必须被受理");

    let sent = wait_for_frames(&frames, 1, Duration::from_secs(2)).await;
    assert_eq!(sent.len(), 1);
    let decoded = decode_scope_frame(&sent[0]).expect("scoped");
    let rpc_wire::DecodedFrame::Scoped { scope_id, payload } = decoded else {
        panic!("必须是 scoped 帧");
    };
    assert_eq!(scope_id, 2, "回帧必须回到发问的那个 scope");
    let response = decode_rpc_response(payload).expect("response");
    assert_eq!(response.header, ResponseHeader::PromiseSuccess { id: 7 });
    assert_eq!(response.body, Value::str("hi-scope-2"));
}

#[tokio::test]
async fn unknown_or_base_scope_frames_are_not_accepted() {
    let (_frames, out) = sink();
    let registry = ScopedServerRegistry::new(out);
    registry
        .attach(|server: &mut ChannelServer| {
            server.register_channel("memory", Arc::new(EchoChannel));
            Ok::<(), String>(())
        })
        .expect("attach");

    // 未登记的 scope：丢弃（TS 解复用同）。
    let payload = request_frame(1, "memory", "echo", Value::Undefined);
    assert!(!registry.handle_frame(&encode_scoped_frame(99, &payload).expect("wrap")));
    // base 帧（不带前缀）不属于任何 scope：也丢弃。
    assert!(!registry.handle_frame(&payload));
}

#[tokio::test]
async fn detach_stops_forwarding_notifies_listeners_and_is_idempotent() {
    let (frames, out) = sink();
    let registry = ScopedServerRegistry::new(out);
    let detaches = Arc::new(Mutex::new(Vec::<u32>::new()));
    let sink_fn = Arc::clone(&detaches);
    let _subscription = registry.subscribe_detach(move |scope_id| {
        sink_fn.lock().expect("detaches").push(scope_id);
    });

    let attachment = registry
        .attach(|server: &mut ChannelServer| {
            server.register_channel("memory", Arc::new(EchoChannel));
            Ok::<(), String>(())
        })
        .expect("attach");
    attachment.start();
    wait_for_frames(&frames, 1, Duration::from_secs(2)).await;
    frames.lock().expect("frames").clear();

    assert!(registry.detach(attachment.scope_id()));
    assert_eq!(registry.count(), 0);
    assert_eq!(*detaches.lock().expect("detaches"), vec![1]);

    // 回收后再发帧：不得再有任何出站（转发器已停）。
    let payload = request_frame(2, "memory", "echo", Value::str("after-detach"));
    let wrapped = encode_scoped_frame(1, &payload).expect("wrap");
    assert!(!registry.handle_frame(&wrapped), "已回收的 scope 不再受理");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(frames.lock().expect("frames").is_empty());

    // 幂等：重复 detach 返回 false，也不再通知。
    assert!(!registry.detach(attachment.scope_id()));
    assert_eq!(*detaches.lock().expect("detaches"), vec![1]);
}

#[tokio::test]
async fn dispose_all_clears_every_scope_and_listener() {
    let (frames, out) = sink();
    let registry = ScopedServerRegistry::new(out);
    let detach_count = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&detach_count);
    let _subscription = registry.subscribe_detach(move |_scope_id| {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    for _ in 0..3 {
        registry
            .attach(|server: &mut ChannelServer| {
                server.register_channel("memory", Arc::new(EchoChannel));
                Ok::<(), String>(())
            })
            .expect("attach");
    }
    assert_eq!(registry.count(), 3);
    wait_for_frames(&frames, 3, Duration::from_secs(2)).await;

    registry.dispose_all();
    assert_eq!(registry.count(), 0);
    assert_eq!(detach_count.load(Ordering::SeqCst), 3);

    // 监听者也被清掉：再 detach 不会回调（此时也没有 scope 了）。
    registry.dispose_all();
    assert_eq!(detach_count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn start_on_unknown_scope_is_false_and_logs() {
    let (_frames, out) = sink();
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let store = Arc::clone(&logs);
    let registry = ScopedServerRegistry::with_options(
        out,
        Some(Arc::new(move |line: &str| {
            store.lock().expect("logs").push(line.to_string());
        })),
        Duration::from_millis(1000),
    );
    assert!(!registry.start(42));
    let logged = logs.lock().expect("logs").clone();
    assert!(
        logged
            .iter()
            .any(|line| line.contains("scope_ready_unknown")),
        "{logged:?}"
    );
}
