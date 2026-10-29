//! 任务索引仓库：核心任务行、状态写入、列表与搜索。
//!
//! 行为真源：`packages/services/src/session/taskIndexRepo.ts`（本轮纳入的方法）。
//! 规范见 `docs/specs/task-index-repo.md`。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::Value;

use crate::error::{Result, TaskIndexRepoError};
use crate::js::{
    build_search_snippets, js_trim, resolve_workspace_key, truncate_utf16,
};
use crate::meta::{
    is_terminal_status, parse_mode_task_meta, ModeTaskGoal, ModeTaskLastError, ModeTaskMeta,
    CRON_DEFAULT_GROUP_ID, MODE_AGENT_PROVIDER, OFF_PEAK_DEFAULT_GROUP_ID,
};
use crate::remote::is_remote_workspace_identity;

const TASK_SEARCH_TEXT_MAX_CHARS: usize = 200_000;
pub(crate) const GROUPED_TASK_ORDER_STEP: i64 = 1000;
const PROVIDER_COLUMN: &str = "provider";

pub(crate) const ROW_COLUMNS: &str = "workspace_key, workspace_path, workspace_identity, task_id, title, \
task_status, provider, mode, model, migration_source, forked_from_task_id, cron_automation_id, \
off_peak_task_id, created_at, updated_at, unread_at, last_unread_at, pinned, archived, deleted, \
title_overridden, searchable_text, meta_json";

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn migrated_registry() -> &'static Mutex<HashSet<PathBuf>> {
    static REGISTRY: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

fn prepared_registry() -> &'static Mutex<HashSet<PathBuf>> {
    static REGISTRY: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 等价于 TS `markTasksStorageMigrated(path)`：当前进程已确认该路径完成迁移。
pub fn mark_tasks_storage_migrated(path: impl AsRef<Path>) {
    migrated_registry()
        .lock()
        .expect("migrated registry poisoned")
        .insert(path.as_ref().to_path_buf());
}

/// 等价于 TS `markTasksStoragePrepared(path)`：启动 Worker 已完成该路径的原始准备。
pub fn mark_tasks_storage_prepared(path: impl AsRef<Path>) {
    let path = path.as_ref().to_path_buf();
    migrated_registry()
        .lock()
        .expect("migrated registry poisoned")
        .insert(path.clone());
    prepared_registry()
        .lock()
        .expect("prepared registry poisoned")
        .insert(path);
}

fn registry_contains(registry: &'static Mutex<HashSet<PathBuf>>, path: &Path) -> bool {
    registry
        .lock()
        .expect("registry poisoned")
        .contains(path)
}

