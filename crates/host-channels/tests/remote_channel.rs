//! `remote` 频道契约（LT-16.4d）。
//!
//! 真源 `packages/services/src/remote/remoteService.ts`。覆盖：connect 的三种结局
//! （成功 / 端口缺失 / 传输失败）、cancel/bind/dispose 的转发、快照形状、
//! **secret 不下发**、事件只发当前订阅者、attach/detach 显式报错。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use host_channels::remote::{
    RemoteChannel, RemoteScopeFactory, RemoteTransportPort, ON_DYNAMIC_CONNECTION_EVENT,
};
use remote_session_registry::RemoteSessionSnapshot;
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, ChannelServer, ScopedServerRegistry, ServerChannel,
};
use rpc_wire::Value;
use serde_json::{json, Value as Json};

/// 可编排的假端口：记录调用了哪些副作用。
#[derive(Default)]
struct FakeTransport {
    connects: AtomicUsize,
    aborts: AtomicUsize,
    disposes: AtomicUsize,
    releases: AtomicUsize,
    fail_with: Mutex<Option<String>>,
}

impl RemoteTransportPort for FakeTransport {
    fn connect(
        &self,
        _plan: remote_session_registry::ConnectPlan,
        _target: remote_session_registry::RemoteTargetSpec,
    ) -> BoxFuture<Result<(), String>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let failure = self.fail_with.lock().expect("fail_with").clone();
        Box::pin(async move {
            match failure {
                Some(message) => Err(message),
                None => Ok(()),
            }
        })
    }

    fn abort(&self, _entry_id: u64) {
        self.aborts.fetch_add(1, Ordering::SeqCst);
    }

    fn dispose(&self, _entry_id: u64) {
        self.disposes.fetch_add(1, Ordering::SeqCst);
    }

    fn release_workspace(
        &self,
        _entry_id: u64,
        _workspace_key: String,
        _generation: u32,
        _context: remote_session_registry::RemoteWorkspaceContext,
    ) -> BoxFuture<Result<(), String>> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

fn args(value: Json) -> Value {
    Value::Json(value.to_string())
}

/// 调用并解析 JSON body；`void` 方法（cancelConnect/bindContext/dispose）返回 `Null`。
async fn call(channel: &RemoteChannel, method: &str, payload: Json) -> Result<Json, String> {
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
        Value::Json(text) => Ok(serde_json::from_str(&text).expect("json body")),
        // void 方法返回 Undefined：TS 侧同样是 `Promise<void>`。
        Value::Undefined => Ok(Json::Null),
        other => panic!("unexpected body for {method}: {other:?}"),
    }
}

fn lan_target(host: &str, port: u16) -> Json {
    json!({ "kind": "lan", "host": host, "port": port })
}

#[tokio::test]
async fn connect_without_a_transport_returns_ok_false_not_an_error() {
    let channel = RemoteChannel::new(None);
    let result = call(
        &channel,
        "connect",
        json!({
            "target": lan_target("10.0.0.5", 45879),
            "workspacePath": "/srv/app",
        }),
    )
    .await
    .expect("端口缺失必须返回 ok:false 而不是 RPC 错误");

    // renderer 按 ok 分支处理；抛错会走成未捕获异常。
    assert_eq!(result["ok"], false);
    assert!(
        result["error"].as_str().unwrap_or("").contains("尚未就绪"),
        "{result:?}"
    );
    // 失败后不得留下任何 session（否则会永远 connecting）。
    assert_eq!(
        call(&channel, "listSessions", json!({}))
            .await
            .expect("listSessions"),
        json!([])
    );
}

