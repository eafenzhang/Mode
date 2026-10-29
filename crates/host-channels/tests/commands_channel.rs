//! `commands` 频道适配器：6 个方法的参数形状、camelCase 返回与错误映射。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use commands_service::{CommandsService, CommandsServiceOptions};
use host_channels::commands::{
    DELETE_COMMAND_FILE, GET_PRIMARY_USER_COMMANDS_DIRECTORY, LIST, SET_COMMAND_ENABLED,
    UPDATE_COMMAND_FILE, WRITE_COMMAND_FILE,
};
use host_channels::CommandsChannel;
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-commands-channel-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn channel(home: &Path) -> CommandsChannel {
    let mut options = CommandsServiceOptions::with_home(home);
    options.write_options.use_file_lock = false;
    CommandsChannel::new(Arc::new(CommandsService::new(options)))
}

async fn call(channel: &CommandsChannel, method: &str, arg: Value) -> Result<Value, ChannelError> {
    channel
        .call("ctx".into(), method.into(), arg, CancellationToken::default())
        .await
}

fn object(value: serde_json::Value) -> Value {
    Value::Array(vec![Value::Json(value.to_string())])
}

fn empty_object() -> Value {
    object(serde_json::json!({}))
}

fn json(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

/// 按 `/` 分段拼接，保证与 `Path::join` 在 Windows 上产生同一串反斜杠。
fn p(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|segment| !segment.is_empty())
        .fold(base.to_path_buf(), |path, segment| path.join(segment))
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[tokio::test]
async fn list_returns_camel_case_shape() {
    let root = TempRoot::new("list");
    let home = root.path().join("home");
    write(&p(&home, ".mode/commands/hello.md"), "hello body");
    let channel = channel(&home);

    let result = json(&call(&channel, LIST, empty_object()).await.unwrap());
    let user = &result["userCommands"][0];

    assert_eq!(user["name"], "/hello");
    assert_eq!(user["prompt"], "hello body");
    assert_eq!(
        user["filePath"],
        p(&home, ".mode/commands/hello.md").to_string_lossy().as_ref()
    );
    assert_eq!(user["agentSource"], "modeAgent");
    assert_eq!(user["source"], "user");
    assert_eq!(user["scope"], "global");
    assert_eq!(user["enabled"], true);
    assert_eq!(user["id"], "modeAgent:mode:global:/hello");
    assert_eq!(user["location"]["source"], "mode");
    assert_eq!(user["location"]["scope"], "user");
    assert!(user["description"].is_null());
    assert!(user["projectPath"].is_null());
    assert_eq!(result["commands"].as_array().unwrap().len(), 1);
    assert_eq!(result["pluginCommands"].as_array().unwrap().len(), 0);
    assert_eq!(result["capability"]["userScopeAvailable"], true);
}

#[tokio::test]
async fn list_accepts_workspace_path_and_ignores_workspace_identity() {
    let root = TempRoot::new("list-workspace");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    write(&p(&workspace, ".mode/commands/project.md"), "project body");
    let channel = channel(&home);

    let result = json(
        &call(
            &channel,
            LIST,
            object(serde_json::json!({
                "workspacePath": workspace.to_string_lossy(),
                "workspaceIdentity": "anything"
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(result["userCommands"][0]["name"], "/project");
    assert_eq!(result["userCommands"][0]["scope"], "project");
    assert_eq!(
        result["userCommands"][0]["projectPath"],
        workspace.to_string_lossy().as_ref()
    );
}

#[tokio::test]
async fn write_update_enable_and_delete_round_trip() {
    let root = TempRoot::new("round-trip");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let channel = channel(&home);

    let written = json(
        &call(
            &channel,
            WRITE_COMMAND_FILE,
            object(serde_json::json!({
                "config": { "name": "/demo", "prompt": "do it", "description": "desc" }
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(written["command"]["name"], "/demo");
    assert_eq!(written["command"]["description"], "desc");
    assert_eq!(written["command"]["enabled"], true);

    let file = p(&home, ".mode/commands/demo.md");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "---\ndescription: desc\n---\n\ndo it"
    );

    // 重复写：错误消息原样透传。
    let error = call(
        &channel,
        WRITE_COMMAND_FILE,
        object(serde_json::json!({ "config": { "name": "/demo", "prompt": "again" } })),
    )
    .await
    .unwrap_err();
    match error {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "Command file already exists: demo.md")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let renamed = json(
        &call(
            &channel,
            UPDATE_COMMAND_FILE,
            object(serde_json::json!({
                "commandId": "modeAgent:mode:global:/demo",
                "config": { "name": "/renamed", "prompt": "renamed" },
                "oldFilePath": file.to_string_lossy()
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(renamed["command"]["name"], "/renamed");
    assert!(!file.exists());

    let renamed_file = p(&home, ".mode/commands/renamed.md");
    assert_eq!(
        call(
            &channel,
            SET_COMMAND_ENABLED,
            object(serde_json::json!({
                "commandId": "modeAgent:mode:global:/renamed",
                "filePath": renamed_file.to_string_lossy(),
                "enabled": false
            })),
        )
        .await
        .unwrap(),
        Value::Undefined
    );
    let listed = json(&call(&channel, LIST, empty_object()).await.unwrap());
    assert_eq!(listed["userCommands"][0]["enabled"], false);

    assert_eq!(
        call(
            &channel,
            DELETE_COMMAND_FILE,
            object(serde_json::json!({
                "commandId": "modeAgent:mode:global:/renamed",
                "filePath": renamed_file.to_string_lossy()
            })),
        )
        .await
        .unwrap(),
        Value::Undefined
    );
    assert!(!renamed_file.exists());
    let listed = json(&call(&channel, LIST, empty_object()).await.unwrap());
    assert_eq!(listed["userCommands"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn primary_directory_is_created_and_returned() {
    let root = TempRoot::new("primary");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let channel = channel(&home);

    let result = json(
        &call(&channel, GET_PRIMARY_USER_COMMANDS_DIRECTORY, empty_object())
            .await
            .unwrap(),
    );
    assert_eq!(
        result["path"],
        p(&home, ".mode/commands").to_string_lossy().as_ref()
    );
    assert!(p(&home, ".mode/commands").is_dir());
}

#[tokio::test]
async fn project_storage_without_workspace_is_an_error() {
    let root = TempRoot::new("project-error");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let channel = channel(&home);

    let error = call(
        &channel,
        WRITE_COMMAND_FILE,
        object(serde_json::json!({
            "config": { "name": "/x", "prompt": "p" },
            "storageLevel": "project"
        })),
    )
    .await
    .unwrap_err();
    match error {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "Missing workspace path for project command")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}

#[tokio::test]
async fn invalid_and_unknown_calls_are_errors() {
    let root = TempRoot::new("errors");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let channel = channel(&home);

    let missing = call(&channel, LIST, Value::Array(Vec::new())).await.unwrap_err();
    match missing {
        ChannelError::Error { message, .. } => assert_eq!(message, "missing arguments"),
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let not_object = call(&channel, LIST, Value::Array(vec![Value::Str("x".into())]))
        .await
        .unwrap_err();
    match not_object {
        ChannelError::Error { message, .. } => assert_eq!(message, "arguments must be an object"),
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let unknown = call(&channel, "nope", empty_object()).await.unwrap_err();
    match unknown {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "commands channel: unknown command nope")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}
