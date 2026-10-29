//! 迁移 runner、adopt、inspect、prepare 的行为用例。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;
use tasks_db::{
    adopt_schema, are_migrations_applied, inspect_tasks_migration_kind, prepare_tasks_storage,
    run_migrations, MigrationFacts, MigrationKind, PrepareOptions, RunOptions, TasksDbErrorKind,
    TasksStoragePhase, AUTOMATION_SCHEMA, MIGRATION_IDS, OFF_PEAK_SCHEMA, TASK_INDEX_SCHEMA,
};

fn memory() -> Connection {
    Connection::open_in_memory().expect("内存库")
}

fn ledger(conn: &Connection) -> Vec<(String, String)> {
    let mut statement = conn
        .prepare("SELECT id, checksum FROM tasks_schema_migration ORDER BY id")
        .expect("prepare ledger");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query ledger")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect ledger")
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("pragma table_info");
    let mut rows = statement.query([]).expect("query pragma");
    while let Some(row) = rows.next().expect("next pragma row") {
        let name: String = row.get(1).expect("column name");
        if name == column {
            return true;
        }
    }
    false
}

fn has_index(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='index' AND name=?",
        [name],
        |_| Ok(true),
    )
    .unwrap_or(false)
}

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-tasks-db-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp root");
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

#[test]
fn empty_database_initializes_and_second_run_is_noop() {
    let conn = memory();
    let facts = run_migrations(&conn, RunOptions::default()).expect("initial migration");
    assert_eq!(facts.kind, MigrationKind::Initialize);
    assert_eq!(facts.executed_count, 3);
    assert_eq!(facts.committed_count, 3);
    assert_eq!(facts.last_applied_migration_id, Some(None));

    let rows = ledger(&conn);
    let ids: Vec<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, MIGRATION_IDS.to_vec());
    let time_applied: i64 = conn
        .query_row(
            "SELECT time_applied FROM tasks_schema_migration WHERE id='0001_adopt_task_schema'",
            [],
            |row| row.get(0),
        )
        .expect("time_applied");
    assert!(time_applied > 0);
    assert!(has_index(&conn, "idx_off_peak_bound_active"));
    assert!(are_migrations_applied(&conn).expect("are applied"));

    let facts = run_migrations(&conn, RunOptions::default()).expect("second migration run");
    assert_eq!(facts.kind, MigrationKind::None);
    assert_eq!(facts.executed_count, 0);
    assert_eq!(facts.committed_count, 0);
    assert_eq!(
        facts.last_applied_migration_id,
        Some(Some("0003_official_glm_selection".to_owned()))
    );
}

#[test]
fn tampered_ledger_reports_checksum_mismatch_with_migration_id() {
    let conn = memory();
    run_migrations(&conn, RunOptions::default()).expect("initial migration");
    conn.execute(
        "UPDATE tasks_schema_migration SET checksum='tampered' WHERE id='0001_adopt_task_schema'",
        [],
    )
    .expect("tamper");

    let error = run_migrations(&conn, RunOptions::default()).expect_err("must fail");
    assert_eq!(error.kind, Some(TasksDbErrorKind::ChecksumMismatch));
    assert_eq!(error.migration_id.as_deref(), Some("0001_adopt_task_schema"));
    assert!(error.message.contains("0001_adopt_task_schema"));

    let inspect_error = inspect_tasks_migration_kind(&conn).expect_err("inspect must fail");
    assert_eq!(
        inspect_error.kind,
        Some(TasksDbErrorKind::ChecksumMismatch)
    );
    assert_eq!(
        inspect_error.migration_id.as_deref(),
        Some("0001_adopt_task_schema")
    );

    let applied_error = are_migrations_applied(&conn).expect_err("are_applied must fail");
    assert_eq!(
        applied_error.kind,
        Some(TasksDbErrorKind::ChecksumMismatch)
    );
}

#[test]
fn inspect_distinguishes_initialize_upgrade_and_none() {
    let fresh = memory();
    assert_eq!(
        inspect_tasks_migration_kind(&fresh).expect("inspect fresh"),
        MigrationKind::Initialize
    );
    fresh
        .execute_batch("CREATE TABLE legacy_other (id TEXT)")
        .expect("create other table");
    assert_eq!(
        inspect_tasks_migration_kind(&fresh).expect("inspect upgrade"),
        MigrationKind::Upgrade
    );

    assert!(!are_migrations_applied(&fresh).expect("not applied"));
    run_migrations(&fresh, RunOptions::default()).expect("migrate");
    assert_eq!(
        inspect_tasks_migration_kind(&fresh).expect("inspect migrated"),
        MigrationKind::None
    );
}