#[derive(Clone, Debug, Default)]
pub struct TaskRef {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub task_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct TaskListWorkspaceScope {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub workspace_purpose: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskListKind {
    Pinned,
    Archived,
    Timeline,
    Active,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskListSortBy {
    Created,
    Updated,
}

#[derive(Clone, Debug)]
pub struct TaskListQuery {
    pub kind: TaskListKind,
    pub workspace_scopes: Vec<TaskListWorkspaceScope>,
    pub sort_by: TaskListSortBy,
    pub search: Option<String>,
    pub provider: Option<String>,
    pub limit: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskListItem {
    #[serde(flatten)]
    pub meta: ModeTaskMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_snippets: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskListResult {
    pub items: Vec<TaskListItem>,
    pub total: i64,
    pub has_more: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ListTaskMetasParams {
    pub workspace_path: Option<String>,
    pub workspace_identity: Option<String>,
    pub provider: Option<String>,
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    pub include_deleted: bool,
}

#[derive(Clone, Debug)]
pub struct ListDeletedTaskIdsParams {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub provider: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ArchiveStaleTasksParams {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub older_than_days: f64,
    pub provider: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SyncTaskMetaParams {
    pub meta: ModeTaskMeta,
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    pub deleted: Option<bool>,
    pub title_overridden: Option<bool>,
    pub searchable_text: Option<String>,
}

/// `syncTaskMetaAtGroupedTop` 的回包：写入结果与是否首次初始化了顶层顺序。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncTaskMetaAtGroupedTop {
    pub meta: ModeTaskMeta,
    pub initialized_grouped_order: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TaskIndexStatePatch {
    pub pinned: Option<bool>,
    pub archived: Option<bool>,
    pub deleted: Option<bool>,
    pub title: Option<String>,
    pub title_overridden: Option<bool>,
    /// `None` = 字段缺失；`Some(None)` = TS 的 present-undefined（清空）；
    /// `Some(Some(v))` = 申请新的未读 marker。
    pub unread_at: Option<Option<i64>>,
    pub model: Option<String>,
    pub status: Option<String>,
    /// `None` = 保留；`Some(None)` = present-undefined（清空）；`Some(Some(_))` = 写入。
    pub last_error: Option<Option<ModeTaskLastError>>,
    /// `None` = 保留；`Some(None)` = JSON null；`Some(Some(_))` = 写入。
    pub target: Option<Option<ModeTaskGoal>>,
    pub updated_at: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct AgentPatch {
    pub title: Option<String>,
    pub status: Option<String>,
    pub last_error: Option<Option<ModeTaskLastError>>,
    pub target: Option<Option<ModeTaskGoal>>,
    pub updated_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClearUnreadResult {
    pub meta: ModeTaskMeta,
    pub cleared: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct TaskIndexRow {
    pub(crate) workspace_key: String,
    pub(crate) workspace_path: String,
    pub(crate) workspace_identity: Option<String>,
    pub(crate) task_id: String,
    pub(crate) title: String,
    pub(crate) task_status: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) mode: String,
    pub(crate) model: Option<String>,
    pub(crate) migration_source: Option<String>,
    pub(crate) forked_from_task_id: Option<String>,
    pub(crate) cron_automation_id: Option<String>,
    pub(crate) off_peak_task_id: Option<String>,
    pub(crate) created_at: i64,
    pub(crate) updated_at: i64,
    pub(crate) unread_at: Option<i64>,
    pub(crate) last_unread_at: i64,
    pub(crate) pinned: i64,
    pub(crate) archived: i64,
    pub(crate) deleted: i64,
    pub(crate) title_overridden: i64,
    pub(crate) searchable_text: String,
    pub(crate) meta_json: String,
}

impl TaskIndexRow {
    pub(crate) fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            workspace_key: row.get(0)?,
            workspace_path: row.get(1)?,
            workspace_identity: row.get(2)?,
            task_id: row.get(3)?,
            title: row.get(4)?,
            task_status: row.get(5)?,
            provider: row.get(6)?,
            mode: row.get(7)?,
            model: row.get(8)?,
            migration_source: row.get(9)?,
            forked_from_task_id: row.get(10)?,
            cron_automation_id: row.get(11)?,
            off_peak_task_id: row.get(12)?,
            created_at: row.get(13)?,
            updated_at: row.get(14)?,
            unread_at: row.get(15)?,
            last_unread_at: row.get(16)?,
            pinned: row.get(17)?,
            archived: row.get(18)?,
            deleted: row.get(19)?,
            title_overridden: row.get(20)?,
            searchable_text: row.get(21)?,
            meta_json: row.get(22)?,
        })
    }
}

struct WriteRecord {
    meta: ModeTaskMeta,
    pinned: bool,
    archived: bool,
    deleted: bool,
    title_overridden: bool,
    write_unread_at: bool,
    /// `None` = 保留 existing 行的 `searchable_text`。
    searchable_text: Option<String>,
}

struct State {
    db: Option<Connection>,
}

/// 系统分组归属的 task 坐标（workspace key 已按 identity ?? path 解析）。
struct MembershipTarget<'a> {
    workspace_key: &'a str,
    workspace_path: &'a str,
    workspace_identity: Option<&'a str>,
    task_id: &'a str,
}

pub struct TaskIndexRepo {
    db_path: PathBuf,
    startup_busy_timeout_ms: u64,
    state: Mutex<State>,
    write_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl TaskIndexRepo {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self::with_clock(db_path, now_millis)
    }

    pub fn with_busy_timeout(db_path: impl Into<PathBuf>, busy_timeout_ms: u64) -> Self {
        Self {
            db_path: db_path.into(),
            startup_busy_timeout_ms: busy_timeout_ms,
            state: Mutex::new(State { db: None }),
            write_locks: Mutex::new(HashMap::new()),
            clock: Box::new(now_millis),
        }
    }

    pub fn with_clock(
        db_path: impl Into<PathBuf>,
        clock: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            db_path: db_path.into(),
            startup_busy_timeout_ms: 5000,
            state: Mutex::new(State { db: None }),
            write_locks: Mutex::new(HashMap::new()),
            clock: Box::new(clock),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub(crate) fn now(&self) -> i64 {
        (self.clock)()
    }

    /// 打开库并完成幂等初始化。
    pub fn ensure_ready(&self) -> Result<()> {
        let mut state = self.state.lock().expect("task index state poisoned");
        if state.db.is_none() {
            if let Some(parent) = self.db_path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)?;
                }
            }
            let connection = Connection::open(&self.db_path)?;
            connection.busy_timeout(Duration::from_millis(self.startup_busy_timeout_ms))?;
            connection.execute_batch("PRAGMA foreign_keys = ON")?;
            connection.execute_batch("PRAGMA journal_mode = WAL")?;
            connection.execute_batch("PRAGMA synchronous = NORMAL")?;
            state.db = Some(connection);
        }
        let connection = state.db.as_ref().expect("connection was just opened");

        if registry_contains(prepared_registry(), &self.db_path)
            && tasks_db::are_migrations_applied(connection)?
        {
            return Ok(());
        }
        if !(registry_contains(migrated_registry(), &self.db_path)
            && tasks_db::are_migrations_applied(connection)?)
        {
            tasks_db::run_migrations(connection, tasks_db::RunOptions::default())?;
            mark_tasks_storage_migrated(&self.db_path);
        }
        self.backfill_off_peak_task_markers(connection)?;
        self.backfill_off_peak_group_memberships(connection)?;
        self.cleanup_deleted_task_grouping_references(connection)?;
        // 回填只在首次初始化执行一次（TS 用 `initializePromise` 等价语义）。
        // 不记住会重复回填：`backfillOffPeakGroupMemberships` 的 INSERT OR IGNORE
        // 会在用户把闲时任务移出系统分组后再次把它加回去、并重建已被清空的分组。
        mark_tasks_storage_prepared(&self.db_path);
        Ok(())
    }

    /// 释放连接与写锁表。
    pub fn close(&self) {
        let mut state = self.state.lock().expect("task index state poisoned");
        if let Some(connection) = state.db.take() {
            let _ = connection.close();
        }
        self.write_locks
            .lock()
            .expect("write locks poisoned")
            .clear();
    }

    pub(crate) fn with_conn<T>(&self, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let state = self.state.lock().expect("task index state poisoned");
        let connection = state
            .db
            .as_ref()
            .ok_or(TaskIndexRepoError::NotInitialized)?;
        operation(connection)
    }

    pub(crate) fn with_write_conn<T>(
        &self,
        key: &str,
        operation: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        let gate = {
            let mut locks = self.write_locks.lock().expect("write locks poisoned");
            locks
                .entry(key.to_owned())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let gate_guard = gate.lock().expect("write gate poisoned");
        let state = self.state.lock().expect("task index state poisoned");
        let connection = state
            .db
            .as_ref()
            .ok_or(TaskIndexRepoError::NotInitialized)?;
        let result = operation(connection);
        drop(state);
        drop(gate_guard);

        let mut locks = self.write_locks.lock().expect("write locks poisoned");
        if Arc::strong_count(&gate) <= 2 {
            locks.remove(key);
        }
        result
    }

    // ── 初始化回填 ────────────────────────────────────────────────────────

    fn backfill_off_peak_task_markers(&self, connection: &Connection) -> Result<()> {
        let has_table = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'off_peak_tasks'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !has_table {
            return Ok(());
        }
        connection.execute(
            "UPDATE tasks SET off_peak_task_id = (
               SELECT o.off_peak_task_id FROM off_peak_tasks o
               WHERE o.session_id = tasks.task_id AND o.workspace_key = tasks.workspace_key
             )
             WHERE off_peak_task_id IS NULL
               AND EXISTS (
                 SELECT 1 FROM off_peak_tasks o
                 WHERE o.session_id = tasks.task_id AND o.workspace_key = tasks.workspace_key
               )",
            [],
        )?;
        Ok(())
    }

    fn backfill_off_peak_group_memberships(&self, connection: &Connection) -> Result<()> {
        let rows: Vec<(String, String, Option<String>, String)> = connection
            .prepare(
                "SELECT workspace_key, workspace_path, workspace_identity, task_id FROM tasks
                 WHERE off_peak_task_id IS NOT NULL AND deleted = 0",
            )?
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (workspace_key, workspace_path, workspace_identity, task_id) in rows {
            // 闲时任务暂不支持远程 workspace；remote 判定看主键 workspace_key。
            if is_remote_workspace_identity(&workspace_key) {
                continue;
            }
            let meta_key = resolve_workspace_key(&workspace_path, workspace_identity.as_deref());
            self.ensure_off_peak_group_membership(
                connection,
                MembershipTarget {
                    workspace_key: &meta_key,
                    workspace_path: &workspace_path,
                    workspace_identity: workspace_identity.as_deref(),
                    task_id: &task_id,
                },
            )?;
        }
        Ok(())
    }

    fn delete_task_grouping_references(
        &self,
        connection: &Connection,
        workspace_key: &str,
        task_id: &str,
    ) -> Result<()> {
        connection.execute(
            "DELETE FROM task_group_members WHERE workspace_key = ? AND task_id = ?",
            params![workspace_key, task_id],
        )?;
        let node_key = serde_json::to_string(&[workspace_key, task_id])?;
        connection.execute(
            "DELETE FROM task_group_view_node_orders
             WHERE node_type = 'task' AND (node_key = ? OR node_key = ?)",
            params![node_key, workspace_key],
        )?;
        Ok(())
    }

    fn cleanup_deleted_task_grouping_references(&self, connection: &Connection) -> Result<()> {
        let rows: Vec<(String, String)> = connection
            .prepare("SELECT workspace_key, task_id FROM tasks WHERE deleted = 1")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() {
            return Ok(());
        }
        connection.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<()> {
            for (workspace_key, task_id) in &rows {
                self.delete_task_grouping_references(connection, workspace_key, task_id)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                connection.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(error) => {
                let _ = connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn ensure_off_peak_group_membership(
        &self,
        connection: &Connection,
        target: MembershipTarget<'_>,
    ) -> Result<()> {
        if target
            .workspace_identity
            .is_some_and(is_remote_workspace_identity)
        {
            return Ok(());
        }
        self.ensure_system_group_membership(
            connection,
            target,
            OFF_PEAK_DEFAULT_GROUP_ID,
            "off-peak",
            "purple",
        )
    }

    fn ensure_cron_group_membership(
        &self,
        connection: &Connection,
        meta: &ModeTaskMeta,
    ) -> Result<()> {
        let workspace_key =
            resolve_workspace_key(&meta.workspace_path, meta.workspace_identity.as_deref());
        self.ensure_system_group_membership(
            connection,
            MembershipTarget {
                workspace_key: &workspace_key,
                workspace_path: &meta.workspace_path,
                workspace_identity: meta.workspace_identity.as_deref(),
                task_id: &meta.task_id,
            },
            CRON_DEFAULT_GROUP_ID,
            "cron",
            "blue",
        )
    }

    fn ensure_system_group_membership(
        &self,
        connection: &Connection,
        target: MembershipTarget<'_>,
        group_id: &str,
        title: &str,
        color: &str,
    ) -> Result<()> {
        let now = self.now();
        connection.execute(
            "INSERT OR IGNORE INTO task_groups (group_id, title, color, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
            params![group_id, title, color, now, now],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO task_group_view_node_orders
               (node_type, node_key, sort_order, created_at, updated_at)
             VALUES ('group', ?, ?, ?, ?)",
            params![group_id, self.next_grouped_top_sort_order(connection)?, now, now],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO task_group_members (
               group_id, workspace_key, workspace_path, workspace_identity, task_id,
               sort_order, added_at, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?)",
            params![
                group_id,
                target.workspace_key,
                target.workspace_path,
                target.workspace_identity,
                target.task_id,
                now,
                now,
                now
            ],
        )?;
        Ok(())
    }

    pub(crate) fn next_grouped_top_sort_order(&self, connection: &Connection) -> Result<i64> {
        let minimum: Option<i64> = connection
            .query_row(
                "SELECT MIN(sort_order) FROM task_group_view_node_orders",
                [],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(minimum.unwrap_or(GROUPED_TASK_ORDER_STEP * 2) - GROUPED_TASK_ORDER_STEP)
    }

    // ── 行读写 ────────────────────────────────────────────────────────────

    pub(crate) fn get_task_row(
        &self,
        connection: &Connection,
        workspace_key: &str,
        task_id: &str,
    ) -> Result<Option<TaskIndexRow>> {
        Ok(connection
            .query_row(
                &format!("SELECT {ROW_COLUMNS} FROM tasks WHERE workspace_key = ? AND task_id = ?"),
                params![workspace_key, task_id],
                TaskIndexRow::read,
            )
            .optional()?)
    }

    fn write_record(&self, connection: &Connection, record: WriteRecord) -> Result<ModeTaskMeta> {
        let workspace_key =
            resolve_workspace_key(&record.meta.workspace_path, record.meta.workspace_identity.as_deref());
        let existing = if record.searchable_text.is_none() {
            self.get_task_row(connection, &workspace_key, &record.meta.task_id)?
        } else {
            None
        };
        let searchable_text = match &record.searchable_text {
            Some(text) => truncate_utf16(text, TASK_SEARCH_TEXT_MAX_CHARS),
            None => existing
                .as_ref()
                .map(|row| row.searchable_text.clone())
                .unwrap_or_default(),
        };
        let meta_json = serde_json::to_string(&record.meta)?;
        connection.execute(
            "INSERT INTO tasks (
               workspace_key, workspace_path, workspace_identity, task_id, title, task_status,
               provider, mode, model, migration_source, forked_from_task_id, cron_automation_id,
               off_peak_task_id, created_at, updated_at, unread_at, last_unread_at, pinned,
               archived, deleted, title_overridden, searchable_text, meta_json
             ) VALUES (
               @workspace_key, @workspace_path, @workspace_identity, @task_id, @title,
               @task_status, @provider, @mode, @model, @migration_source, @forked_from_task_id,
               @cron_automation_id, @off_peak_task_id, @created_at, @updated_at, @unread_at,
               @last_unread_at, @pinned, @archived, @deleted, @title_overridden,
               @searchable_text, @meta_json
             )
             ON CONFLICT(workspace_key, task_id) DO UPDATE SET
               workspace_path = excluded.workspace_path,
               workspace_identity = excluded.workspace_identity,
               title = excluded.title,
               task_status = excluded.task_status,
               provider = excluded.provider,
               mode = excluded.mode,
               model = excluded.model,
               migration_source = excluded.migration_source,
               forked_from_task_id = excluded.forked_from_task_id,
               cron_automation_id = excluded.cron_automation_id,
               off_peak_task_id = excluded.off_peak_task_id,
               created_at = excluded.created_at,
               updated_at = excluded.updated_at,
               unread_at = CASE
                 WHEN @write_unread_at = 1 THEN excluded.unread_at
                 ELSE tasks.unread_at
               END,
               last_unread_at = MAX(
                 tasks.last_unread_at,
                 COALESCE(tasks.unread_at, 0),
                 CASE WHEN @write_unread_at = 1 THEN excluded.last_unread_at ELSE 0 END
               ),
               pinned = excluded.pinned,
               archived = excluded.archived,
               deleted = excluded.deleted,
               title_overridden = excluded.title_overridden,
               searchable_text = excluded.searchable_text,
               meta_json = excluded.meta_json",
            rusqlite::named_params! {
                "@workspace_key": workspace_key,
                "@workspace_path": record.meta.workspace_path,
                "@workspace_identity": record.meta.workspace_identity,
                "@task_id": record.meta.task_id,
                "@title": record.meta.title,
                "@task_status": record.meta.status,
                "@provider": record.meta.provider,
                "@mode": record.meta.mode,
                "@model": record.meta.model,
                "@migration_source": record.meta.migration_source,
                "@forked_from_task_id": record.meta.forked_from_task_id,
                "@cron_automation_id": record.meta.cron_automation_id,
                "@off_peak_task_id": record.meta.off_peak_task_id,
                "@created_at": record.meta.created_at,
                "@updated_at": record.meta.updated_at,
                "@unread_at": record.meta.unread_at,
                "@last_unread_at": record.meta.unread_at.unwrap_or(0),
                "@write_unread_at": bool_int(record.write_unread_at),
                "@pinned": bool_int(record.pinned),
                "@archived": bool_int(record.archived),
                "@deleted": bool_int(record.deleted),
                "@title_overridden": bool_int(record.title_overridden),
                "@searchable_text": searchable_text,
                "@meta_json": meta_json,
            },
        )?;
        let persisted = self
            .get_task_row(connection, &workspace_key, &record.meta.task_id)?
            .ok_or_else(|| {
                TaskIndexRepoError::TaskNotFound(format!(
                    "task index 写入后缺少 task: {}",
                    record.meta.task_id
                ))
            })?;
        Ok(row_to_meta(&persisted))
    }

    // ── 读接口 ────────────────────────────────────────────────────────────

    pub fn get_task_meta(&self, params: &TaskRef) -> Result<Option<ModeTaskMeta>> {
        self.ensure_ready()?;
        let workspace_key =
            resolve_workspace_key(&params.workspace_path, params.workspace_identity.as_deref());
        self.with_conn(|connection| {
            let row = self.get_task_row(connection, &workspace_key, &params.task_id)?;
            Ok(row.filter(|row| row.deleted != 1).map(|row| row_to_meta(&row)))
        })
    }

    pub fn list_task_metas(&self, params: &ListTaskMetasParams) -> Result<Vec<ModeTaskMeta>> {
        self.ensure_ready()?;
        let target_key = params.workspace_path.as_deref().map(|path| {
            resolve_workspace_key(path, params.workspace_identity.as_deref())
        });
        self.with_conn(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {ROW_COLUMNS} FROM tasks
                 WHERE (?1 IS NULL OR workspace_key = ?1)
                   AND (?2 = 1 OR deleted = 0)
                   AND (?3 IS NULL OR provider = ?3)
                   AND (?4 IS NULL OR pinned = ?4)
                   AND (?5 IS NULL OR archived = ?5)
                 ORDER BY updated_at DESC, created_at DESC, task_id DESC"
            ))?;
            let rows = statement
                .query_map(
                    params![
                        target_key,
                        bool_int(params.include_deleted),
                        params.provider,
                        params.pinned.map(bool_int),
                        params.archived.map(bool_int),
                    ],
                    TaskIndexRow::read,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.iter().map(row_to_meta).collect())
        })
    }

    pub fn list_deleted_task_ids(&self, params: &ListDeletedTaskIdsParams) -> Result<Vec<String>> {
        self.ensure_ready()?;
        let workspace_key =
            resolve_workspace_key(&params.workspace_path, params.workspace_identity.as_deref());
        self.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT task_id FROM tasks
                 WHERE workspace_key = ?1 AND deleted = 1 AND (?2 IS NULL OR provider = ?2)
                 ORDER BY task_id",
            )?;
            let rows = statement
                .query_map(params![workspace_key, params.provider], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    pub fn list_sessions_by_automation(&self, automation_id: &str) -> Result<Vec<ModeTaskMeta>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {ROW_COLUMNS} FROM tasks
                 WHERE cron_automation_id = ?1 AND deleted = 0
                 ORDER BY created_at DESC, task_id DESC"
            ))?;
            let rows = statement
                .query_map(params![automation_id], TaskIndexRow::read)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.iter().map(row_to_meta).collect())
        })
    }

