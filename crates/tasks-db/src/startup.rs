//! 打开任务库并完成迁移：`prepare_tasks_storage`。
//!
//! 行为真源：`packages/services/src/session/tasksDatabase/startup.ts`。
//!
//! 与 TS 的边界：TS 在迁移后还会让 `TaskIndexRepo` / `AutomationRepo` /
//! `OffPeakTaskRepo` 执行 `ensureReady()`（历史修复），然后才上报 `ready`。这些 Repo 由
//! 后续任务域接管；本函数上报 `maintaining` 与迁移层的 `ready`。

use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::Connection;

use crate::error::TasksDbError;
use crate::migrations::{
    inspect_tasks_migration_kind, run_migrations, MigrationFacts, MigrationProgress, RunOptions,
};

const DEFAULT_LOCK_WAIT_MS: u64 = 60 * 60_000;
const LOCK_POLL_MS: u64 = 100;
const BUSY_TIMEOUT_MS: u64 = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksStoragePhase {
    Checking,
    WaitingForLock,
    Migrating,
    Committing,
    Maintaining,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrepareOptions {
    /// busy 重试上限，默认 60 分钟（与 TS `LOCK_WAIT_MS` 一致）。
    pub lock_wait_ms: u64,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        Self {
            lock_wait_ms: DEFAULT_LOCK_WAIT_MS,
        }
    }
}

fn acquire<T>(
    deadline: Instant,
    waiting_reported: &mut bool,
    migration: &Option<MigrationFacts>,
    on_progress: &mut dyn FnMut(TasksStoragePhase, Option<&MigrationFacts>),
    mut operation: impl FnMut() -> Result<T, TasksDbError>,
) -> Result<T, TasksDbError> {
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                // 只重试 SQLITE_BUSY；其它真实错误直接失败。
                if !error.is_busy() {
                    return Err(error);
                }
                if Instant::now() >= deadline {
                    return Err(TasksDbError::lock_timeout(error));
                }
                if !*waiting_reported {
                    *waiting_reported = true;
                    on_progress(TasksStoragePhase::WaitingForLock, migration.as_ref());
                }
                thread::sleep(Duration::from_millis(LOCK_POLL_MS));
            }
        }
    }
}

fn map_phase(progress: MigrationProgress) -> TasksStoragePhase {
    match progress {
        MigrationProgress::Migrating => TasksStoragePhase::Migrating,
        MigrationProgress::Committing => TasksStoragePhase::Committing,
    }
}

/// 建父目录、打开连接、等待写锁并跑迁移。
///
/// 失败时错误携带当时的迁移 facts（`startup_migration`），不倒退上报阶段。
pub fn prepare_tasks_storage(
    path: &Path,
    options: PrepareOptions,
    on_progress: &mut dyn FnMut(TasksStoragePhase, Option<&MigrationFacts>),
) -> Result<MigrationFacts, TasksDbError> {
    on_progress(TasksStoragePhase::Checking, None);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
    conn.execute_batch("PRAGMA foreign_keys = ON")?;

    let deadline = Instant::now() + Duration::from_millis(options.lock_wait_ms);
    let mut waiting_reported = false;
    let mut migration: Option<MigrationFacts> = None;

    let outcome = (|| -> Result<MigrationFacts, TasksDbError> {
        acquire(
            deadline,
            &mut waiting_reported,
            &migration,
            on_progress,
            || {
                conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
                    row.get::<_, String>(0)
                })
                .map(|_| ())
                .map_err(TasksDbError::from)
            },
        )?;
        conn.execute_batch("PRAGMA synchronous = NORMAL")?;

        let kind = acquire(
            deadline,
            &mut waiting_reported,
            &migration,
            on_progress,
            || inspect_tasks_migration_kind(&conn),
        )?;
        migration = Some(MigrationFacts {
            kind,
            executed_count: 0,
            committed_count: 0,
            last_applied_migration_id: None,
        });
        on_progress(TasksStoragePhase::Checking, migration.as_ref());

        acquire(
            deadline,
            &mut waiting_reported,
            &migration,
            on_progress,
            || {
                conn.execute_batch("BEGIN IMMEDIATE")
                    .map_err(TasksDbError::from)
            },
        )?;

        let mut forward = |progress: MigrationProgress, facts: &MigrationFacts| {
            on_progress(map_phase(progress), Some(facts));
        };
        let facts = run_migrations(
            &conn,
            RunOptions {
                transaction_open: true,
                migration: migration.clone(),
                on_progress: Some(&mut forward),
            },
        )?;
        migration = Some(facts.clone());
        // COMMIT 已成功，先发布事实；后续 close 失败不能把已提交误报为未提交。
        on_progress(TasksStoragePhase::Maintaining, Some(&facts));
        Ok(facts)
    })();

    let close_error = match conn.close() {
        Ok(()) => None,
        Err((_conn, error)) => Some(TasksDbError::from(error)),
    };

    match outcome {
        Ok(facts) => {
            if let Some(error) = close_error {
                return Err(error);
            }
            on_progress(TasksStoragePhase::Ready, Some(&facts));
            Ok(facts)
        }
        Err(mut error) => {
            if let Some(facts) = &migration {
                error.startup_migration = Some(Box::new(facts.clone()));
            }
            Err(error)
        }
    }
}