#[test]
fn adopt_schema_upgrades_legacy_tables_and_backfills_scheduled_count() {
    let conn = memory();
    conn.execute_batch(
        "
        CREATE TABLE tasks (
          workspace_key TEXT NOT NULL,
          workspace_path TEXT NOT NULL,
          workspace_identity TEXT,
          task_id TEXT NOT NULL,
          title TEXT NOT NULL DEFAULT '',
          task_status TEXT,
          provider TEXT,
          mode TEXT NOT NULL DEFAULT 'build',
          model TEXT,
          migration_source TEXT,
          forked_from_task_id TEXT,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          unread_at INTEGER,
          last_unread_at INTEGER NOT NULL DEFAULT 0,
          pinned INTEGER NOT NULL DEFAULT 0,
          archived INTEGER NOT NULL DEFAULT 0,
          deleted INTEGER NOT NULL DEFAULT 0,
          title_overridden INTEGER NOT NULL DEFAULT 0,
          meta_json TEXT NOT NULL DEFAULT '{}',
          PRIMARY KEY (workspace_key, task_id)
        );
        CREATE TABLE automations (
          automation_id TEXT PRIMARY KEY,
          cron_expr TEXT NOT NULL,
          prompt TEXT NOT NULL,
          model TEXT,
          provider TEXT,
          workspace_key TEXT NOT NULL,
          workspace_path TEXT NOT NULL,
          enabled INTEGER NOT NULL DEFAULT 1,
          next_run_at INTEGER,
          retry_at INTEGER,
          run_count INTEGER NOT NULL DEFAULT 0,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        );
        INSERT INTO automations
          (automation_id, cron_expr, prompt, workspace_key, workspace_path, run_count, created_at, updated_at)
          VALUES ('a1', '0 0 * * *', 'prompt', 'ws', '/ws', 7, 1, 1);
        ",
    )
    .expect("legacy schema");

    let facts = run_migrations(&conn, RunOptions::default()).expect("upgrade migration");
    assert_eq!(facts.kind, MigrationKind::Upgrade);
    assert_eq!(facts.executed_count, 3);

    assert!(has_column(&conn, "tasks", "searchable_text"));
    assert!(has_column(&conn, "tasks", "cron_automation_id"));
    assert!(has_column(&conn, "tasks", "off_peak_task_id"));
    assert!(has_column(&conn, "automations", "scheduled_run_count"));
    assert!(has_index(&conn, "idx_tasks_cron_automation"));
    assert!(has_index(&conn, "idx_tasks_off_peak_task"));
    assert!(has_index(&conn, "idx_automations_target_task"));

    let scheduled: i64 = conn
        .query_row(
            "SELECT scheduled_run_count FROM automations WHERE automation_id='a1'",
            [],
            |row| row.get(0),
        )
        .expect("backfilled scheduled_run_count");
    assert_eq!(scheduled, 7);
}

#[test]
fn duplicate_active_bindings_skip_unique_bound_index() {
    let conn = memory();
    conn.execute_batch(&format!(
        "{TASK_INDEX_SCHEMA}{AUTOMATION_SCHEMA}{OFF_PEAK_SCHEMA}"
    ))
    .expect("base schema");
    for id in ["op1", "op2"] {
        conn.execute(
            "INSERT INTO off_peak_tasks
               (off_peak_task_id, prompt, permission_mode, workspace_key, workspace_path,
                status, session_id, queued_at, created_at, updated_at)
             VALUES (?, 'prompt', 'default', 'ws', '/ws', 'queued', 'session-1', 1, 1, 1)",
            [id],
        )
        .expect("insert duplicate");
    }

    adopt_schema(&conn).expect("adopt");
    assert!(!has_index(&conn, "idx_off_peak_bound_active"));
    assert!(has_index(&conn, "idx_off_peak_pick"));
    assert!(!are_migrations_applied(&conn).expect("no ledger"));
}

