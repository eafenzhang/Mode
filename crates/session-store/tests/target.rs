//! `session_target` 读写：单行覆盖语义、状态 CHECK 与 clear 删行。

mod common;

use common::{create_baseline_database, sample_session, FixtureOptions, TempRoot};
use session_store::{SessionStore, TargetRecord};

fn open_fixture(label: &str) -> (TempRoot, SessionStore) {
    let root = TempRoot::new(label);
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());
    let store = SessionStore::open(&db_path).expect("打开测试库");
    store
        .create_session(sample_session("sess_goal", r"C:\ws\goal", 100, false))
        .expect("写入 session");
    (root, store)
}

fn target(target_id: &str, objective: &str, status: &str) -> TargetRecord {
    TargetRecord {
        session_id: "sess_goal".to_string(),
        target_id: target_id.to_string(),
        objective: objective.to_string(),
        status: status.to_string(),
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        time_created: 1_000,
        time_updated: 1_000,
        summary_title: Some(objective.to_string()),
        active_input_id: None,
        active_run_started_at: None,
        active_run_last_seen_at: None,
    }
}

#[test]
fn target_upsert_overwrites_single_row_and_preserves_created_at() {
    let (_root, store) = open_fixture("target-upsert");
    assert_eq!(store.get_target("sess_goal").expect("读空目标"), None);

    let first = target("target-1", "Goal one", "active");
    store.upsert_target(&first).expect("首次写入");

    let mut second = target("target-2", "Goal two", "paused");
    second.time_created = 2_000;
    second.time_updated = 3_000;
    second.tokens_used = 22;
    second.time_used_seconds = 4;
    second.active_input_id = Some("goal_set".to_string());
    second.active_run_started_at = Some(2_500);
    second.active_run_last_seen_at = Some(2_600);
    store.upsert_target(&second).expect("覆盖写入");

    let read = store
        .get_target("sess_goal")
        .expect("读取目标")
        .expect("目标必须存在");
    // 覆盖写不改 `time_created`（首次创建时间保留），其余字段整行替换。
    assert_eq!(read.time_created, 1_000);
    assert_eq!(read.target_id, "target-2");
    assert_eq!(read.objective, "Goal two");
    assert_eq!(read.status, "paused");
    assert_eq!(read.tokens_used, 22);
    assert_eq!(read.time_used_seconds, 4);
    assert_eq!(read.active_input_id.as_deref(), Some("goal_set"));
    assert_eq!(read.active_run_started_at, Some(2_500));
    assert_eq!(read.active_run_last_seen_at, Some(2_600));
}

#[test]
fn delete_target_removes_row_and_reports_whether_present() {
    let (_root, store) = open_fixture("target-delete");
    store
        .upsert_target(&target("target-1", "Goal", "complete"))
        .expect("写入目标");
    assert!(store.delete_target("sess_goal").expect("删除目标"));
    assert!(!store.delete_target("sess_goal").expect("再次删除"));
    assert_eq!(store.get_target("sess_goal").expect("读目标"), None);
}
