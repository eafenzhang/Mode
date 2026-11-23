//! 远端频道代理与远端服务面工厂的契约。
//!
//! 用真 `rpc_host::ChannelServer` 做远端、真 `rpc_client::ChannelClient` 做本地代理，
//! 中间用两个 mpsc 搬真实帧——这样「参数原样透传」「错误形状保留」「订阅释放转发」
//! 都是被真实验证过的。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_channels::remote::RemoteScopeFactory;
use host_channels::{RemoteProxyChannel, RemoteServicesScopeFactory};
use remote_session_registry::{ConnectionState, RemoteSessionSnapshot, RemoteTargetSpec};
use rpc_client::ChannelClient;
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, ChannelServer, EventCallback, ServerChannel,
    Subscription,
};
use rpc_wire::Value;
use tokio::sync::mpsc;

/// 远端频道：echo 原样返回、boom 返回带 code 的错误、fire 发一条事件。
struct RemoteMemoryChannel {
    disposals: Arc<AtomicUsize>,
}

impl ServerChannel for RemoteMemoryChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let result = match command.as_str() {
            "echo" => Ok(arg),
            "boom" => Err(ChannelError::error("远端炸了").with_extra("code", Value::str("E_REMOTE"))),
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
        emit(Value::str("remote-event"));
        let disposals = Arc::clone(&self.disposals);
        Ok(Box::new(move || {
            disposals.fetch_add(1, Ordering::SeqCst);
        }))
    }
}

struct Harness {
    client: Arc<ChannelClient>,
}

