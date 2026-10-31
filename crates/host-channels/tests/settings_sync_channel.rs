//! `settings-sync` 频道适配器：6 个方法的参数形状、camelCase 返回与错误映射。
//!
//! 行为本身的 fixture 对拍在 `crates/settings-sync-service/tests/replay.rs`；
//! 这里只验证传输层。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use host_channels::settings_sync::{
    COPY_CLAUDE_AGENTS_FILE_TO_MODE_AGENTS_FILE, DETECT, GET_CLAUDE_AGENTS_FILE_MIGRATION_STATUS,
    GET_FIRST_RUN_PROMPT_STATE, IMPORT_SELECTED, MARK_FIRST_RUN_PROMPT_HANDLED,
};
use host_channels::SettingsSyncChannel;
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use settings_sync_service::{SettingsBridge, SettingsSyncService, SettingsSyncServiceOptions};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-settings-sync-channel-{label}-{}-{}",
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

#[derive(Default)]
struct MemorySettings(Mutex<serde_json::Map<String, serde_json::Value>>);

impl SettingsBridge for MemorySettings {
    fn get_settings(&self) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Object(self.0.lock().unwrap().clone()))
    }

    fn update_settings(&self, patch: &serde_json::Value) -> Result<(), String> {
        let mut settings = self.0.lock().unwrap();
        if let serde_json::Value::Object(patch) = patch {
            for (key, value) in patch {
                settings.insert(key.clone(), value.clone());
            }
        }
        Ok(())
    }
}

fn channel(home: &Path) -> SettingsSyncChannel {
    SettingsSyncChannel::new(Arc::new(SettingsSyncService::new(
        SettingsSyncServiceOptions {
            home_dir: Some(home.to_path_buf()),
        },
        Arc::new(MemorySettings::default()),
    )))
}

async fn call(
    channel: &SettingsSyncChannel,
    method: &str,
    arg: Value,
) -> Result<Value, ChannelError> {
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

fn skill_file(name: &str) -> String {
    format!("---\nname: {name}\ndescription: {name} description\n---\n{name} body\n")
}

#[tokio::test]
async fn detect_and_import_round_trip() {
    let root = TempRoot::new("import");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&workspace).unwrap();
    write(
        &p(&home, ".claude/skills/alpha/SKILL.md"),
        &skill_file("alpha"),
    );
    let channel = channel(&home);

    let detected = json(
        &call(
            &channel,
            DETECT,
            object(serde_json::json!({
                "categories": ["skills"],
                "intent": "manualImport",
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(detected["agents"][0]["agent"], "claudeCode");
    assert_eq!(detected["agents"][0]["categories"][0]["category"], "skills");
    assert_eq!(detected["agents"][0]["categories"][0]["discoveredCount"], 1);
    assert_eq!(
        detected["agents"][0]["categories"][0]["sourceRoots"][0]["skills"][0]["name"],
        "alpha"
    );

    let imported = json(
        &call(
            &channel,
            IMPORT_SELECTED,
            object(serde_json::json!({
                "workspacePath": workspace.to_string_lossy(),
                "selections": [{
                    "agent": "claudeCode",
                    "category": "skills",
                    "importMode": "copy",
                }],
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(imported["successCount"], 1);
    assert_eq!(imported["taskResults"][0]["status"], "success");
    assert!(p(&home, ".mode/skills/alpha/SKILL.md").is_file());

    // 再探测：目标已存在 → sameNameExists/targetExists，不再可导入。
    let detected = json(
        &call(
            &channel,
            DETECT,
            object(serde_json::json!({
                "categories": ["skills"],
                "intent": "manualImport",
            })),
        )
        .await
        .unwrap(),
    );
    let category = &detected["agents"][0]["categories"][0];
    assert_eq!(category["importableCount"], 0);
    assert_eq!(category["skippedCount"], 1);
}

#[tokio::test]
async fn agents_file_and_first_run_state() {
    let root = TempRoot::new("agents");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let channel = channel(&home);

    let status = json(
        &call(
            &channel,
            GET_CLAUDE_AGENTS_FILE_MIGRATION_STATUS,
            empty_object(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(status["sourceExists"], false);
    assert_eq!(status["supported"], false);
    assert_eq!(status["unavailableReason"], "missingSource");

    let skipped = json(
        &call(
            &channel,
            COPY_CLAUDE_AGENTS_FILE_TO_MODE_AGENTS_FILE,
            empty_object(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(skipped["status"], "skipped");
    assert_eq!(skipped["skippedReason"], "missingSource");

    write(&p(&home, ".claude/CLAUDE.md"), "agents body\n");
    let copied = json(
        &call(
            &channel,
            COPY_CLAUDE_AGENTS_FILE_TO_MODE_AGENTS_FILE,
            empty_object(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(copied["status"], "copied");
    assert_eq!(copied["overwritten"], false);
    assert_eq!(
        fs::read_to_string(p(&home, ".mode/AGENTS.md")).unwrap(),
        "agents body\n"
    );

    let again = json(
        &call(
            &channel,
            COPY_CLAUDE_AGENTS_FILE_TO_MODE_AGENTS_FILE,
            empty_object(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(again["status"], "skipped");
    assert_eq!(again["skippedReason"], "targetExists");

    // 首启状态读写。
    let state = json(
        &call(&channel, GET_FIRST_RUN_PROMPT_STATE, empty_object())
            .await
            .unwrap(),
    );
    assert_eq!(state["handled"], false);
    call(&channel, MARK_FIRST_RUN_PROMPT_HANDLED, empty_object())
        .await
        .unwrap();
    let state = json(
        &call(&channel, GET_FIRST_RUN_PROMPT_STATE, empty_object())
            .await
            .unwrap(),
    );
    assert_eq!(state["handled"], true);
}

#[tokio::test]
async fn workspace_scope_and_first_run_intent() {
    let root = TempRoot::new("workspace");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    write(
        &p(&workspace, ".claude/skills/wsone/SKILL.md"),
        &skill_file("wsone"),
    );
    let channel = channel(&home);

    let first_run = json(
        &call(
            &channel,
            DETECT,
            object(serde_json::json!({
                "categories": ["skills"],
                "intent": "firstRun",
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(first_run["agents"], serde_json::json!([]));

    let workspace_scan = json(
        &call(
            &channel,
            DETECT,
            object(serde_json::json!({
                "workspacePath": workspace.to_string_lossy(),
                "categories": ["skills"],
                "intent": "manualImport",
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(
        workspace_scan["agents"][0]["categories"][0]["sourceRoots"][0]["scope"],
        "project"
    );
}

#[tokio::test]
async fn invalid_and_unknown_calls_are_errors() {
    let root = TempRoot::new("errors");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let channel = channel(&home);

    let missing = call(&channel, DETECT, Value::Array(Vec::new()))
        .await
        .unwrap_err();
    match missing {
        ChannelError::Error { message, .. } => assert_eq!(message, "missing arguments"),
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let not_object = call(&channel, DETECT, Value::Array(vec![Value::Str("x".into())]))
        .await
        .unwrap_err();
    match not_object {
        ChannelError::Error { message, .. } => assert_eq!(message, "arguments must be an object"),
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let unknown = call(&channel, "nope", empty_object()).await.unwrap_err();
    match unknown {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "settings-sync channel: unknown command nope")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}
