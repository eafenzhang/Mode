//! `mode-agent` 频道适配器契约测试（S2 第一批）：参数映射错误与
//! `unsupported_method` 降级。真实协议往返由 agent-process 的录制夹具覆盖。

use std::sync::Arc;

use agent_process::{AgentControlClient, AgentRuntimeRegistry, SpecFactory};
use host_channels::{ModeAgentChannel, MODE_AGENT_CHANNEL, UNSUPPORTED_METHOD_CODE};
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

fn channel() -> ModeAgentChannel {
    let factory: SpecFactory = Arc::new(|_target| panic!("契约测试不应拉起 runtime"));
    let registry = AgentRuntimeRegistry::new(factory);
    ModeAgentChannel::new(
        Arc::new(AgentControlClient::new(Arc::new(registry))),
        std::env::temp_dir().join("mode-agent-health-test-workspace"),
    )
}

async fn call(
    channel: &ModeAgentChannel,
    command: &str,
    arg: Value,
) -> Result<Value, ChannelError> {
    channel
        .call(
            "ctx".into(),
            command.into(),
            arg,
            CancellationToken::default(),
        )
        .await
}

fn object_arg(json: &str) -> Value {
    Value::Json(json.to_string())
}

fn message(error: &ChannelError) -> String {
    match error {
        ChannelError::Error { message, .. } => message.clone(),
        other => format!("{other:?}"),
    }
}

fn has_unsupported_code(error: &ChannelError) -> bool {
    match error {
        ChannelError::Error { extra, .. } => extra.iter().any(|(key, value)| {
            key == "code" && matches!(value, Value::Str(code) if code == UNSUPPORTED_METHOD_CODE)
        }),
        ChannelError::Value(_) => false,
    }
}

#[test]
fn channel_name_matches_service_channel() {
    assert_eq!(MODE_AGENT_CHANNEL, "mode-agent");
}

#[tokio::test]
async fn unmigrated_method_returns_unsupported_method() {
    let channel = channel();
    let error = call(&channel, "sendSessionMessage", object_arg("{}"))
        .await
        .expect_err("未迁移方法");
    assert!(
        message(&error).contains("MODE_HOST_INPROCESS_CHANNELS"),
        "{error:?}"
    );
    assert!(has_unsupported_code(&error), "{error:?}");
}

