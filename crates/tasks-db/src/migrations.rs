//! 迁移账本与 runner：`run_migrations` / `adopt_schema` / inspect / areApplied。
//!
//! 行为真源：`packages/services/src/session/tasksDatabase/migrations.ts`。

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::TasksDbError;
use crate::schema::{
    active_predicate, bound_index_sql, migration_definitions, ADOPT_INDEXES, AUTOMATION_SCHEMA,
    FROZEN_COLUMNS, OFF_PEAK_SCHEMA, OFFICIAL_GLM_SELECTION_MIGRATION_SQL, TASK_INDEX_SCHEMA,
};
use crate::legacy_selection::import_legacy_automation_selections;

const LEDGER_TABLE: &str = "tasks_schema_migration";

const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS tasks_schema_migration (
      id TEXT PRIMARY KEY, checksum TEXT NOT NULL, time_applied INTEGER NOT NULL
    )";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MigrationKind {
    None,
    Initialize,
    Upgrade,
}

/// 与 shared `databaseMigrationFactsSchema` 对齐的事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationFacts {
    pub kind: MigrationKind,
    pub executed_count: u64,
    pub committed_count: u64,
    /// 外层 `None` = 尚未取得可信起点；`Some(None)` = 锁内账本为空。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_applied_migration_id: Option<Option<String>>,
}

impl Default for MigrationFacts {
    fn default() -> Self {
        Self {
            kind: MigrationKind::None,
            executed_count: 0,
            committed_count: 0,
            last_applied_migration_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationProgress {
    Migrating,
    Committing,
}

/// 迁移进度回调：`migrating` / `committing`，facts 为当时的快照。
pub type MigrationProgressCallback<'a> = &'a mut dyn FnMut(MigrationProgress, &MigrationFacts);

#[derive(Default)]
pub struct RunOptions<'a> {
    pub transaction_open: bool,
    /// 调用方已取得的迁移事实；提供时 runner 不再自行 inspect。
    pub migration: Option<MigrationFacts>,
    pub on_progress: Option<MigrationProgressCallback<'a>>,
}

fn emit(
    callback: &mut Option<MigrationProgressCallback<'_>>,
    progress: MigrationProgress,
    facts: &MigrationFacts,
) {
    if let Some(callback) = callback.as_mut() {
        (*callback)(progress, facts);
    }
}

fn is_valid_migration_id(value: &str) -> bool {
    let mut length = 0;
    for byte in value.bytes() {
        let valid = byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-';
        if !valid {
            return false;
        }
        length += 1;
    }
    (1..=128).contains(&length)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, TasksDbError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
            params![name],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false))
}

fn checksum_for_id(conn: &Connection, id: &str) -> Result<Option<String>, TasksDbError> {
    Ok(conn
        .query_row(
            "SELECT checksum FROM tasks_schema_migration WHERE id=?",
            params![id],
            |row| row.get(0),
        )
        .optional()?)
}

/// 只读账本的展示预检，不授权执行；runner 拿锁后仍复查每一项。
pub fn inspect_tasks_migration_kind(conn: &Connection) -> Result<MigrationKind, TasksDbError> {
    let has_ledger = table_exists(conn, LEDGER_TABLE)?;
    let mut pending = false;
    for definition in migration_definitions() {
        let row = if has_ledger {
            checksum_for_id(conn, definition.id)?
        } else {
            None
        };
        match row {
            None => pending = true,
            Some(checksum) => {
                if checksum != definition.checksum {
                    return Err(TasksDbError::checksum_mismatch(definition.id));
                }
            }
        }
    }
    if !pending {
        return Ok(MigrationKind::None);
    }
    let has_other_table = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT IN ('tasks_schema_migration', 'sqlite_sequence') LIMIT 1",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    Ok(if has_other_table {
        MigrationKind::Upgrade
    } else {
        MigrationKind::Initialize
    })
}

/// 交接只复用已完成初始化；每次仍按冻结账本确认，替换/清空文件不能假 ready。
pub fn are_migrations_applied(conn: &Connection) -> Result<bool, TasksDbError> {
    if !table_exists(conn, LEDGER_TABLE)? {
        return Ok(false);
    }
    for definition in migration_definitions() {
        let Some(checksum) = checksum_for_id(conn, definition.id)? else {
            return Ok(false);
        };
        if checksum != definition.checksum {
            return Err(TasksDbError::checksum_mismatch(definition.id));
        }
    }
    Ok(true)
}

