//! 与 TS 的黄金数据对拍：迁移 checksum、schema 快照、0002 旧选择导入。
//!
//! fixture 由 `pnpm tasks-db:fixtures` 用真实 TS 实现生成；Rust 侧只能设法通过。

use rusqlite::{params, Connection};
use serde_json::Value;
use tasks_db::{
    build_tasks_schema_snapshot, import_legacy_automation_selections, migration_definitions,
    run_migrations, RunOptions,
};

const FIXTURE: &str = include_str!("../fixtures/tasks-db.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("fixture 必须是合法 JSON")
}

#[test]
fn checksums_match_ts_ledger() {
    let fixture = fixture();
    let expected = fixture["migrations"]
        .as_array()
        .expect("migrations 必须是数组");
    let actual = migration_definitions();
    assert_eq!(actual.len(), expected.len(), "迁移数量必须一致");
    for (definition, expected) in actual.iter().zip(expected) {
        assert_eq!(
            definition.id,
            expected["id"].as_str().expect("id 必须是字符串")
        );
        assert_eq!(
            definition.checksum,
            expected["checksum"]
                .as_str()
                .expect("checksum 必须是字符串"),
            "迁移 {} 的 checksum 必须与 TS 真实账本一致",
            definition.id
        );
    }
}

#[test]
fn snapshot_matches_ts() {
    let actual = serde_json::to_value(build_tasks_schema_snapshot().expect("快照必须可构建"))
        .expect("快照必须可序列化");
    assert_eq!(
        actual,
        fixture()["snapshot"],
        "Rust 快照必须与 buildTasksSchemaSnapshot() 深度相等"
    );
}

#[test]
fn legacy_selection_import_matches_ts() {
    let fixture = fixture();
    let cases = fixture["legacySelection"]["cases"]
        .as_array()
        .expect("cases 必须是数组");
    assert!(!cases.is_empty(), "fixture 不能为空");

    let conn = Connection::open_in_memory().expect("内存库");
    run_migrations(&conn, RunOptions::default()).expect("空库迁移");
    let mut insert = conn
        .prepare(
            "INSERT INTO automations
               (automation_id, cron_expr, prompt, workspace_key, workspace_path,
                model, provider, thought_level, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .expect("prepare insert");
    for case in cases {
        insert
            .execute(params![
                case["id"].as_str().expect("id 必须是字符串"),
                "0 0 * * *",
                "prompt",
                "workspace-key",
                "/workspace",
                case["model"].as_str(),
                case["provider"].as_str(),
                case["thoughtLevel"].as_str(),
                1_i64,
                1_i64,
            ])
            .expect("insert legacy row");
    }
    drop(insert);
    import_legacy_automation_selections(&conn).expect("0002 导入");

    let mut select = conn
        .prepare("SELECT model_selection FROM automations WHERE automation_id=?")
        .expect("prepare select");
    for case in cases {
        let id = case["id"].as_str().expect("id 必须是字符串");
        let actual: Option<String> = select
            .query_row([id], |row| row.get(0))
            .expect("row 必须存在");
        assert_eq!(
            actual.as_deref(),
            case["expected"].as_str(),
            "旧行 {id} 的 model_selection 原字符串必须与 TS 一致"
        );
    }
}
