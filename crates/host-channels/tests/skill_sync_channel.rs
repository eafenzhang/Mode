//! `skill-sync` 频道适配器：候选/状态/导出的嵌套字节标记与导入还原。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use host_channels::skill_sync::{
    CHECK_REMOTE_USER_SKILL_WRITE_ACCESS, EXPORT_SKILLS_ARCHIVE, IMPORT_SKILLS_ARCHIVE,
    LIST_LOCAL_USER_SKILL_CANDIDATES, LIST_REMOTE_USER_SKILL_STATUSES,
};
use host_channels::SkillSyncChannel;
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::{nested_bytes, Value};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-skill-sync-channel-{label}-{}-{}",
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

fn channel(home: &Path) -> SkillSyncChannel {
    SkillSyncChannel::new(host_channels::skill_sync::service_for_home(
        home.to_path_buf(),
    ))
}

async fn call(channel: &SkillSyncChannel, method: &str, arg: Value) -> Result<Value, ChannelError> {
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
async fn export_marker_and_import_round_trip() {
    let source = TempRoot::new("source");
    let target = TempRoot::new("target");
    write(
        &p(source.path(), "home/.mode/skills/alpha/SKILL.md"),
        &skill_file("alpha"),
    );
    write(
        &p(source.path(), "home/.mode/skills/alpha/note.txt"),
        "note\n",
    );
    let source_channel = channel(&source.path().join("home"));

    let listed = json(
        &call(
            &source_channel,
            LIST_LOCAL_USER_SKILL_CANDIDATES,
            Value::Undefined,
        )
        .await
        .unwrap(),
    );
    let skill_id = listed["candidates"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(listed["candidates"][0]["directoryName"], "alpha");
    assert_eq!(listed["maxArchiveBytes"], 20 * 1024 * 1024u64);

    let exported = json(
        &call(
            &source_channel,
            EXPORT_SKILLS_ARCHIVE,
            object(serde_json::json!({ "skillIds": [skill_id] })),
        )
        .await
        .unwrap(),
    );
    let marker = &exported["archive"];
    assert!(nested_bytes::is_marker(marker));
    assert_eq!(exported["skills"][0]["name"], "alpha");
    let decoded = nested_bytes::from_marker(marker).expect("archive bytes");
    assert_eq!(
        exported["archiveBytes"].as_u64().unwrap(),
        decoded.len() as u64
    );

    // 用另一个 home 的频道导入导出的 archive。
    let target_channel = channel(&target.path().join("home"));
    let imported = json(
        &call(
            &target_channel,
            IMPORT_SKILLS_ARCHIVE,
            object(serde_json::json!({ "archive": marker, "overwrite": false })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(imported["results"][0]["status"], "synced");
    assert!(p(target.path(), "home/.mode/skills/alpha/SKILL.md").is_file());
    assert_eq!(
        fs::read_to_string(p(target.path(), "home/.mode/skills/alpha/note.txt")).unwrap(),
        "note\n"
    );

    // 再次导入 → targetExists。
    let again = json(
        &call(
            &target_channel,
            IMPORT_SKILLS_ARCHIVE,
            object(serde_json::json!({ "archive": marker })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(again["results"][0]["status"], "skipped");
}

#[tokio::test]
async fn remote_statuses_and_write_access() {
    let root = TempRoot::new("status");
    let home = root.path().join("home");
    write(
        &p(&home, ".agents/skills/only/SKILL.md"),
        &skill_file("agentsonly"),
    );
    let channel = channel(&home);

    let statuses = json(
        &call(
            &channel,
            LIST_REMOTE_USER_SKILL_STATUSES,
            object(serde_json::json!({
                "directoryNames": ["missing", "viaName"],
                "skills": [{ "directoryName": "viaName", "name": "agentsonly" }],
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(statuses["statuses"][0]["exists"], false);
    assert_eq!(statuses["statuses"][1]["exists"], true);
    assert!(statuses["statuses"][1]["path"]
        .as_str()
        .unwrap()
        .ends_with("only"));

    let access = json(
        &call(
            &channel,
            CHECK_REMOTE_USER_SKILL_WRITE_ACCESS,
            Value::Undefined,
        )
        .await
        .unwrap(),
    );
    assert_eq!(access["ok"], true);
}

#[tokio::test]
async fn invalid_and_unknown_calls_are_errors() {
    let root = TempRoot::new("errors");
    let channel = channel(&root.path().join("home"));

    let missing = call(
        &channel,
        IMPORT_SKILLS_ARCHIVE,
        Value::Array(vec![Value::Str("x".into())]),
    )
    .await
    .unwrap_err();
    match missing {
        ChannelError::Error { message, .. } => assert_eq!(message, "arguments must be an object"),
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let missing_archive = call(&channel, IMPORT_SKILLS_ARCHIVE, empty_object())
        .await
        .unwrap_err();
    match missing_archive {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "invalid arguments: missing field `archive`")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let bad_archive = call(
        &channel,
        IMPORT_SKILLS_ARCHIVE,
        object(serde_json::json!({ "archive": "not-bytes" })),
    )
    .await
    .unwrap_err();
    match bad_archive {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "invalid arguments: archive must be a Uint8Array")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let unsafe_name = call(
        &channel,
        LIST_REMOTE_USER_SKILL_STATUSES,
        object(serde_json::json!({ "directoryNames": ["../escape"] })),
    )
    .await
    .unwrap_err();
    match unsafe_name {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "unsafe skill sync path: ../escape")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let unknown = call(&channel, "nope", empty_object()).await.unwrap_err();
    match unknown {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "skill-sync channel: unknown command nope")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let missing_ids = call(
        &channel,
        EXPORT_SKILLS_ARCHIVE,
        object(serde_json::json!({})),
    )
    .await
    .unwrap_err();
    match missing_ids {
        ChannelError::Error { message, .. } => {
            assert!(message.starts_with("invalid arguments: missing field `skillIds`"))
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}
