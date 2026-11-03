//! `agent-health` 频道契约测试：只读 list/describe 与空 registry 的 Null 语义。
//! 健康计算本身由 `agent-process/tests/health.rs` 覆盖；这里锁的是频道形状。

use std::sync::Arc;

use agent_process::{AgentControlClient, AgentRuntimeRegistry, SpecFactory};
use host_channels::{AgentHealthChannel, AGENT_HEALTH_CHANNEL, DESCRIBE, LIST};
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

fn channel() -> AgentHealthChannel {
    let factory: SpecFactory = Arc::new(|_target| {
        panic!("只读健康频道不得拉起 runtime");
    });
    let registry = AgentRuntimeRegistry::new(factory);
    AgentHealthChannel::new(Arc::new(AgentControlClient::new(Arc::new(registry))))
}

fn object_arg(json: &str) -> Value {
    Value::Json(json.to_string())
}

fn parse(value: Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(&text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

async fn call(channel: &AgentHealthChannel, method: &str, arg: Value) -> Result<Value, String> {
    channel
        .call(
            "ctx".into(),
            method.into(),
            arg,
            CancellationToken::default(),
        )
        .await
        .map_err(|error| match error {
            ChannelError::Error { message, .. } => message,
            other => format!("{other:?}"),
        })
}

#[test]
fn channel_name_matches_registry_gate() {
    assert_eq!(AGENT_HEALTH_CHANNEL, "agent-health");
}

#[tokio::test]
async fn list_on_empty_registry_returns_empty_array() {
    let channel = channel();
    let value = parse(call(&channel, LIST, object_arg("[]")).await.expect("list"));
    assert_eq!(value, serde_json::json!([]));
}

#[tokio::test]
async fn describe_missing_workspace_returns_null() {
    let channel = channel();
    let value = parse(
        call(
            &channel,
            DESCRIBE,
            object_arg(r#"{"workspacePath":"D:/ws"}"#),
        )
        .await
        .expect("describe"),
    );
    assert_eq!(value, serde_json::Value::Null);
}

#[tokio::test]
async fn describe_requires_workspace_path() {
    let channel = channel();
    let error = call(&channel, DESCRIBE, object_arg(r#"{}"#))
        .await
        .expect_err("缺 workspacePath");
    assert!(error.contains("workspacePath"), "{error}");
}

#[tokio::test]
async fn unknown_command_is_rejected() {
    let channel = channel();
    let error = call(&channel, "ping", object_arg("[]"))
        .await
        .expect_err("未知命令");
    assert!(error.contains("unknown command"), "{error}");
}