#[tokio::test]
async fn connect_success_returns_a_descriptor_without_secrets() {
    let transport = Arc::new(FakeTransport::default());
    let channel = RemoteChannel::new(Some(Arc::clone(&transport) as Arc<dyn RemoteTransportPort>));

    let result = call(
        &channel,
        "connect",
        json!({
            "target": {
                "kind": "ssh",
                "host": " Build-01 ",
                "username": "dev",
                "password": "s3cret",
            },
            "workspacePath": "/srv/app",
        }),
    )
    .await
    .expect("connect");

    assert_eq!(result["ok"], true, "{result:?}");
    assert_eq!(transport.connects.load(Ordering::SeqCst), 1);

    let descriptor = &result["descriptor"];
    // 密码绝不能出现在任何下发给 renderer 的形状里。
    assert!(
        !descriptor.to_string().contains("s3cret"),
        "descriptor 泄漏了密码：{descriptor}"
    );
    assert_eq!(descriptor["target"]["kind"], "ssh");
    assert_eq!(
        descriptor["target"]["host"], "Build-01",
        "host 保留原大小写供展示"
    );
    assert_eq!(descriptor["target"]["port"], 22, "缺省端口补成 22");
    // identity 由统一构造器派生（host 归一为小写）。
    assert_eq!(
        descriptor["workspaceIdentity"],
        "remote:ssh:build-01:22:dev:/srv/app"
    );
}

#[tokio::test]
async fn transport_failure_is_reported_and_leaves_no_session() {
    let transport = Arc::new(FakeTransport::default());
    *transport.fail_with.lock().expect("fail_with") =
        Some("连接对端通道失败：拒绝连接".to_string());
    let channel = RemoteChannel::new(Some(Arc::clone(&transport) as Arc<dyn RemoteTransportPort>));

    let result = call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/w" }),
    )
    .await
    .expect("connect");

    assert_eq!(result["ok"], false);
    assert_eq!(result["error"], "连接对端通道失败：拒绝连接");
    assert_eq!(
        call(&channel, "listSessions", json!({}))
            .await
            .expect("listSessions"),
        json!([]),
        "传输失败后不得留下 connecting 状态的 session"
    );
}

#[tokio::test]
async fn duplicate_request_id_is_an_rpc_error() {
    let transport = Arc::new(FakeTransport::default());
    let channel = RemoteChannel::new(Some(Arc::clone(&transport) as Arc<dyn RemoteTransportPort>));
    call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/w", "requestId": "req-1" }),
    )
    .await
    .expect("first connect");

    let error = call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/w", "requestId": "req-1" }),
    )
    .await
    .expect_err("重复 requestId 必须失败");
    assert!(error.contains("requestId 重复"), "{error}");
}

#[tokio::test]
async fn same_target_reuses_one_transport_but_keeps_two_sessions() {
    let transport = Arc::new(FakeTransport::default());
    let channel = RemoteChannel::new(Some(Arc::clone(&transport) as Arc<dyn RemoteTransportPort>));

    let first = call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/a" }),
    )
    .await
    .expect("first");
    let second = call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/b" }),
    )
    .await
    .expect("second");

    // 复用已 online 的 entry：第二次不该再起传输。
    assert_eq!(transport.connects.load(Ordering::SeqCst), 1);
    assert_ne!(
        first["descriptor"]["remoteSessionId"],
        second["descriptor"]["remoteSessionId"]
    );
    assert_eq!(
        call(&channel, "listSessions", json!({}))
            .await
            .expect("list")
            .as_array()
            .map(Vec::len),
        Some(2)
    );
}

#[tokio::test]
async fn bind_context_bumps_generation_and_get_session_reflects_it() {
    let transport = Arc::new(FakeTransport::default());
    let channel = RemoteChannel::new(Some(Arc::clone(&transport) as Arc<dyn RemoteTransportPort>));
    let connected = call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/old" }),
    )
    .await
    .expect("connect");
    let session_id = connected["descriptor"]["remoteSessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    assert_eq!(
        call(
            &channel,
            "getSession",
            json!({ "sessionId": session_id.clone() })
        )
        .await
        .expect("getSession")["generation"],
        1
    );

    call(
        &channel,
        "bindContext",
        json!({
            "sessionId": session_id.clone(),
            "workspacePath": "/new",
            "workspaceIdentity": "remote:lan:h:1:/new",
        }),
    )
    .await
    .expect("bindContext");

    let snapshot = call(
        &channel,
        "getSession",
        json!({ "sessionId": session_id.clone() }),
    )
    .await
    .expect("getSession");
    assert_eq!(snapshot["generation"], 2);
    assert_eq!(snapshot["workspacePath"], "/new");
    assert_eq!(snapshot["workspaceIdentity"], "remote:lan:h:1:/new");

    // 空 workspacePath 必须是明确错误，不能静默把 session 绑到空路径。
    let error = call(
        &channel,
        "bindContext",
        json!({ "sessionId": session_id, "workspacePath": "  " }),
    )
    .await
    .expect_err("空路径必须失败");
    assert!(error.contains("workspacePath 不能为空"), "{error}");
}