    pub fn query_task_list(&self, params: &TaskListQuery) -> Result<TaskListResult> {
        self.ensure_ready()?;
        let workspace_keys = normalize_workspace_keys(&params.workspace_scopes);
        if workspace_keys.is_empty() {
            return Ok(TaskListResult {
                items: Vec::new(),
                total: 0,
                has_more: false,
            });
        }

        let search = params.search.as_deref().map(js_trim).map(str::to_owned);
        let search_like = search
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{}%", value.to_lowercase()));

        let mut where_clause = vec![
            "deleted = 0".to_owned(),
            format!(
                "workspace_key IN ({})",
                vec!["?"; workspace_keys.len()].join(", ")
            ),
        ];
        let mut arguments: Vec<SqlValue> = workspace_keys
            .iter()
            .map(|key| SqlValue::Text(key.clone()))
            .collect();
        if let Some(provider) = &params.provider {
            where_clause.push(format!("{PROVIDER_COLUMN} = ?"));
            arguments.push(SqlValue::Text(provider.clone()));
        }
        match params.kind {
            TaskListKind::Pinned => {
                where_clause.push("pinned = 1".to_owned());
                where_clause.push("archived = 0".to_owned());
            }
            TaskListKind::Archived => {
                where_clause.push("archived = 1".to_owned());
            }
            TaskListKind::Timeline | TaskListKind::Active => {
                where_clause.push("pinned = 0".to_owned());
                where_clause.push("archived = 0".to_owned());
            }
        }
        if let Some(search_like) = &search_like {
            where_clause.push("(LOWER(title) LIKE ? OR LOWER(searchable_text) LIKE ?)".to_owned());
            arguments.push(SqlValue::Text(search_like.clone()));
            arguments.push(SqlValue::Text(search_like.clone()));
        }
        let where_sql = where_clause.join(" AND ");

        self.with_conn(|connection| {
            let total: i64 = connection.query_row(
                &format!("SELECT COUNT(1) AS total FROM tasks WHERE {where_sql}"),
                params_from_iter(arguments.iter()),
                |row| row.get(0),
            )?;

            let limit = params.limit.and_then(|limit| {
                if limit.is_finite() && limit > 0.0 {
                    Some(limit.floor())
                } else {
                    None
                }
            });
            let order_by = if params.sort_by == TaskListSortBy::Created {
                "created_at DESC, updated_at DESC, task_id DESC"
            } else {
                "updated_at DESC, created_at DESC, task_id DESC"
            };
            let mut list_arguments = arguments.clone();
            let limit_sql = if let Some(limit) = limit {
                list_arguments.push(SqlValue::Integer(limit as i64));
                " LIMIT ?"
            } else {
                ""
            };
            let mut statement = connection.prepare(&format!(
                "SELECT {ROW_COLUMNS} FROM tasks
                 WHERE {where_sql}
                 ORDER BY {order_by}{limit_sql}"
            ))?;
            let rows = statement
                .query_map(params_from_iter(list_arguments.iter()), TaskIndexRow::read)?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let purpose_by_key: HashMap<String, String> = params
                .workspace_scopes
                .iter()
                .filter_map(|scope| {
                    scope.workspace_purpose.as_ref().map(|purpose| {
                        (
                            resolve_workspace_key(
                                &scope.workspace_path,
                                scope.workspace_identity.as_deref(),
                            ),
                            purpose.clone(),
                        )
                    })
                })
                .collect();

            let items = rows
                .iter()
                .map(|row| {
                    let mut item = row_to_task_list_item(row, search.as_deref());
                    if let Some(purpose) = purpose_by_key.get(&row.workspace_key) {
                        item.meta.workspace_purpose = Some(purpose.clone());
                    }
                    item
                })
                .collect::<Vec<_>>();

            Ok(TaskListResult {
                has_more: total > rows.len() as i64,
                items,
                total,
            })
        })
    }

