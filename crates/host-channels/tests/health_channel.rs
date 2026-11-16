//! `agent-health` 频道契约测试：只读 list/describe 与空 registry 的 Null 语义。
//! 健康计算本身由 `agent-process/tests/health.rs` 覆盖；这里锁的是频道形状。

use std::sync::Arc;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, RuntimeKind, SpecFactory,
};
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

/// 跨包拿不到 agent-process 夹具的 `CARGO_BIN_EXE`；用平台自带的常驻命令占位一个
/// 活进程，锁「agent-health list/describe 输出包含 additive 的 runtime 字段」
/// （Task 9 控制器裁决 1）。命令本身与断言无关——runtime kind 由 spec 显式标注。
#[tokio::test]
async fn list_and_describe_include_runtime_kind_for_live_entry() {
    let (command, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("cmd", vec!["/c", "ping", "-n", "30", "127.0.0.1"])
    } else {
        ("sleep", vec!["30"])
    };
    let factory: SpecFactory = Arc::new(move |_target| {
        let mut spec = AgentProcessSpec::new(command);
        spec.args = args.iter().map(|arg| arg.to_string()).collect();
        spec.runtime = RuntimeKind::Node;
        Ok(spec)
    });
    let registry = Arc::new(AgentRuntimeRegistry::new(factory));
    let control = Arc::new(AgentControlClient::new(Arc::clone(&registry)));
    let channel = AgentHealthChannel::new(Arc::clone(&control));

    let target = agent_process::WorkspaceTarget::new("D:/ws/health-channel-runtime");
    registry
        .get_or_spawn(&target)
        .await
        .expect("占位进程必须能拉起");

    // describe：health.runtime 随既有形状一起出现（additive，不改既有字段）。
    let value = parse(
        call(
            &channel,
            DESCRIBE,
            object_arg(r#"{"workspacePath":"D:/ws/health-channel-runtime"}"#),
        )
        .await
        .expect("describe"),
    );
    assert_eq!(
        value["workspaceKey"],
        serde_json::json!("D:/ws/health-channel-runtime")
    );
    assert_eq!(value["runtime"], serde_json::json!("node"));
    assert_eq!(value["state"], serde_json::json!("ready"));

    // list：health 条目同样带 runtime。
    let items = parse(call(&channel, LIST, object_arg("[]")).await.expect("list"));
    assert_eq!(items.as_array().expect("list 是数组").len(), 1);
    assert_eq!(
        items[0]["workspacePath"],
        serde_json::json!("D:/ws/health-channel-runtime")
    );
    assert_eq!(items[0]["health"]["runtime"], serde_json::json!("node"));

    registry
        .shutdown_all(std::time::Duration::from_secs(5))
        .await;
}