#[tokio::test]
async fn dispose_and_cancel_tell_the_transport() {
    let transport = Arc::new(FakeTransport::default());
    let channel = RemoteChannel::new(Some(Arc::clone(&transport) as Arc<dyn RemoteTransportPort>));

    // cancelConnect 对未知 requestId 幂等（TS 同）。
    call(&channel, "cancelConnect", json!({ "requestId": "nope" }))
        .await
        .expect("幂等");

    let connected = call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/w" }),
    )
    .await
    .expect("connect");
    let session_id = connected["descriptor"]["remoteSessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    call(&channel, "dispose", json!({ "sessionId": session_id }))
        .await
        .expect("dispose");
    assert_eq!(
        call(&channel, "listSessions", json!({}))
            .await
            .expect("list"),
        json!([])
    );
    // 最后一个 waiter 走了：LAN 不是 wsl/ssh online，应当被立刻销毁。
    assert!(
        transport.disposes.load(Ordering::SeqCst) >= 1,
        "应通知端口销毁"
    );
}

#[tokio::test]
async fn attach_without_scopes_reports_the_missing_wiring() {
    // 未装配 scoped 服务面时必须显式报错，不能给个假 scopeId。
    let channel = RemoteChannel::new(None);
    let error = call(&channel, "attach", json!({ "sessionId": "s" }))
        .await
        .expect_err("必须显式报错");
    assert!(error.contains("scoped 服务面装配"), "{error}");
    assert_eq!(channel.attachment_count(), 0);
}

#[tokio::test]
async fn detach_unknown_attachment_is_idempotent() {
    // TS 同：找不到 attachmentId 就 return，不报错。
    let channel = RemoteChannel::new(None);
    call(&channel, "detach", json!({ "attachmentId": "nope" }))
        .await
        .expect("幂等");
}

#[tokio::test]
async fn dynamic_events_only_reach_current_subscribers_and_are_not_replayed() {
    let transport = Arc::new(FakeTransport::default());
    let channel = RemoteChannel::new(Some(Arc::clone(&transport) as Arc<dyn RemoteTransportPort>));
    let received = Arc::new(Mutex::new(Vec::<Json>::new()));
    let sink = Arc::clone(&received);
    let subscription = channel
        .listen(
            "ctx".into(),
            ON_DYNAMIC_CONNECTION_EVENT.into(),
            Value::Undefined,
            Arc::new(move |value: Value| {
                if let Value::Json(text) = value {
                    sink.lock()
                        .expect("sink")
                        .push(serde_json::from_str(&text).expect("json"));
                }
            }),
        )
        .expect("listen");

    call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/w" }),
    )
    .await
    .expect("connect");

    let events = received.lock().expect("received").clone();
    assert_eq!(events.len(), 1, "连接成功应发一条 connected：{events:?}");
    assert_eq!(events[0]["kind"], "connected");

    // 订阅结束后不再收到；并且新订阅者**不会**补发历史事件（与 onDidChange 不同）。
    subscription();
    call(
        &channel,
        "connect",
        json!({ "target": lan_target("h2", 2), "workspacePath": "/w2" }),
    )
    .await
    .expect("second connect");
    assert_eq!(received.lock().expect("received").len(), 1);

    let late = Arc::new(Mutex::new(0usize));
    let sink = Arc::clone(&late);
    let _late_subscription = channel
        .listen(
            "ctx".into(),
            ON_DYNAMIC_CONNECTION_EVENT.into(),
            Value::Undefined,
            Arc::new(move |_value: Value| {
                *sink.lock().expect("late") += 1;
            }),
        )
        .expect("listen");
    assert_eq!(*late.lock().expect("late"), 0, "动态事件不重放历史");
}