    // ── 写入接口 ──────────────────────────────────────────────────────────

    pub fn sync_task_meta(&self, params: SyncTaskMetaParams) -> Result<ModeTaskMeta> {
        self.ensure_ready()?;
        let workspace_key = resolve_workspace_key(
            &params.meta.workspace_path,
            params.meta.workspace_identity.as_deref(),
        );
        let key = format!("{workspace_key}\u{0000}{}", params.meta.task_id);
        self.with_write_conn(&key, |connection| {
            self.sync_task_meta_body(connection, &params)
        })
    }

    /// 首次公开 root task 时，原子提交 task row 与 grouped 顶层顺序。
    pub fn sync_task_meta_at_grouped_top(
        &self,
        params: SyncTaskMetaParams,
    ) -> Result<SyncTaskMetaAtGroupedTop> {
        self.ensure_ready()?;
        let workspace_key = resolve_workspace_key(
            &params.meta.workspace_path,
            params.meta.workspace_identity.as_deref(),
        );
        let key = format!("{workspace_key}\u{0000}{}", params.meta.task_id);
        self.with_write_conn(&key, |connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let outcome = (|| -> Result<(ModeTaskMeta, bool)> {
                let meta = self.sync_task_meta_body(connection, &params)?;
                let initialized_grouped_order = self.initialize_grouped_task_at_top_ready(
                    connection,
                    &crate::grouped::GroupedTaskRef {
                        workspace_path: params.meta.workspace_path.clone(),
                        workspace_identity: params.meta.workspace_identity.clone(),
                        task_id: params.meta.task_id.clone(),
                    },
                )?;
                Ok((meta, initialized_grouped_order))
            })();
            match outcome {
                Ok((meta, initialized_grouped_order)) => {
                    connection.execute_batch("COMMIT")?;
                    Ok(SyncTaskMetaAtGroupedTop {
                        meta,
                        initialized_grouped_order,
                    })
                }
                Err(error) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    fn sync_task_meta_body(
        &self,
        connection: &Connection,
        params: &SyncTaskMetaParams,
    ) -> Result<ModeTaskMeta> {
        let workspace_key = resolve_workspace_key(
            &params.meta.workspace_path,
            params.meta.workspace_identity.as_deref(),
        );
        {
            let existing = self.get_task_row(connection, &workspace_key, &params.meta.task_id)?;
            let existing_meta = existing.as_ref().map(row_to_meta);
            let title_overridden = params
                .title_overridden
                .unwrap_or_else(|| existing.as_ref().is_some_and(|row| row.title_overridden == 1));
            let updated_at = params
                .meta
                .updated_at
                .max(existing_meta.as_ref().map(|meta| meta.updated_at).unwrap_or(0));
            let preserve_terminal = should_preserve_newer_terminal_status(
                existing_meta.as_ref(),
                &params.meta,
            );

            let mut meta = params.meta.clone();
            meta.title = if title_overridden && existing_meta.is_some() {
                existing_meta
                    .as_ref()
                    .map(|existing| existing.title.clone())
                    .unwrap_or_else(|| params.meta.title.clone())
            } else {
                params.meta.title.clone()
            };
            meta.title_overridden = Some(title_overridden);
            if preserve_terminal {
                if let Some(existing) = &existing_meta {
                    meta.status = existing.status.clone();
                    meta.last_error = existing.last_error.clone();
                }
            }
            meta.target = if params.meta.target.is_some() {
                params.meta.target.clone()
            } else {
                existing_meta.as_ref().and_then(|meta| meta.target.clone())
            };
            meta.migration_source = params
                .meta
                .migration_source
                .clone()
                .or_else(|| existing_meta.as_ref().and_then(|meta| meta.migration_source.clone()));
            meta.cron_automation_id = params
                .meta
                .cron_automation_id
                .clone()
                .or_else(|| {
                    existing_meta
                        .as_ref()
                        .and_then(|meta| meta.cron_automation_id.clone())
                });
            meta.off_peak_task_id = params
                .meta
                .off_peak_task_id
                .clone()
                .or_else(|| {
                    existing_meta
                        .as_ref()
                        .and_then(|meta| meta.off_peak_task_id.clone())
                });
            meta.unread_at = params
                .meta
                .unread_at
                .or_else(|| existing_meta.as_ref().and_then(|meta| meta.unread_at));
            meta.updated_at = updated_at;

            let persisted = self.write_record(
                connection,
                WriteRecord {
                    meta: meta.clone(),
                    pinned: params
                        .pinned
                        .unwrap_or_else(|| existing.as_ref().is_some_and(|row| row.pinned == 1)),
                    archived: params
                        .archived
                        .unwrap_or_else(|| existing.as_ref().is_some_and(|row| row.archived == 1)),
                    deleted: params
                        .deleted
                        .unwrap_or_else(|| existing.as_ref().is_some_and(|row| row.deleted == 1)),
                    title_overridden,
                    write_unread_at: false,
                    searchable_text: params.searchable_text.clone(),
                },
            )?;

            let had_cron = existing_meta
                .as_ref()
                .and_then(|meta| meta.cron_automation_id.as_ref())
                .is_some();
            if meta.cron_automation_id.is_some() && !had_cron {
                self.ensure_cron_group_membership(connection, &meta)?;
            }
            let had_off_peak = existing_meta
                .as_ref()
                .and_then(|meta| meta.off_peak_task_id.as_ref())
                .is_some();
            if meta.off_peak_task_id.is_some() && !had_off_peak {
                let meta_key = resolve_workspace_key(
                    &meta.workspace_path,
                    meta.workspace_identity.as_deref(),
                );
                self.ensure_off_peak_group_membership(
                    connection,
                    MembershipTarget {
                        workspace_key: &meta_key,
                        workspace_path: &meta.workspace_path,
                        workspace_identity: meta.workspace_identity.as_deref(),
                        task_id: &meta.task_id,
                    },
                )?;
            }
            Ok(persisted)
        }
    }

    pub fn seed_task_meta_if_missing(&self, meta: &ModeTaskMeta) -> Result<ModeTaskMeta> {
        self.ensure_ready()?;
        let workspace_key =
            resolve_workspace_key(&meta.workspace_path, meta.workspace_identity.as_deref());
        let key = format!("{workspace_key}\u{0000}{}", meta.task_id);
        self.with_write_conn(&key, |connection| {
            if let Some(existing) = self.get_task_row(connection, &workspace_key, &meta.task_id)? {
                return Ok(row_to_meta(&existing));
            }
            self.write_record(
                connection,
                WriteRecord {
                    meta: meta.clone(),
                    pinned: false,
                    archived: false,
                    deleted: false,
                    title_overridden: meta.title_overridden.unwrap_or(false),
                    write_unread_at: false,
                    searchable_text: None,
                },
            )
        })
    }

    pub fn clear_task_unread_if_matches(
        &self,
        params: &TaskRef,
        expected_unread_at: i64,
    ) -> Result<ClearUnreadResult> {
        self.ensure_ready()?;
        let workspace_key =
            resolve_workspace_key(&params.workspace_path, params.workspace_identity.as_deref());
        let key = format!("{workspace_key}\u{0000}{}", params.task_id);
        self.with_write_conn(&key, |connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<ClearUnreadResult> {
                let row = self
                    .get_task_row(connection, &workspace_key, &params.task_id)?
                    .filter(|row| row.deleted != 1)
                    .ok_or_else(|| TaskIndexRepoError::TaskNotFound(params.task_id.clone()))?;
                let current = row_to_meta(&row);
                if current.unread_at != Some(expected_unread_at) {
                    connection.execute_batch("COMMIT")?;
                    return Ok(ClearUnreadResult {
                        meta: current,
                        cleared: false,
                    });
                }
                let mut next = current;
                next.unread_at = None;
                let persisted = self.write_record(
                    connection,
                    WriteRecord {
                        meta: next,
                        pinned: row.pinned == 1,
                        archived: row.archived == 1,
                        deleted: false,
                        title_overridden: row.title_overridden == 1,
                        write_unread_at: true,
                        searchable_text: None,
                    },
                )?;
                connection.execute_batch("COMMIT")?;
                Ok(ClearUnreadResult {
                    meta: persisted,
                    cleared: true,
                })
            })();
            if result.is_err() {
                let _ = connection.execute_batch("ROLLBACK");
            }
            result
        })
    }

    pub fn delete_archived_task(&self, params: &TaskRef) -> Result<Option<ModeTaskMeta>> {
        self.ensure_ready()?;
        let workspace_key =
            resolve_workspace_key(&params.workspace_path, params.workspace_identity.as_deref());
        let key = format!("{workspace_key}\u{0000}{}", params.task_id);
        self.with_write_conn(&key, |connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<Option<ModeTaskMeta>> {
                let Some(row) = self.get_task_row(connection, &workspace_key, &params.task_id)?
                else {
                    connection.execute_batch("COMMIT")?;
                    return Ok(None);
                };
                if row.deleted == 1 || row.archived != 1 {
                    connection.execute_batch("COMMIT")?;
                    return Ok(None);
                }
                let meta = self.write_record(
                    connection,
                    WriteRecord {
                        meta: row_to_meta(&row),
                        pinned: row.pinned == 1,
                        archived: true,
                        deleted: true,
                        title_overridden: row.title_overridden == 1,
                        write_unread_at: false,
                        searchable_text: None,
                    },
                )?;
                self.delete_task_grouping_references(
                    connection,
                    &row.workspace_key,
                    &row.task_id,
                )?;
                connection.execute_batch("COMMIT")?;
                Ok(Some(meta))
            })();
            if result.is_err() {
                let _ = connection.execute_batch("ROLLBACK");
            }
            result
        })
    }

    pub fn update_task_state(
        &self,
        params: &TaskRef,
        patch: &TaskIndexStatePatch,
    ) -> Result<ModeTaskMeta> {
        self.ensure_ready()?;
        let workspace_key =
            resolve_workspace_key(&params.workspace_path, params.workspace_identity.as_deref());
        let key = format!("{workspace_key}\u{0000}{}", params.task_id);
        self.with_write_conn(&key, |connection| {
            let deleting = patch.deleted == Some(true);
            let mutating_unread_at = patch.unread_at.is_some();
            let transactional = deleting || mutating_unread_at;
            if transactional {
                connection.execute_batch("BEGIN IMMEDIATE")?;
            }
            let result = (|| -> Result<ModeTaskMeta> {
                let row = self
                    .get_task_row(connection, &workspace_key, &params.task_id)?
                    .filter(|row| row.deleted != 1)
                    .ok_or_else(|| TaskIndexRepoError::TaskNotFound(params.task_id.clone()))?;
                let current = row_to_meta(&row);
                let last_unread_at = row
                    .last_unread_at
                    .max(row.unread_at.unwrap_or(0))
                    .max(current.unread_at.unwrap_or(0));
                let unread_at = match patch.unread_at {
                    Some(Some(requested)) => Some(requested.max(last_unread_at.saturating_add(1))),
                    Some(None) => None,
                    None => current.unread_at,
                };
                let mut next = current;
                if let Some(title) = &patch.title {
                    next.title = title.clone();
                }
                if let Some(title_overridden) = patch.title_overridden {
                    next.title_overridden = Some(title_overridden);
                }
                if let Some(model) = &patch.model {
                    next.model = Some(model.clone());
                }
                if let Some(updated_at) = patch.updated_at {
                    next.updated_at = updated_at;
                }
                next.unread_at = unread_at;
                if let Some(status) = &patch.status {
                    next.status = Some(status.clone());
                }
                if patch.last_error.is_some() {
                    next.last_error = patch.last_error.clone().flatten();
                }
                if patch.target.is_some() {
                    next.target = patch.target.clone();
                }
                let persisted = self.write_record(
                    connection,
                    WriteRecord {
                        meta: next,
                        pinned: patch.pinned.unwrap_or(row.pinned == 1),
                        archived: patch.archived.unwrap_or(row.archived == 1),
                        deleted: patch.deleted.unwrap_or(row.deleted == 1),
                        title_overridden: patch
                            .title_overridden
                            .unwrap_or(row.title_overridden == 1),
                        write_unread_at: mutating_unread_at,
                        searchable_text: None,
                    },
                )?;
                if deleting {
                    self.delete_task_grouping_references(
                        connection,
                        &row.workspace_key,
                        &row.task_id,
                    )?;
                }
                if transactional {
                    connection.execute_batch("COMMIT")?;
                }
                Ok(persisted)
            })();
            if result.is_err() && transactional {
                let _ = connection.execute_batch("ROLLBACK");
            }
            result
        })
    }

    pub fn apply_agent_patch(
        &self,
        params: &TaskRef,
        patch: &AgentPatch,
    ) -> Result<Option<ModeTaskMeta>> {
        self.ensure_ready()?;
        let workspace_key =
            resolve_workspace_key(&params.workspace_path, params.workspace_identity.as_deref());
        let key = format!("{workspace_key}\u{0000}{}", params.task_id);
        self.with_write_conn(&key, |connection| {
            let Some(row) = self
                .get_task_row(connection, &workspace_key, &params.task_id)?
                .filter(|row| row.deleted != 1)
            else {
                return Ok(None);
            };
            let current = row_to_meta(&row);
            let can_accept_agent_title = row.title_overridden != 1;
            let mut next = current;
            if can_accept_agent_title {
                if let Some(title) = patch.title.as_deref().filter(|title| !title.is_empty()) {
                    next.title = title.to_owned();
                }
            }
            next.title_overridden = Some(row.title_overridden == 1);
            if let Some(updated_at) = patch.updated_at {
                next.updated_at = updated_at;
            }
            if let Some(status) = &patch.status {
                next.status = Some(status.clone());
            }
            if patch.last_error.is_some() {
                next.last_error = patch.last_error.clone().flatten();
            }
            if patch.target.is_some() {
                next.target = patch.target.clone();
            }
            let persisted = self.write_record(
                connection,
                WriteRecord {
                    meta: next,
                    pinned: row.pinned == 1,
                    archived: row.archived == 1,
                    deleted: row.deleted == 1,
                    title_overridden: row.title_overridden == 1,
                    write_unread_at: false,
                    searchable_text: None,
                },
            )?;
            Ok(Some(persisted))
        })
    }

    pub fn archive_stale_tasks(&self, params: &ArchiveStaleTasksParams) -> Result<Vec<ModeTaskMeta>> {
        self.ensure_ready()?;
        let days = params.older_than_days.floor().max(1.0) as i64;
        let cutoff = self.now().saturating_sub(days.saturating_mul(24 * 60 * 60 * 1000));
        let workspace_key =
            resolve_workspace_key(&params.workspace_path, params.workspace_identity.as_deref());
        let mut where_clause = vec![
            "workspace_key = ?".to_owned(),
            "deleted = 0".to_owned(),
            "archived = 0".to_owned(),
            "pinned = 0".to_owned(),
            "unread_at IS NULL".to_owned(),
            "updated_at < ?".to_owned(),
            "task_status = 'completed'".to_owned(),
        ];
        let mut arguments: Vec<SqlValue> = vec![
            SqlValue::Text(workspace_key),
            SqlValue::Integer(cutoff),
        ];
        if let Some(provider) = &params.provider {
            where_clause.push(format!("{PROVIDER_COLUMN} = ?"));
            arguments.push(SqlValue::Text(provider.clone()));
        }
        self.with_conn(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {ROW_COLUMNS} FROM tasks
                 WHERE {}
                 ORDER BY updated_at DESC, created_at DESC, task_id DESC",
                where_clause.join(" AND ")
            ))?;
            let rows = statement
                .query_map(params_from_iter(arguments.iter()), TaskIndexRow::read)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.is_empty() {
                return Ok(Vec::new());
            }
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let update_result = (|| -> Result<()> {
                for row in &rows {
                    connection.execute(
                        "UPDATE tasks SET archived = 1 WHERE workspace_key = ? AND task_id = ?",
                        params![row.workspace_key, row.task_id],
                    )?;
                }
                Ok(())
            })();
            match update_result {
                Ok(()) => connection.execute_batch("COMMIT")?,
                Err(error) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    return Err(error);
                }
            }
            Ok(rows.iter().map(row_to_meta).collect())
        })
    }
}

