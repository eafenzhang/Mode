//! `subagents` 频道适配器：8 个方法的参数形状、camelCase 返回与错误映射。
//!
//! 行为本身的 fixture 对拍在 `crates/subagents-service/tests/replay.rs`；
//! 这里只验证传输层：位置参数解析、桌面运行时能力位与错误文本透传。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use host_channels::subagents::{
    CREATE_AGENT, DELETE_AGENT, GET_PRIMARY_USER_AGENTS_DIRECTORY, LIST,
    SET_BUILT_IN_MODEL_OVERRIDE, SET_ENABLED, SET_PLUGIN_AGENT_MODEL_OVERRIDE, UPDATE_AGENT,
};
use host_channels::SubagentsChannel;
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use subagents_service::{StorageOptions, SubagentsService, SubagentsServiceOptions};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-subagents-channel-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(canonical_long_path(&path))
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

fn canonical_long_path(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    canonical
}

fn channel(home: &Path, desktop: bool) -> SubagentsChannel {
    SubagentsChannel::new(Arc::new(SubagentsService::new(SubagentsServiceOptions {
        storage: StorageOptions::with_home(home.to_path_buf()),
        is_desktop_runtime: Some(desktop),
    })))
}

async fn call(channel: &SubagentsChannel, method: &str, arg: Value) -> Result<Value, ChannelError> {
    channel
        .call(
            "ctx".into(),
            method.into(),
            arg,
            CancellationToken::default(),
        )
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

fn agent_file(name: &str, description: &str) -> String {
    format!("---\nname: \"{name}\"\ndescription: \"{description}\"\n---\n{name} body\n")
}

fn id_by_name(listed: &serde_json::Value, name: &str) -> String {
    listed["agents"]
        .as_array()
        .and_then(|agents| {
            agents
                .iter()
                .find(|agent| agent["name"] == serde_json::Value::String(name.into()))
        })
        .and_then(|agent| agent["id"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("list 里找不到 agent {name}: {listed}"))
}

async fn listed(channel: &SubagentsChannel, workspace: &Path) -> serde_json::Value {
    json(
        &call(
            channel,
            LIST,
            object(serde_json::json!({ "workspacePath": workspace.to_string_lossy() })),
        )
        .await
        .unwrap(),
    )
}

#[tokio::test]
async fn list_returns_camel_case_builtins_and_agents() {
    let root = TempRoot::new("list");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    write(
        &p(&workspace, ".mode/agents/alpha.md"),
        &agent_file("alpha", "workspace alpha"),
    );
    write(
        &p(&home, ".mode/agents/userone.md"),
        &agent_file("userone", "user agent"),
    );
    let channel = channel(&home, true);

    let result = json(
        &call(
            &channel,
            LIST,
            object(serde_json::json!({
                "workspacePath": workspace.to_string_lossy(),
                "workspaceIdentity": "we-1",
                "provider": "glm",
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(result["capability"]["userScopeAvailable"], true);
    assert!(result["capability"].get("userScopeReason").is_none());
    let names: Vec<&str> = result["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"general-purpose"));
    assert!(names.contains(&"Explore"));
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"userone"));

    let alpha = result["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"] == "alpha")
        .unwrap();
    assert_eq!(alpha["scope"], "workspace");
    assert_eq!(alpha["source"], "user");
    assert_eq!(alpha["systemPrompt"], "alpha body");
    assert_eq!(
        alpha["path"],
        p(&workspace, ".mode/agents/alpha.md")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(alpha["projectPath"], workspace.to_string_lossy().as_ref());
    assert_eq!(alpha["id"], "user:workspace:alpha");

    let user_agent_names: Vec<&str> = result["userAgents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap())
        .collect();
    // 与 TS 一致：先用户根、后工作区根。
    assert_eq!(user_agent_names, vec!["userone", "alpha"]);
}

#[tokio::test]
async fn list_hides_user_scope_when_not_desktop() {
    let root = TempRoot::new("list-cli");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    write(
        &p(&workspace, ".mode/agents/alpha.md"),
        &agent_file("alpha", "workspace alpha"),
    );
    write(
        &p(&home, ".mode/agents/userone.md"),
        &agent_file("userone", "user agent"),
    );
    let channel = channel(&home, false);
    let result = json(
        &call(
            &channel,
            LIST,
            object(serde_json::json!({ "workspacePath": workspace.to_string_lossy() })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(result["capability"]["userScopeAvailable"], false);
    assert_eq!(result["capability"]["userScopeReason"], "desktop_only");
    let names: Vec<&str> = result["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"userone"));
    assert!(names.contains(&"alpha"));
}

#[tokio::test]
async fn settings_user_only_omits_workspace_agents() {
    let root = TempRoot::new("settings");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    write(
        &p(&workspace, ".mode/agents/alpha.md"),
        &agent_file("alpha", "workspace alpha"),
    );
    write(
        &p(&home, ".mode/agents/userone.md"),
        &agent_file("userone", "user agent"),
    );
    let channel = channel(&home, true);

    let result = json(
        &call(
            &channel,
            LIST,
            object(serde_json::json!({
                "workspacePath": workspace.to_string_lossy(),
                "mode": "settingsUserOnly",
            })),
        )
        .await
        .unwrap(),
    );
    let names: Vec<&str> = result["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"alpha"));
    assert!(names.contains(&"userone"));

    let unknown = call(
        &channel,
        LIST,
        object(serde_json::json!({
            "workspacePath": workspace.to_string_lossy(),
            "mode": "other",
        })),
    )
    .await
    .unwrap_err();
    match unknown {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "invalid arguments: unknown list mode other")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}

#[tokio::test]
async fn enabled_and_override_round_trip() {
    let root = TempRoot::new("state");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&workspace).unwrap();
    write(
        &p(&home, ".mode/agents/userone.md"),
        &agent_file("userone", "user agent"),
    );
    let channel = channel(&home, true);
    let result = listed(&channel, &workspace).await;
    let agent_id = id_by_name(&result, "userone");

    call(
        &channel,
        SET_ENABLED,
        object(serde_json::json!({ "agentId": agent_id, "enabled": false })),
    )
    .await
    .unwrap();
    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p(&home, ".mode/v2/agents-state.json")).unwrap())
            .unwrap();
    assert_eq!(state["disabledAgentIds"][0], agent_id);
    let result = listed(&channel, &workspace).await;
    let userone = result["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"] == "userone")
        .unwrap();
    assert_eq!(userone["enabled"], false);

    call(
        &channel,
        SET_ENABLED,
        object(serde_json::json!({ "agentId": agent_id, "enabled": true })),
    )
    .await
    .unwrap();
    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p(&home, ".mode/v2/agents-state.json")).unwrap())
            .unwrap();
    assert_eq!(state["disabledAgentIds"], serde_json::json!([]));

    call(
        &channel,
        SET_BUILT_IN_MODEL_OVERRIDE,
        object(serde_json::json!({
            "agentName": "general-purpose",
            "modelSelection": {
                "providerId": "glm",
                "modelId": "GLM-5",
                "options": { "reasoningLevel": "high" }
            }
        })),
    )
    .await
    .unwrap();
    call(
        &channel,
        SET_PLUGIN_AGENT_MODEL_OVERRIDE,
        object(serde_json::json!({
            "agentId": "plugin:plug@inline:helper",
            "modelSelection": { "providerId": "glm", "modelId": "GLM-5" }
        })),
    )
    .await
    .unwrap();
    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p(&home, ".mode/v2/agents-state.json")).unwrap())
            .unwrap();
    assert_eq!(
        state["builtInModelSelectionOverrides"]["general-purpose"]["options"]["reasoningLevel"],
        "high"
    );
    assert_eq!(
        state["pluginAgentModelSelectionOverrides"]["plugin:plug@inline:helper"]["modelId"],
        "GLM-5"
    );

    let invalid = call(
        &channel,
        SET_PLUGIN_AGENT_MODEL_OVERRIDE,
        object(serde_json::json!({ "agentId": "not-a-plugin-id" })),
    )
    .await
    .unwrap_err();
    match invalid {
        ChannelError::Error { message, .. } => assert_eq!(message, "无效插件 Subagent 身份"),
        other => panic!("期望 Error，实际 {other:?}"),
    }
}

#[tokio::test]
async fn create_update_delete_round_trip() {
    let root = TempRoot::new("mutate");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let channel = channel(&home, true);

    let created = json(
        &call(
            &channel,
            CREATE_AGENT,
            object(serde_json::json!({
                "config": {
                    "name": "new-one",
                    "description": "created",
                    "systemPrompt": "do it",
                    "color": "blue",
                    "modelSelection": { "providerId": "glm", "modelId": "GLM-5" },
                    "tools": ["Read"],
                    "permissionMode": "auto",
                    "maxTurns": 2,
                    "background": false,
                },
                "scope": "workspace",
                "workspacePath": workspace.to_string_lossy(),
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(created["agent"]["id"], "user:workspace:new-one");
    assert_eq!(created["agent"]["name"], "new-one");
    let created_file = p(&workspace, ".mode/agents/new-one.md");
    let markdown = fs::read_to_string(&created_file).unwrap();
    assert!(markdown.contains("name: \"new-one\""));
    assert!(markdown.contains("model: glm/GLM-5"));
    assert!(markdown.ends_with("---\n\ndo it\n"));

    let duplicate = call(
        &channel,
        CREATE_AGENT,
        object(serde_json::json!({
            "config": { "name": "new-one", "description": "d", "systemPrompt": "p" },
            "scope": "workspace",
            "workspacePath": workspace.to_string_lossy(),
        })),
    )
    .await
    .unwrap_err();
    match duplicate {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "Agent file \"new-one.md\" already exists")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let short = call(
        &channel,
        CREATE_AGENT,
        object(serde_json::json!({
            "config": { "name": "ab", "description": "d", "systemPrompt": "p" },
        })),
    )
    .await
    .unwrap_err();
    match short {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "Name must be between 3 and 50 characters")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let reserved = call(
        &channel,
        CREATE_AGENT,
        object(serde_json::json!({
            "config": { "name": "Explore", "description": "d", "systemPrompt": "p" },
        })),
    )
    .await
    .unwrap_err();
    match reserved {
        ChannelError::Error { message, .. } => {
            assert_eq!(
                message,
                "Agent name \"Explore\" is reserved by a built-in agent"
            )
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let updated = json(
        &call(
            &channel,
            UPDATE_AGENT,
            object(serde_json::json!({
                "agentId": "user:workspace:new-one",
                "config": {
                    "name": "renamed",
                    "description": "renamed desc",
                    "systemPrompt": "renamed prompt",
                },
                "oldFilePath": created_file.to_string_lossy(),
                "scope": "workspace",
                "workspacePath": workspace.to_string_lossy(),
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(updated["agent"]["id"], "user:workspace:renamed");
    assert!(!created_file.exists());
    let renamed_file = p(&workspace, ".mode/agents/renamed.md");
    assert!(renamed_file.is_file());

    call(
        &channel,
        DELETE_AGENT,
        object(serde_json::json!({
            "agentId": "user:workspace:renamed",
            "filePath": renamed_file.to_string_lossy(),
        })),
    )
    .await
    .unwrap();
    assert!(!renamed_file.exists());
}

#[tokio::test]
async fn primary_directory_is_created() {
    let root = TempRoot::new("primary");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let channel = channel(&home, true);
    let result = json(
        &call(
            &channel,
            GET_PRIMARY_USER_AGENTS_DIRECTORY,
            object(serde_json::json!({ "provider": "glm" })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(
        result["path"],
        p(&home, ".mode/agents").to_string_lossy().as_ref()
    );
    assert!(p(&home, ".mode/agents").is_dir());
}

#[tokio::test]
async fn invalid_and_unknown_calls_are_errors() {
    let root = TempRoot::new("errors");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let channel = channel(&home, true);

    let missing = call(&channel, LIST, Value::Array(Vec::new()))
        .await
        .unwrap_err();
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

    let missing_field = call(&channel, LIST, empty_object()).await.unwrap_err();
    match missing_field {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "invalid arguments: missing field `workspacePath`")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let unknown = call(&channel, "nope", empty_object()).await.unwrap_err();
    match unknown {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "subagents channel: unknown command nope")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}
