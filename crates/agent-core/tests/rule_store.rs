//! `allow_project` 的跨会话持久化（D1 裁决 (a)：Rust 直连 sqlite）。
//!
//! 表结构来自 `probe-rule-store.mts` 的黑盒观察（spec §3.3），不是迁移实现。

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde_json::json;

use agent_core::approval::Ruleset;
use agent_core::permission::PermissionHandler;
use agent_core::provider::{ScriptStep, ScriptToolCall, ScriptedProvider};
use agent_core::rule_store::{scope_id_for, RuleStore, SqliteRuleStore, LOCAL_SETTING_DDL};
use agent_core::session::Session;
use agent_core::SessionEventType;
use agent_protocol::permission::{PermissionRequest, PermissionResponse};

fn temp_db(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mode-rule-store-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).expect("建临时目录");
    let path = dir.join("db.sqlite");
    let connection = Connection::open(&path).expect("建库");
    connection
        .execute_batch(LOCAL_SETTING_DDL)
        .expect("建 local_setting 表");
    path
}

fn ruleset(paths: &[&str]) -> Ruleset {
    let mut ruleset = Ruleset::new();
    ruleset.allow = paths
        .iter()
        .map(|path| agent_protocol::permission::PermissionRuleValue {
            tool_name: "Write".to_string(),
            rule_content: (*path).to_string(),
            extra: agent_protocol::ExtraFields::new(),
        })
        .collect();
    ruleset
}

struct ProjectOnce;

impl PermissionHandler for ProjectOnce {
    fn request(&mut self, request: &PermissionRequest) -> PermissionResponse {
        request.options[1].response.clone()
    }
}

#[test]
fn scope_id_matches_the_observed_normalization() {
    assert_eq!(scope_id_for("C:\\Users\\X\\ws"), "proj_c-users-x-ws");
    assert_eq!(
        scope_id_for("C:\\Users\\ADMINI~1\\AppData\\Local\\Temp\\mode-probe-rule-ws-ntx9AK"),
        "proj_c-users-admini-1-appdata-local-temp-mode-probe-rule-ws-ntx9ak"
    );
}

#[test]
fn save_and_load_round_trip_and_upsert_keeps_created_time() {
    let db = temp_db("roundtrip");
    let store = SqliteRuleStore::new(db.clone());
    assert!(store.load("proj_x").is_none(), "空库应返回 None");

    store
        .save("proj_x", &ruleset(&["C:/ws/a.txt"]))
        .expect("第一次写入");
    let connection = Connection::open(&db).expect("打开库");
    let first: (i64, i64) = connection
        .query_row(
            "SELECT time_created, time_updated FROM local_setting WHERE scope_id = 'proj_x'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("第一行");
    drop(connection);

    std::thread::sleep(Duration::from_millis(5));
    store
        .save("proj_x", &ruleset(&["C:/ws/a.txt", "C:/ws/b.txt"]))
        .expect("第二次写入（upsert）");

    let loaded = store.load("proj_x").expect("应能读回");
    assert_eq!(loaded.allow.len(), 2);

    let connection = Connection::open(&db).expect("打开库");
    let (scope, namespace, key, schema_version, created, updated): (
        String,
        String,
        String,
        i64,
        i64,
        i64,
    ) = connection
        .query_row(
            "SELECT scope, namespace, key, schema_version, time_created, time_updated \
             FROM local_setting WHERE scope_id = 'proj_x'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("行应存在");
    assert_eq!(scope, "project");
    assert_eq!(namespace, "permission");
    assert_eq!(key, "ruleset");
    assert_eq!(schema_version, 1);
    assert_eq!(created, first.0, "upsert 不应改 time_created");
    assert!(updated >= first.1, "upsert 应更新 time_updated");
}

#[test]
fn missing_db_file_is_a_save_error_not_a_panic() {
    let store = SqliteRuleStore::new(
        std::env::temp_dir()
            .join("mode-rule-store-missing")
            .join("db.sqlite"),
    );
    assert!(store.load("proj_x").is_none());
    assert!(store.save("proj_x", &ruleset(&["C:/ws/a.txt"])).is_err());
}

#[test]
fn allow_project_survives_into_a_new_session() {
    let db = temp_db("session");
    let dir = std::env::temp_dir().join(format!("mode-rule-ws-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("建工作目录");
    let path = dir.join("a.txt");
    let path_text = path.to_string_lossy().to_string();

    let provider = ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Write".to_string(),
            input: json!({ "file_path": path_text, "content": "a\n" }),
        }]),
        ScriptStep::Text("first done".to_string()),
    ]);
    let mut first = Session::new("sess_first", dir.to_string_lossy().to_string())
        .with_sqlite_rules(db.clone())
        .with_permission_handler(Box::new(ProjectOnce));
    let mut first_sink = Vec::new();
    first
        .run_turn("write once", &provider, &mut first_sink)
        .expect("第一个 turn");
    assert_eq!(first.rule_store_error(), None, "落盘不应失败");
    assert_eq!(fs::read_to_string(&path).expect("文件应存在"), "a\n");

    // 新会话：规则应从 sqlite 读回，同路径不再请求审批。
    let provider = ScriptedProvider::new(vec![
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Read".to_string(),
            input: json!({ "file_path": path_text }),
        }]),
        ScriptStep::ToolCalls(vec![ScriptToolCall {
            name: "Write".to_string(),
            input: json!({ "file_path": path_text, "content": "b\n" }),
        }]),
        ScriptStep::Text("second done".to_string()),
    ]);
    let mut second = Session::new("sess_second", dir.to_string_lossy().to_string())
        .with_sqlite_rules(db.clone());
    let mut second_sink = Vec::new();
    second
        .run_turn("write again", &provider, &mut second_sink)
        .expect("第二个 turn");

    let permission_requests = second_sink
        .iter()
        .filter(|event| event.event_type == SessionEventType::PermissionRequested)
        .count();
    assert_eq!(permission_requests, 0, "规则应跨会话生效");
    assert_eq!(fs::read_to_string(&path).expect("文件应存在"), "b\n");
}