pub(crate) fn bool_int(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn should_preserve_newer_terminal_status(
    existing_meta: Option<&ModeTaskMeta>,
    incoming_meta: &ModeTaskMeta,
) -> bool {
    let Some(existing) = existing_meta else {
        return false;
    };
    if !is_terminal_status(existing.status.as_deref()) {
        return false;
    }
    // TS 是 `if (incomingMeta.status && incomingMeta.status !== "running")`：
    // 空字符串在 JS 里是 falsy，不能当作“非 running 终态”。
    if incoming_meta
        .status
        .as_deref()
        .is_some_and(|status| !status.is_empty() && status != "running")
    {
        return false;
    }
    existing.updated_at > incoming_meta.updated_at
}

fn resolve_row_workspace_identity(row: &TaskIndexRow) -> Option<String> {
    let column_identity = row.workspace_identity.as_deref().map(js_trim);
    if let Some(identity) = column_identity {
        if identity == row.workspace_key {
            return Some(identity.to_owned());
        }
    }
    if is_remote_workspace_identity(&row.workspace_key) {
        return Some(row.workspace_key.clone());
    }
    None
}

pub(crate) fn row_to_meta(row: &TaskIndexRow) -> ModeTaskMeta {
    let workspace_identity = resolve_row_workspace_identity(row);
    let parsed = serde_json::from_str::<Value>(&row.meta_json)
        .ok()
        .and_then(|value| parse_mode_task_meta(&value));
    if let Some(mut meta) = parsed {
        // meta_json 只提供内容；行主键/产品壳标量必须覆盖旧值。
        meta.task_id = row.task_id.clone();
        meta.workspace_path = row.workspace_path.clone();
        meta.workspace_identity = workspace_identity;
        meta.unread_at = row.unread_at;
        meta.cron_automation_id = meta.cron_automation_id.or_else(|| row.cron_automation_id.clone());
        meta.off_peak_task_id = meta.off_peak_task_id.or_else(|| row.off_peak_task_id.clone());
        meta.title_overridden = Some(row.title_overridden == 1);
        return meta;
    }

    ModeTaskMeta {
        task_id: row.task_id.clone(),
        trace_id: format!("mode-{}", row.task_id),
        title: row.title.clone(),
        title_overridden: Some(row.title_overridden == 1),
        workspace_path: row.workspace_path.clone(),
        workspace_identity,
        workspace_purpose: None,
        created_at: row.created_at,
        updated_at: row.updated_at,
        mode: row.mode.clone(),
        model: row.model.clone(),
        thought_level: None,
        runtime_epoch: None,
        provider: row
            .provider
            .as_deref()
            .filter(|provider| *provider == MODE_AGENT_PROVIDER)
            .map(str::to_owned),
        migration_source: row.migration_source.clone(),
        forked_from_task_id: row.forked_from_task_id.clone(),
        cron_automation_id: row.cron_automation_id.clone(),
        off_peak_task_id: row.off_peak_task_id.clone(),
        unread_at: row.unread_at,
        status: row.task_status.clone(),
        last_error: None,
        change_summary: None,
        target: None,
    }
}

pub(crate) fn row_to_task_list_item(row: &TaskIndexRow, search: Option<&str>) -> TaskListItem {
    let meta = row_to_meta(row);
    let snippets = build_search_snippets(&row.searchable_text, search);
    if snippets.is_empty() {
        return TaskListItem {
            meta,
            search_snippet: None,
            search_snippets: None,
        };
    }
    TaskListItem {
        search_snippet: snippets.first().cloned(),
        search_snippets: Some(snippets),
        meta,
    }
}

pub(crate) fn normalize_workspace_keys(scopes: &[TaskListWorkspaceScope]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for scope in scopes {
        let key = resolve_workspace_key(&scope.workspace_path, scope.workspace_identity.as_deref());
        if js_trim(&key).is_empty() || !seen.insert(key.clone()) {
            continue;
        }
        keys.push(key);
    }
    // TS 用 `localeCompare`；这里用码点序（见 spec 差异表）。
    keys.sort();
    keys
}
