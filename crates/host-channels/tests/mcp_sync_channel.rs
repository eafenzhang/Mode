//! `mcp-sync` 频道契约测试：本地方法形状与 W4 控制面容错。

use std::sync::Arc;

use agent_process::{AgentControlClient, AgentRuntimeRegistry, SpecFactory};
use host_channels::{McpSyncChannel, MCP_SYNC_CHANNEL};
use mcp_sync_service::{McpSyncService, McpSyncServiceOptions};
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value;

fn channel_with_home(home: std::path::PathBuf) -> McpSyncChannel {
    let factory: SpecFactory = Arc::new(|_target| panic!("契约测试不应拉起 runtime"));
    let control = Arc::new(AgentControlClient::new(Arc::new(
        AgentRuntimeRegistry::new(factory),
    )));
    McpSyncChannel::new(
        Arc::new(McpSyncService::new(McpSyncServiceOptions {
            home_dir: Some(home),
        })),
        control,
    )
}

fn temp_home(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcp-sync-channel-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

async fn call(channel: &McpSyncChannel, command: &str, arg: Value) -> Result<Value, String> {
    channel
        .call(
            "ctx".into(),
            command.into(),
            arg,
            CancellationToken::default(),
        )
        .await
        .map_err(|error| match error {
            rpc_host::ChannelError::Error { message, .. } => message,
            other => format!("{other:?}"),
        })
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

#[test]
fn channel_name_matches_service_channel() {
    assert_eq!(MCP_SYNC_CHANNEL, "mcp-sync");
}

#[tokio::test]
async fn load_without_request_loads_empty_home() {
    let home = temp_home("load");
    let channel = channel_with_home(home.clone());
    let value = parse(
        call(&channel, "loadMcpFromUserDirectory", Value::Undefined)
            .await
            .expect("load"),
    );
    assert_eq!(value["servers"], serde_json::json!([]));
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn local_candidates_on_empty_home_are_empty() {
    let home = temp_home("candidates");
    let channel = channel_with_home(home.clone());
    let value = parse(
        call(&channel, "listLocalUserMcpCandidates", object_arg("{}"))
            .await
            .expect("candidates"),
    );
    assert_eq!(value["candidates"], serde_json::json!([]));
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn workspace_statuses_require_workspace_path() {
    let home = temp_home("statuses");
    let channel = channel_with_home(home.clone());
    let error = call(&channel, "listWorkspaceMcpServerStatuses", object_arg("{}"))
        .await
        .expect_err("缺 workspacePath");
    assert!(
        error.contains("invalid listWorkspaceMcpServerStatuses request"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn unknown_command_is_rejected() {
    let home = temp_home("unknown");
    let channel = channel_with_home(home.clone());
    let error = call(&channel, "nope", object_arg("{}"))
        .await
        .expect_err("未知命令");
    assert!(error.contains("unknown command"), "{error}");
    let _ = std::fs::remove_dir_all(&home);
}