#[tokio::test]
async fn unknown_command_and_event_are_rejected() {
    let channel = RemoteChannel::new(None);
    let error = call(&channel, "nope", json!({}))
        .await
        .expect_err("unknown");
    assert!(error.contains("unknown command"), "{error}");

    let error = channel
        .listen(
            "ctx".into(),
            "nope".into(),
            Value::Undefined,
            Arc::new(|_: Value| {}),
        )
        .err()
        .expect("unknown event");
    assert!(format!("{error:?}").contains("unknown event"));
}

/// 最小服务面工厂：注册一个 echo 频道，并记录被调用次数。
#[derive(Default)]
struct FakeScopeFactory {
    calls: AtomicUsize,
}

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

impl RemoteScopeFactory for FakeScopeFactory {
    fn register(
        &self,
        server: &mut ChannelServer,
        _session: &RemoteSessionSnapshot,
        _client: Option<Arc<rpc_client::ChannelClient>>,
    ) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        server.register_channel("memory", Arc::new(EchoChannel));
        Ok(())
    }
}

/// 组装一个带 scoped 服务面的频道。
fn channel_with_scopes(
    transport: Arc<FakeTransport>,
) -> (
    RemoteChannel,
    Arc<ScopedServerRegistry>,
    Arc<FakeScopeFactory>,
) {
    let scopes = Arc::new(ScopedServerRegistry::new(Arc::new(|_frame: Vec<u8>| {})));
    let factory = Arc::new(FakeScopeFactory::default());
    let channel = RemoteChannel::new(Some(transport as Arc<dyn RemoteTransportPort>)).with_scopes(
        Arc::clone(&scopes),
        factory.clone() as Arc<dyn RemoteScopeFactory>,
    );
    (channel, scopes, factory)
}