#[test]
fn prepare_storage_creates_parent_and_three_ledger_rows() {
    let root = TempRoot::new("prepare");
    let path = root.path().join("nested").join("tasks.db");
    let mut phases: Vec<TasksStoragePhase> = Vec::new();
    let mut facts_seen: Vec<Option<MigrationKind>> = Vec::new();
    let facts = prepare_tasks_storage(
        &path,
        PrepareOptions {
            lock_wait_ms: 5_000,
        },
        &mut |phase, facts| {
            phases.push(phase);
            facts_seen.push(facts.map(|facts| facts.kind));
        },
    )
    .expect("prepare storage");

    assert_eq!(facts.kind, MigrationKind::Initialize);
    assert_eq!(facts.executed_count, 3);
    assert_eq!(facts.committed_count, 3);
    assert!(phases.contains(&TasksStoragePhase::Checking));
    assert!(phases.contains(&TasksStoragePhase::Migrating));
    assert!(phases.contains(&TasksStoragePhase::Committing));
    assert!(phases.contains(&TasksStoragePhase::Maintaining));
    assert_eq!(phases.last(), Some(&TasksStoragePhase::Ready));
    assert!(facts_seen.contains(&Some(MigrationKind::Initialize)));

    let conn = Connection::open(&path).expect("open prepared db");
    assert_eq!(ledger(&conn).len(), 3);
    assert!(are_migrations_applied(&conn).expect("applied"));
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("journal mode");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    drop(conn);

    // 再次 prepare：账本已完整，kind=none 且不重复执行。
    let mut second_phases = Vec::new();
    let facts = prepare_tasks_storage(
        &path,
        PrepareOptions {
            lock_wait_ms: 5_000,
        },
        &mut |phase, _facts| second_phases.push(phase),
    )
    .expect("second prepare");
    assert_eq!(facts.kind, MigrationKind::None);
    assert_eq!(facts.executed_count, 0);
    assert_eq!(facts.committed_count, 0);
    assert_eq!(facts.last_applied_migration_id, Some(Some(MIGRATION_IDS[2].to_owned())));
    assert_eq!(second_phases.last(), Some(&TasksStoragePhase::Ready));
}

#[test]
fn prepare_inspect_failure_has_no_migration_facts() {
    let root = TempRoot::new("prepare-inspect-fail");
    let path = root.path().join("tasks.db");
    // 先制造可信账本，再篡改 checksum。
    prepare_tasks_storage(
        &path,
        PrepareOptions {
            lock_wait_ms: 5_000,
        },
        &mut |_phase, _facts| {},
    )
    .expect("first prepare");
    {
        let conn = Connection::open(&path).expect("open");
        conn.execute(
            "UPDATE tasks_schema_migration SET checksum='tampered' WHERE id='0002_provider_selection'",
            [],
        )
        .expect("tamper");
    }

    let mut facts_seen: Option<MigrationFacts> = None;
    let error = prepare_tasks_storage(
        &path,
        PrepareOptions {
            lock_wait_ms: 5_000,
        },
        &mut |_phase, facts| {
            if let Some(facts) = facts {
                facts_seen = Some(facts.clone());
            }
        },
    )
    .expect_err("tampered ledger must fail");
    assert_eq!(error.kind, Some(TasksDbErrorKind::ChecksumMismatch));
    assert_eq!(error.migration_id.as_deref(), Some("0002_provider_selection"));
    // inspect 在事务前失败：此时还没有迁移 facts 可附。
    assert!(error.startup_migration.is_none());
    assert!(facts_seen.is_none());
}

#[test]
fn prepare_migration_failure_carries_facts_and_migration_id() {
    let root = TempRoot::new("prepare-migrate-fail");
    let path = root.path().join("tasks.db");
    {
        // 同名 view 会让 0001 的第一条 CREATE TABLE 失败，但 inspect 仍返回 initialize。
        let conn = Connection::open(&path).expect("open");
        conn.execute_batch("CREATE VIEW tasks AS SELECT 1 AS x")
            .expect("create conflicting view");
    }

    let mut facts_seen: Option<MigrationFacts> = None;
    let error = prepare_tasks_storage(
        &path,
        PrepareOptions {
            lock_wait_ms: 5_000,
        },
        &mut |_phase, facts| {
            if let Some(facts) = facts {
                facts_seen = Some(facts.clone());
            }
        },
    )
    .expect_err("conflicting view must fail migration");
    assert_eq!(error.migration_id.as_deref(), Some("0001_adopt_task_schema"));
    let facts = error
        .startup_migration
        .as_deref()
        .expect("失败必须附带迁移 facts");
    assert_eq!(facts.kind, MigrationKind::Initialize);
    assert_eq!(facts.executed_count, 0);
    assert!(facts_seen.is_some(), "checking 阶段应已发布 facts");
}