#[tokio::test]
async fn invalid_runtime_policy_is_rejected() {
    let channel = channel();
    let error = call(
        &channel,
        "listSessions",
        object_arg(r#"{"workspacePath":"D:/ws","runtimePolicy":"whatever"}"#),
    )
    .await
    .expect_err("非法 runtimePolicy");
    assert!(
        message(&error).contains("invalid runtimePolicy"),
        "{error:?}"
    );
}

#[tokio::test]
async fn invalid_delivery_kind_is_rejected() {
    let channel = channel();
    let error = call(
        &channel,
        "readSession",
        object_arg(r#"{"workspacePath":"D:/ws","sessionId":"s1","deliveryKind":"ftp"}"#),
    )
    .await
    .expect_err("非法 deliveryKind");
    assert!(
        message(&error).contains("invalid deliveryKind"),
        "{error:?}"
    );
}

#[tokio::test]
async fn missing_session_id_is_a_parse_error() {
    let channel = channel();
    let error = call(
        &channel,
        "readSessionMessages",
        object_arg(r#"{"workspacePath":"D:/ws"}"#),
    )
    .await
    .expect_err("缺 sessionId");
    assert!(
        message(&error).contains("invalid readSessionMessages request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn non_object_argument_is_rejected() {
    let channel = channel();
    let error = call(&channel, "listSessions", Value::Json("[]".into()))
        .await
        .expect_err("数组不是对象");
    assert!(
        message(&error).contains("arguments must be an object"),
        "{error:?}"
    );
}

#[tokio::test]
async fn listen_rejects_unknown_event() {
    let channel = channel();
    let error = channel
        .listen(
            "ctx".into(),
            "onDynamicSomething".into(),
            object_arg("{}"),
            std::sync::Arc::new(|_| {}),
        )
        .map(|_| ())
        .expect_err("未知事件");
    assert!(message(&error).contains("unknown event"), "{error:?}");
}

#[tokio::test]
async fn listen_rejects_invalid_delivery_kind() {
    let channel = channel();
    let error = channel
        .listen(
            "ctx".into(),
            "onDynamicSessionEvent".into(),
            object_arg(r#"{"workspacePath":"D:/ws","sessionId":"s1","deliveryKind":"ftp"}"#),
            std::sync::Arc::new(|_| {}),
        )
        .map(|_| ())
        .expect_err("非法 deliveryKind");
    assert!(
        message(&error).contains("invalid onDynamicSessionEvent request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn send_prompt_requires_content() {
    let channel = channel();
    let error = call(
        &channel,
        "sendPrompt",
        object_arg(r#"{"workspacePath":"D:/ws","sessionId":"s1"}"#),
    )
    .await
    .expect_err("缺 content");
    assert!(
        message(&error).contains("invalid sendPrompt request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn send_prompt_rejects_automation_and_offpeak_together() {
    let channel = channel();
    let error = call(
        &channel,
        "sendPrompt",
        object_arg(
            r#"{"workspacePath":"D:/ws","sessionId":"s1","content":"hi",
                "automationId":"a1","offPeakTaskId":"o1"}"#,
        ),
    )
    .await
    .expect_err("互斥校验");
    // 组合约束在 client_for 之前校验：不会拉起 runtime，错误直达调用方。
    assert!(message(&error).contains("mutually exclusive"), "{error:?}");
}

#[tokio::test]
async fn describe_plugin_requires_marketplace_and_name() {
    let channel = channel();
    let error = call(
        &channel,
        "describePlugin",
        object_arg(r#"{"workspacePath":"D:/ws"}"#),
    )
    .await
    .expect_err("缺 marketplace/pluginName");
    assert!(
        message(&error).contains("invalid describePlugin request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn configure_plugin_requires_options_object() {
    let channel = channel();
    let error = call(
        &channel,
        "configurePlugin",
        object_arg(r#"{"workspacePath":"D:/ws","pluginId":"p1","options":"nope"}"#),
    )
    .await
    .expect_err("options 非对象");
    assert!(
        message(&error).contains("invalid configurePlugin request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn list_saved_workflow_runs_requires_limit() {
    let channel = channel();
    let error = call(
        &channel,
        "listSavedWorkflowRuns",
        object_arg(r#"{"workspacePath":"D:/ws","name":"wf"}"#),
    )
    .await
    .expect_err("缺 limit");
    assert!(
        message(&error).contains("invalid listSavedWorkflowRuns request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn move_saved_workflow_requires_workspace() {
    let channel = channel();
    let error = call(
        &channel,
        "moveSavedWorkflow",
        object_arg(r#"{"name":"wf"}"#),
    )
    .await
    .expect_err("缺 workspacePath");
    assert!(
        message(&error).contains("invalid moveSavedWorkflow request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn runtime_identity_unavailable_matches_ts_message() {
    let channel = channel();
    let error = call(
        &channel,
        "getWorkspaceRuntimeIdentity",
        object_arg(r#"{"workspacePath":"D:/ws"}"#),
    )
    .await
    .expect_err("无 runtime");
    assert!(
        message(&error).contains("Mode agent runtime identity is unavailable."),
        "{error:?}"
    );
}

#[tokio::test]
async fn sync_app_runtime_preferences_requires_flag() {
    let channel = channel();
    let error = call(&channel, "syncAppRuntimePreferences", object_arg(r#"{}"#))
        .await
        .expect_err("缺 askUserQuestionAutoResolutionEnabled");
    assert!(
        message(&error).contains("invalid syncAppRuntimePreferences request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn grant_workspace_hook_trust_requires_digests() {
    let channel = channel();
    let error = call(
        &channel,
        "grantWorkspaceHookTrust",
        object_arg(r#"{"workspacePath":"D:/ws"}"#),
    )
    .await
    .expect_err("缺 digest");
    assert!(
        message(&error).contains("invalid grantWorkspaceHookTrust request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn storage_startup_listen_without_runtime_is_noop() {
    let channel = channel();
    let subscription = channel
        .listen(
            "ctx".into(),
            "onDynamicStorageStartupState".into(),
            object_arg(r#"{"workspacePath":"D:/ws"}"#),
            std::sync::Arc::new(|_| {}),
        )
        .expect("无活进程时应返回空订阅");
    subscription();
}

#[tokio::test]
async fn storage_startup_listen_requires_workspace() {
    let channel = channel();
    let error = channel
        .listen(
            "ctx".into(),
            "onDynamicStorageStartupState".into(),
            object_arg("{}"),
            std::sync::Arc::new(|_| {}),
        )
        .map(|_| ())
        .expect_err("缺 workspacePath");
    assert!(
        message(&error).contains("invalid onDynamicStorageStartupState request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn conversation_listen_registers_and_disposes() {
    let channel = channel();
    let subscription = channel
        .listen(
            "ctx".into(),
            "onDynamicConversationFrame".into(),
            object_arg(r#"{"workspacePath":"D:/ws"}"#),
            std::sync::Arc::new(|_| {}),
        )
        .expect("注册帧监听");
    subscription();
}

#[tokio::test]
async fn subscribe_conversation_rejects_invalid_visibility() {
    let channel = channel();
    let error = call(
        &channel,
        "subscribeConversationV4",
        object_arg(r#"{"workspacePath":"D:/ws","sessionId":"s1","visibility":"sideways"}"#),
    )
    .await
    .map(|_| ())
    .expect_err("非法 visibility");
    assert!(message(&error).contains("invalid visibility"), "{error:?}");
}

#[tokio::test]
async fn unsubscribe_unknown_subscription_is_noop() {
    let channel = channel();
    call(
        &channel,
        "unsubscribeConversationV4",
        object_arg(r#"{"workspacePath":"D:/ws","subscriptionId":"missing"}"#),
    )
    .await
    .expect("未知订阅应静默返回");
}

#[tokio::test]
async fn subscribe_conversation_requires_session_id() {
    let channel = channel();
    let error = call(
        &channel,
        "subscribeConversationV4",
        object_arg(r#"{"workspacePath":"D:/ws"}"#),
    )
    .await
    .map(|_| ())
    .expect_err("缺 sessionId");
    assert!(
        message(&error).contains("invalid subscribeConversationV4 request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn sessions_index_listen_registers_and_disposes() {
    let channel = channel();
    let subscription = channel
        .listen(
            "ctx".into(),
            "onDynamicSessionsIndexFrame".into(),
            object_arg(r#"{"workspacePath":"D:/ws"}"#),
            std::sync::Arc::new(|_| {}),
        )
        .expect("注册索引帧监听");
    subscription();
}

#[tokio::test]
async fn subscribe_sessions_index_rejects_invalid_visibility() {
    let channel = channel();
    let error = call(
        &channel,
        "subscribeSessionsIndexV4",
        object_arg(r#"{"workspacePath":"D:/ws","visibility":"sideways"}"#),
    )
    .await
    .map(|_| ())
    .expect_err("非法 visibility");
    assert!(message(&error).contains("invalid visibility"), "{error:?}");
}

#[tokio::test]
async fn unsubscribe_unknown_sessions_index_is_noop() {
    let channel = channel();
    call(
        &channel,
        "unsubscribeSessionsIndexV4",
        object_arg(r#"{"workspacePath":"D:/ws","subscriptionId":"missing"}"#),
    )
    .await
    .expect("未知订阅应静默返回");
}

#[tokio::test]
async fn workspace_config_listen_registers_and_disposes() {
    let channel = channel();
    let subscription = channel
        .listen(
            "ctx".into(),
            "onDynamicWorkspaceConfigFrame".into(),
            object_arg(r#"{"workspacePath":"D:/ws"}"#),
            std::sync::Arc::new(|_| {}),
        )
        .expect("注册配置帧监听");
    subscription();
}

#[tokio::test]
async fn subscribe_workspace_config_rejects_invalid_visibility() {
    let channel = channel();
    let error = call(
        &channel,
        "subscribeWorkspaceConfigV4",
        object_arg(r#"{"workspacePath":"D:/ws","visibility":"sideways"}"#),
    )
    .await
    .map(|_| ())
    .expect_err("非法 visibility");
    assert!(message(&error).contains("invalid visibility"), "{error:?}");
}

#[tokio::test]
async fn unsubscribe_unknown_workspace_config_is_noop() {
    let channel = channel();
    call(
        &channel,
        "unsubscribeWorkspaceConfigV4",
        object_arg(r#"{"workspacePath":"D:/ws","subscriptionId":"missing"}"#),
    )
    .await
    .expect("未知订阅应静默返回");
}

#[tokio::test]
async fn resync_without_route_is_not_owned() {
    let channel = channel();
    let error = call(
        &channel,
        "resyncConversationV4",
        object_arg(r#"{"workspacePath":"D:/ws","subscriptionId":"missing","base":null}"#),
    )
    .await
    .map(|_| ())
    .expect_err("无归属订阅");
    assert!(
        message(&error).contains("fault.subscription.notOwned"),
        "{error:?}"
    );
}

#[tokio::test]
async fn conversation_rows_range_requires_limit() {
    let channel = channel();
    let error = call(
        &channel,
        "conversationRowsRangeV4",
        object_arg(r#"{"workspacePath":"D:/ws","sessionId":"s1"}"#),
    )
    .await
    .map(|_| ())
    .expect_err("缺 limit");
    assert!(
        message(&error).contains("invalid conversationRowsRangeV4 request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn resync_sessions_index_without_route_is_not_owned() {
    let channel = channel();
    let error = call(
        &channel,
        "resyncSessionsIndexV4",
        object_arg(r#"{"workspacePath":"D:/ws","subscriptionId":"missing","base":null}"#),
    )
    .await
    .map(|_| ())
    .expect_err("无归属订阅");
    assert!(
        message(&error).contains("fault.subscription.notOwned"),
        "{error:?}"
    );
}

#[tokio::test]
async fn hello_conversation_returns_local_host_hello() {
    let channel = channel();
    let value = call(&channel, "helloConversationV4", Value::Undefined)
        .await
        .expect("host hello");
    let Value::Json(text) = value else {
        panic!("expected JSON string");
    };
    let hello: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(hello["kind"], "hello");
    assert_eq!(hello["connectionId"], "rust-inprocess");
    assert_eq!(hello["clientMode"], "desktop-continuous");
    assert_eq!(hello["deliveryProfile"], "continuous");
}

#[tokio::test]
async fn initialize_conversation_rejects_unknown_client_hello_field() {
    let channel = channel();
    let error = call(
        &channel,
        "initializeConversationV4",
        object_arg(
            r#"{"kind":"clientHello","protocolVersion":4,"clientId":"c1","appVersion":"1.0.0","extra":true}"#,
        ),
    )
    .await
    .map(|_| ())
    .expect_err("未知字段");
    assert!(
        message(&error).contains("invalid clientHello"),
        "{error:?}"
    );
}

#[tokio::test]
async fn initialize_conversation_accepts_valid_client_hello() {
    let channel = channel();
    call(
        &channel,
        "initializeConversationV4",
        object_arg(r#"{"kind":"clientHello","protocolVersion":4,"clientId":"c1","appVersion":"1.0.0"}"#),
    )
    .await
    .expect("合法 clientHello");
}

#[tokio::test]
async fn conversation_plans_requires_session_id() {
    let channel = channel();
    let error = call(
        &channel,
        "conversationPlansV4",
        object_arg(r#"{"workspacePath":"D:/ws"}"#),
    )
    .await
    .map(|_| ())
    .expect_err("缺 sessionId");
    assert!(
        message(&error).contains("invalid conversationPlansV4 request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn workflow_run_events_requires_run_id() {
    let channel = channel();
    let error = call(
        &channel,
        "conversationWorkflowRunEventsV4",
        object_arg(r#"{"workspacePath":"D:/ws","sessionId":"s1"}"#),
    )
    .await
    .map(|_| ())
    .expect_err("缺 runId");
    assert!(
        message(&error).contains("invalid conversationWorkflowRunEventsV4 request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn artifact_read_requires_offset_and_limit() {
    let channel = channel();
    let error = call(
        &channel,
        "conversationWorkflowRunArtifactReadV4",
        object_arg(
            r#"{"workspacePath":"D:/ws","sessionId":"s1","runId":"r1","artifactId":"a1","version":1}"#,
        ),
    )
    .await
    .map(|_| ())
    .expect_err("缺 offset/limit");
    assert!(
        message(&error).contains("invalid conversationWorkflowRunArtifactReadV4 request"),
        "{error:?}"
    );
}

#[tokio::test]
async fn node_result_requires_site_id_and_ordinal() {
    let channel = channel();
    let error = call(
        &channel,
        "conversationWorkflowRunNodeResultV4",
        object_arg(r#"{"workspacePath":"D:/ws","sessionId":"s1","runId":"r1"}"#),
    )
    .await
    .map(|_| ())
    .expect_err("缺 siteId/ordinal");
    assert!(
        message(&error).contains("invalid conversationWorkflowRunNodeResultV4 request"),
        "{error:?}"
    );
}