/// 起一对「本地 client ↔ 远端 ChannelServer」。
fn harness() -> (Harness, Arc<AtomicUsize>) {
    let (to_server_tx, mut to_server_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (to_client_tx, mut to_client_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let disposals = Arc::new(AtomicUsize::new(0));
    let mut server = ChannelServer::new(
        to_client_tx,
        "remote-host",
        Duration::from_secs(5),
        false,
    );
    server.register_channel(
        "memory",
        Arc::new(RemoteMemoryChannel {
            disposals: Arc::clone(&disposals),
        }),
    );
    let server = Arc::new(tokio::sync::Mutex::new(server));

    let client = Arc::new(ChannelClient::new(Arc::new(move |frame: Vec<u8>| {
        let _ = to_server_tx.send(frame);
    })));

    tokio::spawn(async move {
        while let Some(frame) = to_server_rx.recv().await {
            server.lock().await.handle_frame(&frame);
        }
    });
    let client_for_reader = Arc::clone(&client);
    tokio::spawn(async move {
        while let Some(frame) = to_client_rx.recv().await {
            client_for_reader.handle_frame(&frame);
        }
    });

    (Harness { client }, disposals)
}

async fn wait_initialized(client: &ChannelClient) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !client.is_initialized() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Initialize 必须到达");
}

#[tokio::test]
async fn proxy_forwards_call_arguments_verbatim() {
    let (harness, _disposals) = harness();
    wait_initialized(&harness.client).await;
    let proxy = RemoteProxyChannel::new(Arc::clone(&harness.client), "memory");

    // 位置参数数组：对象在首位——与 renderer 直连远端时同形。
    let arg = Value::Json(r#"[{"path":"/a","limit":5}]"#.to_string());
    let result = proxy
        .call("ctx".into(), "echo".into(), arg.clone(), CancellationToken::default())
        .await
        .expect("echo");
    assert_eq!(result, arg, "参数必须原样透传，不拆包不重组");
}

#[tokio::test]
async fn proxy_preserves_the_remote_error_shape() {
    let (harness, _disposals) = harness();
    wait_initialized(&harness.client).await;
    let proxy = RemoteProxyChannel::new(Arc::clone(&harness.client), "memory");

    let error = proxy
        .call(
            "ctx".into(),
            "boom".into(),
            Value::Undefined,
            CancellationToken::default(),
        )
        .await
        .expect_err("boom 必须失败");
    match error {
        ChannelError::Error { message, extra, .. } => {
            assert_eq!(message, "远端炸了");
            // 透传字段必须保留，否则 renderer 拿不到 code 无法分支处理。
            assert!(
                extra.iter().any(|(key, _)| key == "code"),
                "透传字段丢失：{extra:?}"
            );
        }
        other => panic!("期望 Error 形态，实际 {other:?}"),
    }
}

#[tokio::test]
async fn proxy_forwards_subscription_disposal_to_the_remote() {
    let (harness, disposals) = harness();
    wait_initialized(&harness.client).await;
    let proxy = RemoteProxyChannel::new(Arc::clone(&harness.client), "memory");

    let received = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = Arc::clone(&received);
    let subscription = proxy
        .listen(
            "ctx".into(),
            "fire".into(),
            Value::Undefined,
            Arc::new(move |value: Value| {
                sink.lock().expect("sink").push(value);
            }),
        )
        .expect("listen");

    // 事件到达（远端在 listen 里立即发一条）。
    tokio::time::timeout(Duration::from_secs(2), async {
        while received.lock().expect("received").is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("事件必须到达");
    assert_eq!(received.lock().expect("received")[0], Value::str("remote-event"));

    // 释放订阅必须传导到远端：否则 scope 回收后远端订阅永久泄漏。
    subscription();
    tokio::time::timeout(Duration::from_secs(2), async {
        while disposals.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("远端必须收到 EventDispose");
}

#[tokio::test]
async fn proxy_reports_closed_connection_instead_of_hanging() {
    let (harness, _disposals) = harness();
    wait_initialized(&harness.client).await;
    let proxy = RemoteProxyChannel::new(Arc::clone(&harness.client), "memory");
    harness.client.dispose();

    let error = proxy
        .call(
            "ctx".into(),
            "echo".into(),
            Value::str("x"),
            CancellationToken::default(),
        )
        .await
        .expect_err("dispose 后必须失败");
    // 必须是可读错误而不是永久挂起。
    assert!(
        format!("{error:?}").contains("disposed") || format!("{error:?}").contains("已关闭"),
        "{error:?}"
    );
}

#[tokio::test]
async fn scope_factory_registers_every_declared_channel() {
    let (harness, _disposals) = harness();
    wait_initialized(&harness.client).await;
    let factory = RemoteServicesScopeFactory::new(vec!["memory".to_string()]);

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut server = ChannelServer::new(out_tx, "scope", Duration::from_secs(5), true);
    let snapshot = RemoteSessionSnapshot {
        remote_session_id: "s".to_string(),
        request_id: "r".to_string(),
        target: RemoteTargetSpec::Lan {
            host: "h".to_string(),
            port: 1,
            server_id: None,
            token: None,
        },
        workspace_path: Some("/w".to_string()),
        workspace_identity: Some("remote:lan:h:1:/w".to_string()),
        generation: 1,
        state: ConnectionState::Online,
        source_availability: "online",
    };
    factory
        .register(&mut server, &snapshot, Some(Arc::clone(&harness.client)))
        .expect("注册必须成功");

    // 往已注册的频道发一个请求：代理把它转给远端，远端回帧后再经代理回本地。
    let payload = rpc_wire::encode_rpc_message(&rpc_wire::RpcMessage {
        header: rpc_wire::RequestHeader::Promise {
            id: 1,
            channel: "memory".to_string(),
            method: "echo".to_string(),
        },
        body: Value::str("ping"),
    });
    server.handle_frame(&payload);

    let frame = tokio::time::timeout(Duration::from_secs(3), out_rx.recv())
        .await
        .expect("必须回帧（说明代理确实挂上了）")
        .expect("frame");
    let response = rpc_wire::decode_rpc_response(&frame).expect("response");
    assert_eq!(
        response.header,
        rpc_wire::ResponseHeader::PromiseSuccess { id: 1 }
    );
    assert_eq!(response.body, Value::str("ping"));
}

#[tokio::test]
async fn proxy_cancel_does_not_hang_the_caller() {
    let (harness, _disposals) = harness();
    wait_initialized(&harness.client).await;
    let proxy = RemoteProxyChannel::new(Arc::clone(&harness.client), "memory");

    let cancel = CancellationToken::default();
    cancel.cancel();
    let error = proxy
        .call("ctx".into(), "echo".into(), Value::Undefined, cancel)
        .await
        .expect_err("已取消的调用必须立即失败");
    assert!(format!("{error:?}").contains("Cancelled"), "{error:?}");
}