#[tokio::test]
async fn attach_requires_a_bound_workspace_context() {
    let transport = Arc::new(FakeTransport::default());
    let (channel, scopes, factory) = channel_with_scopes(Arc::clone(&transport));

    // connect 时不给 workspacePath/identity：session 存在但未绑定上下文。
    let connected = call(&channel, "connect", json!({ "target": lan_target("h", 1) }))
        .await
        .expect("connect");
    let session_id = connected["descriptor"]["remoteSessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    let error = call(&channel, "attach", json!({ "sessionId": session_id }))
        .await
        .expect_err("未绑定上下文必须报错");
    assert!(
        error.contains("先 bindContext 再 attach"),
        "报错必须指向正确的修复动作：{error}"
    );
    // 失败不得留下 scope 或 attachment。
    assert_eq!(scopes.count(), 0);
    assert_eq!(channel.attachment_count(), 0);
    assert_eq!(factory.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn attach_creates_a_scope_that_is_not_ready_yet() {
    let transport = Arc::new(FakeTransport::default());
    let (channel, scopes, factory) = channel_with_scopes(Arc::clone(&transport));
    let connected = call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/w" }),
    )
    .await
    .expect("connect");
    let session_id = connected["descriptor"]["remoteSessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    let attachment = call(
        &channel,
        "attach",
        json!({ "sessionId": session_id.clone() }),
    )
    .await
    .expect("attach");

    // 描述符三件套齐备，scopeId 从 1 起。
    assert_eq!(attachment["scopeId"], 1);
    assert_eq!(attachment["sessionId"], session_id);
    assert!(attachment["attachmentId"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    assert_eq!(scopes.count(), 1);
    assert_eq!(channel.attachment_count(), 1);
    assert_eq!(factory.calls.load(Ordering::SeqCst), 1);

    // **attach 不 ready**：页面端口没注册就发 Initialize 会被丢弃且不重发。
    // scopes.start 才是发 Initialize 的时机（这里只验「scope 已建但不 ready」的
    // 语义由 ScopedServerRegistry 的用例覆盖）。
    let attachment_id = attachment["attachmentId"].as_str().expect("id").to_string();
    assert_eq!(
        channel.attachment_ids_for_session(&session_id),
        vec![attachment_id.clone()]
    );

    // detach 回收 scope 与登记。
    call(&channel, "detach", json!({ "attachmentId": attachment_id }))
        .await
        .expect("detach");
    assert_eq!(scopes.count(), 0);
    assert_eq!(channel.attachment_count(), 0);
}

#[tokio::test]
async fn detach_only_reclaims_the_named_attachment() {
    let transport = Arc::new(FakeTransport::default());
    let (channel, scopes, _factory) = channel_with_scopes(Arc::clone(&transport));
    let connected = call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/w" }),
    )
    .await
    .expect("connect");
    let session_id = connected["descriptor"]["remoteSessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    let first = call(
        &channel,
        "attach",
        json!({ "sessionId": session_id.clone() }),
    )
    .await
    .expect("attach 1");
    let second = call(
        &channel,
        "attach",
        json!({ "sessionId": session_id.clone() }),
    )
    .await
    .expect("attach 2");
    assert_eq!(scopes.count(), 2, "两次 attach 必须是两个独立 scope");
    assert_ne!(first["scopeId"], second["scopeId"], "scope id 不复用");

    call(
        &channel,
        "detach",
        json!({ "attachmentId": first["attachmentId"].as_str().expect("id") }),
    )
    .await
    .expect("detach 1");

    // 只回收第一个：第二个仍活着。
    assert_eq!(scopes.count(), 1);
    assert_eq!(channel.attachment_count(), 1);
    assert_eq!(
        channel.attachment_ids_for_session(&session_id),
        vec![second["attachmentId"].as_str().expect("id").to_string()]
    );
}

#[tokio::test]
async fn dispose_reclaims_every_attachment_of_the_session() {
    let transport = Arc::new(FakeTransport::default());
    let (channel, scopes, _factory) = channel_with_scopes(Arc::clone(&transport));
    let connected = call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/w" }),
    )
    .await
    .expect("connect");
    let session_id = connected["descriptor"]["remoteSessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    for _ in 0..3 {
        call(
            &channel,
            "attach",
            json!({ "sessionId": session_id.clone() }),
        )
        .await
        .expect("attach");
    }
    assert_eq!(scopes.count(), 3);

    call(
        &channel,
        "dispose",
        json!({ "sessionId": session_id.clone() }),
    )
    .await
    .expect("dispose");

    // TS 同序：session 释放时它的 scope 必须一起回收，否则 scope 泄漏。
    assert_eq!(scopes.count(), 0, "dispose 必须回收该 session 的全部 scope");
    assert_eq!(channel.attachment_count(), 0);
    assert!(channel.attachment_ids_for_session(&session_id).is_empty());
}

#[tokio::test]
async fn scope_factory_failure_leaves_no_attachment_or_scope() {
    struct FailingFactory;
    impl RemoteScopeFactory for FailingFactory {
        fn register(
            &self,
            _server: &mut ChannelServer,
            _session: &RemoteSessionSnapshot,
            _client: Option<Arc<rpc_client::ChannelClient>>,
        ) -> Result<(), String> {
            Err("服务面注册失败".to_string())
        }
    }

    let transport = Arc::new(FakeTransport::default());
    let scopes = Arc::new(ScopedServerRegistry::new(Arc::new(|_frame: Vec<u8>| {})));
    let channel = RemoteChannel::new(Some(Arc::clone(&transport) as Arc<dyn RemoteTransportPort>))
        .with_scopes(Arc::clone(&scopes), Arc::new(FailingFactory));
    let connected = call(
        &channel,
        "connect",
        json!({ "target": lan_target("h", 1), "workspacePath": "/w" }),
    )
    .await
    .expect("connect");
    let session_id = connected["descriptor"]["remoteSessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    let error = call(&channel, "attach", json!({ "sessionId": session_id }))
        .await
        .expect_err("注册失败必须上抛");
    assert!(error.contains("服务面注册失败"), "{error}");
    assert_eq!(scopes.count(), 0, "注册失败要回收 scope");
    assert_eq!(channel.attachment_count(), 0, "失败不得登记 attachment");
}
