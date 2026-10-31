//! `skills` 频道适配器：6 个方法的参数形状、camelCase 返回与错误映射。
//!
//! 行为本身的 fixture 对拍在 `crates/skills-service/tests/replay.rs`；
//! 这里只验证传输层：位置参数解析、桌面运行时能力位与错误文本透传。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use host_channels::skills::{
    BUILD_PROMPT_CONTEXT, COPY_TO_COMMON, DELETE_SKILL, LIST, REMOVE_FROM_COMMON, SET_ENABLED,
};
use host_channels::SkillsChannel;
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use skills_service::{SkillsService, SkillsServiceOptions};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-skills-channel-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        // 临时目录可能是 8.3 短名，服务侧 realpath 会展开成长名；
        // 统一存储长名，避免同一路径出现两种文本形式。
        Self(canonical_long_path(&path))
    }

    fn path(&self) -> &Path {
        &self.0
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

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn channel(home: &Path, desktop: bool) -> SkillsChannel {
    SkillsChannel::new(Arc::new(SkillsService::new(SkillsServiceOptions {
        home_dir: home.to_path_buf(),
        is_desktop_runtime: desktop,
    })))
}

async fn call(channel: &SkillsChannel, method: &str, arg: Value) -> Result<Value, ChannelError> {
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

fn skill_file(name: &str) -> String {
    format!("---\nname: {name}\ndescription: {name} description\n---\n{name} body\n")
}

fn skill_id_by_name(listed: &serde_json::Value, name: &str) -> String {
    listed["skills"]
        .as_array()
        .and_then(|skills| {
            skills
                .iter()
                .find(|skill| skill["name"] == serde_json::Value::String(name.into()))
        })
        .and_then(|skill| skill["id"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("list 里找不到技能 {name}: {listed}"))
}

#[tokio::test]
async fn list_returns_camel_case_shape_with_user_scope() {
    let root = TempRoot::new("list");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    write(
        &p(&workspace, ".mode/skills/alpha/SKILL.md"),
        &skill_file("alpha"),
    );
    write(
        &p(&home, ".mode/skills/userone/SKILL.md"),
        &skill_file("userone"),
    );
    let channel = channel(&home, true);

    let listed = json(
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

    assert_eq!(listed["capability"]["userScopeAvailable"], true);
    assert!(listed["capability"].get("userScopeReason").is_none());
    let names: Vec<&str> = listed["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skill| skill["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["alpha", "userone"]);

    let alpha = &listed["skills"][0];
    assert_eq!(alpha["scope"], "workspace");
    assert_eq!(alpha["enabled"], true);
    assert_eq!(alpha["description"], "alpha description");
    assert_eq!(alpha["body"], "alpha body");
    assert_eq!(
        alpha["path"],
        p(&workspace, ".mode/skills/alpha/SKILL.md")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(alpha["sourcePath"], alpha["path"]);
    assert!(alpha["id"]
        .as_str()
        .unwrap()
        .starts_with("glm:workspace:alpha:"));
}

#[tokio::test]
async fn list_hides_user_scope_when_not_desktop() {
    let root = TempRoot::new("list-cli");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    write(
        &p(&workspace, ".mode/skills/alpha/SKILL.md"),
        &skill_file("alpha"),
    );
    write(
        &p(&home, ".mode/skills/userone/SKILL.md"),
        &skill_file("userone"),
    );
    let channel = channel(&home, false);

    let listed = json(
        &call(
            &channel,
            LIST,
            object(serde_json::json!({ "workspacePath": workspace.to_string_lossy() })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(listed["capability"]["userScopeAvailable"], false);
    assert_eq!(listed["capability"]["userScopeReason"], "desktop_only");
    let names: Vec<&str> = listed["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skill| skill["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["alpha"]);
}

#[tokio::test]
async fn set_enabled_round_trip_writes_cli_config() {
    let root = TempRoot::new("enabled");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    let skill_path = p(&workspace, ".mode/skills/alpha/SKILL.md");
    write(&skill_path, &skill_file("alpha"));
    let channel = channel(&home, true);

    let listed = json(
        &call(
            &channel,
            LIST,
            object(serde_json::json!({ "workspacePath": workspace.to_string_lossy() })),
        )
        .await
        .unwrap(),
    );
    let skill_id = skill_id_by_name(&listed, "alpha");

    assert_eq!(
        call(
            &channel,
            SET_ENABLED,
            object(serde_json::json!({
                "workspacePath": workspace.to_string_lossy(),
                "workspaceIdentity": "we-1",
                "scope": "workspace",
                "skillId": skill_id,
                "enabled": false,
            })),
        )
        .await
        .unwrap(),
        Value::Undefined
    );

    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p(&home, ".mode/cli/config.json")).unwrap())
            .unwrap();
    let key = skill_path.to_string_lossy().replace('\\', "/");
    assert_eq!(config["skills"][&key]["enable"], false);

    let listed = json(
        &call(
            &channel,
            LIST,
            object(serde_json::json!({ "workspacePath": workspace.to_string_lossy() })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(listed["skills"][0]["enabled"], false);

    // 开启态是默认值：删除 override 后 `skills` 字段整体消失。
    call(
        &channel,
        SET_ENABLED,
        object(serde_json::json!({
            "workspacePath": workspace.to_string_lossy(),
            "skillId": skill_id,
            "enabled": true,
        })),
    )
    .await
    .unwrap();
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(p(&home, ".mode/cli/config.json")).unwrap())
            .unwrap();
    assert!(config.get("skills").is_none());
}

#[tokio::test]
async fn build_prompt_context_activates_mentioned_skill_and_audits() {
    let root = TempRoot::new("prompt");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    write(
        &p(&workspace, ".mode/skills/alpha/SKILL.md"),
        &skill_file("alpha"),
    );
    let channel = channel(&home, true);

    let result = json(
        &call(
            &channel,
            BUILD_PROMPT_CONTEXT,
            object(serde_json::json!({
                "workspacePath": workspace.to_string_lossy(),
                "workspaceIdentity": "we-1",
                "prompt": "please use $alpha now",
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(result["activatedSkillNames"][0], "alpha");
    let prompt = result["prompt"].as_str().unwrap();
    assert!(prompt.starts_with("please use $alpha now\n\n<available_skills>"));
    assert!(prompt.contains("<activated_skill name=\"alpha\""));
    assert!(prompt.contains("alpha body"));
    assert!(prompt.ends_with("</available_skills>"));

    let audit = fs::read_to_string(p(&home, ".mode/v2/skills-audit.log")).unwrap();
    let line: serde_json::Value = serde_json::from_str(audit.trim()).unwrap();
    assert_eq!(line["workspacePath"], workspace.to_string_lossy().as_ref());
    assert_eq!(line["workspaceIdentity"], "we-1");
    assert_eq!(line["activatedSkillNames"][0], "alpha");

    // 未提及时原样返回，不写审计日志。
    let untouched = json(
        &call(
            &channel,
            BUILD_PROMPT_CONTEXT,
            object(serde_json::json!({
                "workspacePath": workspace.to_string_lossy(),
                "prompt": "no mentions",
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(untouched["prompt"], "no mentions");
    assert_eq!(untouched["activatedSkillNames"], serde_json::json!([]));
}

#[tokio::test]
async fn copy_remove_and_delete_round_trip() {
    let root = TempRoot::new("mutate");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    let agents_skill = p(&workspace, ".agents/skills/beta/SKILL.md");
    write(&agents_skill, &skill_file("beta"));
    let channel = channel(&home, true);

    let listed = json(
        &call(
            &channel,
            LIST,
            object(serde_json::json!({ "workspacePath": workspace.to_string_lossy() })),
        )
        .await
        .unwrap(),
    );
    let skill_id = skill_id_by_name(&listed, "beta");

    let copied = json(
        &call(
            &channel,
            COPY_TO_COMMON,
            object(serde_json::json!({
                "workspacePath": workspace.to_string_lossy(),
                "skillId": skill_id,
            })),
        )
        .await
        .unwrap(),
    );
    let copied_path = p(&workspace, ".mode/skills/beta/SKILL.md");
    assert_eq!(copied["newPath"], copied_path.to_string_lossy().as_ref());
    assert!(copied_path.is_file());

    // 复制后同名列两项：按 path 找到通用目录里的那一项再移除。
    let listed = json(
        &call(
            &channel,
            LIST,
            object(serde_json::json!({ "workspacePath": workspace.to_string_lossy() })),
        )
        .await
        .unwrap(),
    );
    let copied_id = listed["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| {
            skill["path"]
                .as_str()
                .is_some_and(|path| path.replace('\\', "/").contains("/.mode/skills/beta/"))
        })
        .and_then(|skill| skill["id"].as_str())
        .unwrap()
        .to_string();
    call(
        &channel,
        REMOVE_FROM_COMMON,
        object(serde_json::json!({
            "workspacePath": workspace.to_string_lossy(),
            "skillId": copied_id,
        })),
    )
    .await
    .unwrap();
    assert!(!copied_path.exists());

    call(
        &channel,
        DELETE_SKILL,
        object(serde_json::json!({
            "workspacePath": workspace.to_string_lossy(),
            "skillId": skill_id,
        })),
    )
    .await
    .unwrap();
    assert!(!agents_skill.exists());
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
            assert_eq!(message, "skills channel: unknown command nope")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let not_found = call(
        &channel,
        DELETE_SKILL,
        object(serde_json::json!({
            "workspacePath": workspace.to_string_lossy(),
            "skillId": "glm:workspace:ghost:000000000000",
        })),
    )
    .await
    .unwrap_err();
    match not_found {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "Skill not found: glm:workspace:ghost:000000000000")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}