/// 0001：接管已有分散建表，补齐冻结列并回填 scheduled_run_count。
pub fn adopt_schema(conn: &Connection) -> Result<(), TasksDbError> {
    conn.execute_batch(&format!(
        "{TASK_INDEX_SCHEMA}{AUTOMATION_SCHEMA}{OFF_PEAK_SCHEMA}"
    ))?;
    for &(table, column, definition) in FROZEN_COLUMNS {
        if table_has_column(conn, table, column)? {
            continue;
        }
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
        if table == "automations" && column == "scheduled_run_count" {
            conn.execute_batch("UPDATE automations SET scheduled_run_count=run_count")?;
        }
    }
    conn.execute_batch(ADOPT_INDEXES)?;
    // 沿用已裁决的旧重复绑定保留策略，但不吞掉权限/语法/磁盘等真实 SQL 错误。
    let duplicate = conn
        .query_row(
            &format!(
                "SELECT 1 FROM off_peak_tasks WHERE {}
    GROUP BY workspace_key, session_id HAVING count(*)>1 LIMIT 1",
                active_predicate()
            ),
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !duplicate {
        conn.execute_batch(&bound_index_sql())?;
    }
    Ok(())
}

fn table_has_column(
    conn: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, TasksDbError> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 执行冻结迁移。失败时回滚当前事务并给错误附上 `migrationId`。
pub fn run_migrations(
    conn: &Connection,
    options: RunOptions<'_>,
) -> Result<MigrationFacts, TasksDbError> {
    let RunOptions {
        transaction_open,
        migration,
        mut on_progress,
    } = options;
    if !transaction_open {
        conn.execute_batch("BEGIN IMMEDIATE")?;
    }
    let mut current_migration_id: Option<&'static str> = None;
    let result = (|| -> Result<MigrationFacts, TasksDbError> {
        let mut facts = match migration {
            Some(facts) => facts,
            None => MigrationFacts {
                kind: inspect_tasks_migration_kind(conn)?,
                ..MigrationFacts::default()
            },
        };
        conn.execute_batch(LEDGER_DDL)?;
        // 锁内、版本 SQL 之前采集；空账本是 null，异常编号不作为遥测原文发送。
        let baseline = conn
            .query_row(
                "SELECT id FROM tasks_schema_migration ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        facts.last_applied_migration_id =
            Some(baseline.filter(|value| is_valid_migration_id(value)));

        for definition in migration_definitions() {
            current_migration_id = Some(definition.id);
            let applied = checksum_for_id(conn, definition.id)?;
            if let Some(checksum) = applied {
                if checksum != definition.checksum {
                    return Err(TasksDbError::checksum_mismatch(definition.id));
                }
                continue;
            }
            if facts.kind == MigrationKind::None {
                facts.kind = MigrationKind::Upgrade;
            }
            emit(&mut on_progress, MigrationProgress::Migrating, &facts);
            match definition.id {
                "0001_adopt_task_schema" => adopt_schema(conn)?,
                "0002_provider_selection" => import_legacy_automation_selections(conn)?,
                _ => conn.execute_batch(OFFICIAL_GLM_SELECTION_MIGRATION_SQL)?,
            }
            facts.executed_count += 1;
            conn.execute(
                "INSERT INTO tasks_schema_migration VALUES(?,?,?)",
                params![definition.id, definition.checksum, now_millis()],
            )?;
        }
        emit(&mut on_progress, MigrationProgress::Committing, &facts);
        conn.execute_batch("COMMIT")?;
        facts.committed_count = facts.executed_count;
        Ok(facts)
    })();

    match result {
        Ok(facts) => Ok(facts),
        Err(error) => {
            // 回滚也可能因 IO 失败，不能覆盖真正导致迁移失败的异常。
            if !conn.is_autocommit() {
                let _ = conn.execute_batch("ROLLBACK");
            }
            Err(match current_migration_id {
                Some(id) => error.with_migration_id(id),
                None => error,
            })
        }
    }
}
