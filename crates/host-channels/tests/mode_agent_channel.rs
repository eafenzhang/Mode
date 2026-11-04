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
    ModeAgentChannel::new(Arc::new(AgentControlClient::new(Arc::new(registry))))
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
