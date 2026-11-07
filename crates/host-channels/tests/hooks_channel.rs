//! `hooks` 频道适配器契约：load/save 的形状与 grant 转发边界。
//!
//! load/save 是纯文件实现，不拉起 Agent；grant 必须转发给 Agent authority。

use std::path::PathBuf;
use std::sync::Arc;

use agent_process::{AgentControlClient, AgentRuntimeRegistry, SpecFactory};
use host_channels::{HooksChannel, HOOKS_CHANNEL};
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mode-hooks-channel-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn channel(home: PathBuf) -> HooksChannel {
    let factory: SpecFactory = Arc::new(|_target| panic!("load/save 不应拉起 runtime"));
    let registry = AgentRuntimeRegistry::new(factory);
    HooksChannel::with_home_dir(
        Arc::new(AgentControlClient::new(Arc::new(registry))),
        home,
    )
}

async fn call(
    channel: &HooksChannel,
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

fn payload(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).unwrap(),
        other => panic!("unexpected value: {other:?}"),
    }
}

fn write_project_hooks(workspace: &std::path::Path) {
    let dir = workspace.join(".mode");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.json"),
        r#"{"hooks":{"enabled":true,"events":{"Stop":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}}"#,
    )
    .unwrap();
}

#[test]
fn channel_name_matches_service_channel() {
    assert_eq!(HOOKS_CHANNEL, "hooks");
}

#[tokio::test]
async fn load_hooks_returns_camel_case_shape() {
    let root = temp_dir("load");
    let workspace = root.join("ws");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_project_hooks(&workspace);
    let channel = channel(home);

    let value = call(
        &channel,
        "loadHooks",
        object_arg(&format!(
            r#"{{"workspacePath":{}}}"#,
            serde_json::to_string(&workspace.to_string_lossy()).unwrap()
        )),
    )
    .await
    .unwrap();
    let body = payload(&value);
    assert_eq!(body["hooksEnabled"], true);
    assert!(!body["hooks"].as_array().unwrap().is_empty());
    assert!(body["workspaceHookSnapshot"].is_object());
    assert!(body.get("trustStoreCorrupt").is_none());

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn load_hooks_uses_provided_workspace_identity() {
    let root = temp_dir("identity");
    let workspace = root.join("ws");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_project_hooks(&workspace);
    let channel = channel(home);

    let value = call(
        &channel,
        "loadHooks",
        object_arg(&format!(
            r#"{{"workspacePath":{},"workspaceIdentity":"remote:identity"}}"#,
            serde_json::to_string(&workspace.to_string_lossy()).unwrap()
        )),
    )
    .await
    .unwrap();
    let body = payload(&value);
    assert_eq!(
        body["workspaceHookSnapshot"]["workspaceIdentity"],
        "remote:identity"
    );

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn save_hooks_writes_project_config() {
    let root = temp_dir("save");
    let workspace = root.join("ws");
    let home = root.join("home");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let channel = channel(home);

    let hook = serde_json::json!({
        "id": "hook-mode-project-0",
        "event": "Stop",
        "enabled": true,
        "editable": true,
        "type": "command",
        "command": "echo saved",
        "location": { "source": "mode", "scope": "project" }
    });
    call(
        &channel,
        "saveHooks",
        object_arg(&format!(
            r#"{{"workspacePath":{},"hooks":[{}]}}"#,
            serde_json::to_string(&workspace.to_string_lossy()).unwrap(),
            hook
        )),
    )
    .await
    .unwrap();

    let config = std::fs::read_to_string(workspace.join(".mode").join("config.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert!(parsed["hooks"]["events"]["Stop"].is_array());

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn grant_rejects_invalid_digest_before_spawn() {
    let channel = channel(std::env::temp_dir());
    let error = call(
        &channel,
        "grantWorkspaceHookTrust",
        object_arg(
            r#"{"workspacePath":"D:/ws","bundleDigest":"not-a-digest","hookDeclarationDigest":"also-bad"}"#,
        ),
    )
    .await
    .expect_err("非法摘要");
    assert!(message(&error).contains("bundleDigest"), "{error:?}");
}

#[tokio::test]
async fn unknown_command_is_an_error() {
    let channel = channel(std::env::temp_dir());
    let error = call(&channel, "nope", object_arg("{}"))
        .await
        .expect_err("未知命令");
    assert!(message(&error).contains("unknown command"), "{error:?}");
}
