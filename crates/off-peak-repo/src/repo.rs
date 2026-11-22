//! 闲时任务仓库：`off_peak_tasks` 的状态机守卫写入与调度认领。
//!
//! 行为真源：`packages/services/src/session/offPeakTaskRepo.ts`。规范见
//! `docs/specs/off-peak-repo.md`。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use uuid::Uuid;

use crate::error::{OffPeakRepoError, Result};
use model_selection::{parse_model_selection, serialize_model_selection, ModelSelection};

/// 认领超时回收：`claim_running=1` 超过该时长仍未结算视为持有者已崩溃。
///
/// 与 automation 的 `CLAIM_STALE_MS` 语义相同、常量独立一份，勿互相引用。
pub const OFF_PEAK_CLAIM_STALE_MS: i64 = 10 * 60_000;

/// 终态 SQL IN 片段；`OFF_PEAK_TERMINAL_STATUSES` 的单一来源投影。
const TERMINAL_SQL_LIST: &str = "'completed', 'failed', 'cancelled'";

const ROW_COLUMNS: &str = "off_peak_task_id, server_ticket_id, title, conversation_id, session_id, \
prompt, permission_mode, model, thought_level, model_selection, workspace_key, workspace_path, \
workspace_identity, status, queued_at, started_at, ended_at, failure_reason, files_changed, \
settled_at, history_deleted_at, registered_at, schedulable, queue_position, next_poll_at, \
claim_running, claimed_at, attempt_count, last_error, created_at, updated_at";

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn prepared_registry() -> &'static Mutex<HashSet<PathBuf>> {
    static REGISTRY: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

fn registry_contains(registry: &Mutex<HashSet<PathBuf>>, path: &Path) -> bool {
    registry
        .lock()
        .expect("off-peak registry poisoned")
        .contains(path)
}

// ── 公开类型 ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectionIssue {
    pub code: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OffPeakTask {
    pub off_peak_task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_ticket_id: Option<String>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_title: Option<String>,
    pub prompt: String,
    pub permission_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<ModelSelection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_selection_issue: Option<ModelSelectionIssue>,
    pub workspace_key: String,
    pub workspace_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    pub status: String,
    pub queued_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_deleted_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<i64>,
    pub schedulable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_poll_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug)]
pub struct CreateParams {
    pub title: String,
    pub prompt: String,
    pub permission_mode: String,
    pub model_selection: ModelSelection,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    /// 会话内创建：绑定创建时所在会话；表单创建为 `None`。
    pub bound_session_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct CreateOptions {
    pub now: Option<i64>,
    pub off_peak_task_id: Option<String>,
    pub server_ticket_id: Option<String>,
    pub queue_position: Option<i64>,
    pub registered_at: Option<i64>,
    pub schedulable: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct TaskScope {
    pub workspace_path: Option<String>,
    pub workspace_identity: Option<String>,
}

/// 编辑窗口期的模型选择补丁：`Replace` 明确替换；`Clear` 对应 TS 的
/// `modelSelection: null`（直接拒绝编辑）。
#[derive(Clone, Debug)]
pub enum EditableModelSelection {
    Replace(ModelSelection),
    Clear,
}

#[derive(Clone, Debug, Default)]
pub struct EditableFields {
    pub title: Option<String>,
    pub prompt: Option<String>,
    pub permission_mode: Option<String>,
    pub model_selection: Option<EditableModelSelection>,
}

#[derive(Clone, Debug, Default)]
pub struct SchedulingPatch {
    pub schedulable: Option<bool>,
    /// `None` = 未传；`Some(None)` = 显式 null。
    pub queue_position: Option<Option<i64>>,
    pub next_poll_at: Option<Option<i64>>,
    pub server_ticket_id: Option<String>,
    pub registered_at: Option<i64>,
    pub now: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct MarkRunningOptions {
    pub started_at: i64,
    pub conversation_id: Option<String>,
    pub session_id: Option<String>,
    pub server_ticket_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MarkTerminalOptions {
    pub status: String,
    pub ended_at: i64,
    pub failure_reason: Option<String>,
    pub files_changed: Option<i64>,
    pub dispatch_error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ReleaseClaimOptions {
    pub error: Option<String>,
    pub now: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct InvalidateOptions {
    pub now: Option<i64>,
}

// ── 行结构 ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct OffPeakRow {
    off_peak_task_id: String,
    server_ticket_id: Option<String>,
    title: String,
    conversation_id: Option<String>,
    session_id: Option<String>,
    prompt: String,
    permission_mode: String,
    model_selection: Option<String>,
    workspace_key: String,
    workspace_path: String,
    workspace_identity: Option<String>,
    status: String,
    queued_at: i64,
    started_at: Option<i64>,
    ended_at: Option<i64>,
    failure_reason: Option<String>,
    files_changed: Option<i64>,
    settled_at: Option<i64>,
    history_deleted_at: Option<i64>,
    registered_at: Option<i64>,
    schedulable: i64,
    queue_position: Option<i64>,
    next_poll_at: Option<i64>,
    claim_running: i64,
    claimed_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

impl OffPeakRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            off_peak_task_id: row.get(0)?,
            server_ticket_id: row.get(1)?,
            title: row.get(2)?,
            conversation_id: row.get(3)?,
            session_id: row.get(4)?,
            prompt: row.get(5)?,
            permission_mode: row.get(6)?,
            model_selection: row.get(9)?,
            workspace_key: row.get(10)?,
            workspace_path: row.get(11)?,
            workspace_identity: row.get(12)?,
            status: row.get(13)?,
            queued_at: row.get(14)?,
            started_at: row.get(15)?,
            ended_at: row.get(16)?,
            failure_reason: row.get(17)?,
            files_changed: row.get(18)?,
            settled_at: row.get(19)?,
            history_deleted_at: row.get(20)?,
            registered_at: row.get(21)?,
            schedulable: row.get(22)?,
            queue_position: row.get(23)?,
            next_poll_at: row.get(24)?,
            claim_running: row.get(25)?,
            claimed_at: row.get(26)?,
            created_at: row.get(29)?,
            updated_at: row.get(30)?,
        })
    }
}

fn read_selection(row: &OffPeakRow) -> Option<ModelSelection> {
    row.model_selection
        .as_deref()
        .and_then(parse_model_selection)
}

fn row_to_task(row: &OffPeakRow, session_title: Option<String>) -> OffPeakTask {
    let model_selection = read_selection(row);
    let model_selection_issue = if model_selection.is_some() {
        None
    } else {
        Some(ModelSelectionIssue {
            code: "repair-required",
        })
    };
    OffPeakTask {
        off_peak_task_id: row.off_peak_task_id.clone(),
        server_ticket_id: row.server_ticket_id.clone(),
        title: row.title.clone(),
        conversation_id: row.conversation_id.clone(),
        session_id: row.session_id.clone(),
        // TS: `...(row.session_title ? { sessionTitle } : {})`，空串不输出。
        session_title: session_title.filter(|title| !title.is_empty()),
        prompt: row.prompt.clone(),
        permission_mode: row.permission_mode.clone(),
        model_selection,
        model_selection_issue,
        workspace_key: row.workspace_key.clone(),
        workspace_path: row.workspace_path.clone(),
        workspace_identity: row.workspace_identity.clone(),
        status: row.status.clone(),
        queued_at: row.queued_at,
        started_at: row.started_at,
        ended_at: row.ended_at,
        failure_reason: row.failure_reason.clone(),
        files_changed: row.files_changed,
        settled_at: row.settled_at,
        history_deleted_at: row.history_deleted_at,
        registered_at: row.registered_at,
        schedulable: row.schedulable == 1,
        queue_position: row.queue_position,
        next_poll_at: row.next_poll_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

struct State {
    db: Option<Connection>,
    db_path: Option<PathBuf>,
}

pub struct OffPeakTaskRepo {
    resolved_db_path: Option<PathBuf>,
    startup_busy_timeout_ms: u64,
    state: Mutex<State>,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl OffPeakTaskRepo {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self::with_clock(db_path, now_millis)
    }

    pub fn with_busy_timeout(db_path: impl Into<PathBuf>, busy_timeout_ms: u64) -> Self {
        Self {
            resolved_db_path: Some(db_path.into()),
            startup_busy_timeout_ms: busy_timeout_ms,
            state: Mutex::new(State {
                db: None,
                db_path: None,
            }),
            clock: Box::new(now_millis),
        }
    }

    pub fn with_clock(
        db_path: impl Into<PathBuf>,
        clock: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            resolved_db_path: Some(db_path.into()),
            startup_busy_timeout_ms: 5000,
            state: Mutex::new(State {
                db: None,
                db_path: None,
            }),
            clock: Box::new(clock),
        }
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    fn resolve_db_path(&self) -> Result<PathBuf> {
        self.resolved_db_path.clone().ok_or_else(|| {
            OffPeakRepoError::Message("OffPeakTaskRepo 需要显式 db 路径".to_owned())
        })
    }

    /// 打开库并完成一次性准备（迁移 + `awaiting_approval` 历史修复）。
    pub fn ensure_ready(&self) -> Result<()> {
        let path = self.resolve_db_path()?;
        let mut state = self.state.lock().expect("off-peak state poisoned");
        if state.db_path.as_deref() != Some(path.as_path()) {
            state.db = None;
            state.db_path = None;
        }
        if state.db.is_none() {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)?;
                }
            }
            let connection = Connection::open(&path)?;
            connection.busy_timeout(Duration::from_millis(self.startup_busy_timeout_ms))?;
            connection.execute_batch("PRAGMA journal_mode = WAL")?;
            connection.execute_batch("PRAGMA synchronous = NORMAL")?;
            state.db = Some(connection);
            state.db_path = Some(path.clone());
        }
        let connection = state.db.as_ref().expect("connection was just opened");
        if registry_contains(prepared_registry(), &path)
            && tasks_db::are_migrations_applied(connection)?
        {
            return Ok(());
        }
        if !tasks_db::are_migrations_applied(connection)? {
            tasks_db::run_migrations(connection, tasks_db::RunOptions::default())?;
        }
        // 早期预留的 awaiting_approval 生产链路从未写入；迁移遗留行回到 running。
        connection.execute(
            "UPDATE off_peak_tasks SET status = 'running', updated_at = ? WHERE status = 'awaiting_approval'",
            params![self.now()],
        )?;
        prepared_registry()
            .lock()
            .expect("off-peak registry poisoned")
            .insert(path);
        Ok(())
    }

    pub fn close(&self) {
        let mut state = self.state.lock().expect("off-peak state poisoned");
        if let Some(connection) = state.db.take() {
            let _ = connection.close();
        }
        state.db_path = None;
    }

    fn with_conn<T>(&self, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let state = self.state.lock().expect("off-peak state poisoned");
        let connection = state
            .db
            .as_ref()
            .ok_or(OffPeakRepoError::NotInitialized)?;
        operation(connection)
    }

    fn has_tasks_index_table(&self, connection: &Connection) -> Result<bool> {
        let found: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tasks'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    fn get_row(&self, connection: &Connection, off_peak_task_id: &str) -> Result<Option<OffPeakRow>> {
        Ok(connection
            .query_row(
                &format!("SELECT {ROW_COLUMNS} FROM off_peak_tasks WHERE off_peak_task_id = ?"),
                params![off_peak_task_id],
                OffPeakRow::read,
            )
            .optional()?)
    }

    fn require_row(&self, connection: &Connection, off_peak_task_id: &str) -> Result<OffPeakRow> {
        self.get_row(connection, off_peak_task_id)?
            .ok_or_else(|| {
                OffPeakRepoError::Message(format!(
                    "Off-Peak task 不存在: {off_peak_task_id}"
                ))
            })
    }

    // ── 管理 CRUD ────────────────────────────────────────────────────────

    pub fn create(&self, params: &CreateParams, options: &CreateOptions) -> Result<OffPeakTask> {
        self.ensure_ready()?;
        let now = options.now.unwrap_or_else(|| self.now());
        let off_peak_task_id = options
            .off_peak_task_id
            .clone()
            .unwrap_or_else(|| format!("offpeak-{}", Uuid::new_v4()));
        let workspace_key = workspace_identity::workspace_key(
            &params.workspace_path,
            params.workspace_identity.as_deref(),
        );
        let model_selection = serialize_model_selection(&params.model_selection)
            .map_err(OffPeakRepoError::Message)?;
        self.with_conn(|connection| {
            connection.execute(
                "INSERT INTO off_peak_tasks (
                   off_peak_task_id, server_ticket_id, title, conversation_id, session_id,
                   prompt, permission_mode, model, thought_level, model_selection,
                   workspace_key, workspace_path, workspace_identity,
                   status, queued_at, registered_at, schedulable, queue_position,
                   claim_running, attempt_count, created_at, updated_at
                 ) VALUES (?, ?, ?, NULL, ?, ?, ?, NULL, NULL, ?, ?, ?, ?, 'queued', ?, ?, ?, ?, 0, 0, ?, ?)",
                params![
                    off_peak_task_id,
                    options.server_ticket_id,
                    params.title,
                    params.bound_session_id,
                    params.prompt,
                    params.permission_mode,
                    model_selection,
                    workspace_key,
                    params.workspace_path,
                    params.workspace_identity,
                    now,
                    options.registered_at,
                    options.schedulable.map(|value| if value { 1 } else { 0 }).unwrap_or(0),
                    options.queue_position,
                    now,
                    now
                ],
            )?;
            let row = self.require_row(connection, &off_peak_task_id)?;
            Ok(row_to_task(&row, None))
        })
    }

    pub fn list(&self, scope: &TaskScope) -> Result<Vec<OffPeakTask>> {
        self.ensure_ready()?;
        let workspace_key = scope.workspace_path.as_deref().map(|path| {
            workspace_identity::workspace_key(path, scope.workspace_identity.as_deref())
        });
        self.with_conn(|connection| {
            if self.has_tasks_index_table(connection)? {
                // `t.*` 列序与 ROW_COLUMNS 一致；显式列名会在 JOIN 里与 tasks 撞名。
                let mut statement = connection.prepare(
                    "SELECT t.*, s.title AS session_title FROM off_peak_tasks t
                     LEFT JOIN tasks s
                       ON s.workspace_key = t.workspace_key AND s.task_id = t.session_id
                     WHERE (?1 IS NULL OR t.workspace_key = ?1)
                     ORDER BY t.created_at DESC",
                )?;
                let rows = statement
                    .query_map(params![workspace_key], |row| {
                        let parsed = OffPeakRow::read(row)?;
                        // `SELECT {ROW_COLUMNS}, s.title` 的联查标题固定是第 32 列。
                        let title: Option<String> = row.get(31)?;
                        Ok((parsed, title))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows
                    .into_iter()
                    .map(|(row, title)| row_to_task(&row, title))
                    .collect())
            } else {
                let mut statement = connection.prepare(&format!(
                    "SELECT {ROW_COLUMNS} FROM off_peak_tasks
                     WHERE (?1 IS NULL OR workspace_key = ?1)
                     ORDER BY created_at DESC"
                ))?;
                let rows = statement
                    .query_map(params![workspace_key], OffPeakRow::read)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows.iter().map(|row| row_to_task(row, None)).collect())
            }
        })
    }

    pub fn get(&self, off_peak_task_id: &str) -> Result<Option<OffPeakTask>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            Ok(self
                .get_row(connection, off_peak_task_id)?
                .map(|row| row_to_task(&row, None)))
        })
    }

    pub fn invalidate_model_selection(
        &self,
        off_peak_task_id: &str,
        model_selection: &ModelSelection,
        options: &InvalidateOptions,
    ) -> Result<Option<OffPeakTask>> {
        self.ensure_ready()?;
        let now = options.now.unwrap_or_else(|| self.now());
        self.with_conn(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let outcome = (|| -> Result<Option<OffPeakTask>> {
                let Some(current) = self.get_row(connection, off_peak_task_id)? else {
                    return Ok(None);
                };
                if let Some(current_selection) = read_selection(&current) {
                    let differs = current_selection.provider_id != model_selection.provider_id
                        || current_selection.model_id != model_selection.model_id
                        || current_selection
                            .options
                            .as_ref()
                            .and_then(|options| options.reasoning_level.as_deref())
                            != model_selection
                                .options
                                .as_ref()
                                .and_then(|options| options.reasoning_level.as_deref());
                    if differs {
                        return Ok(Some(row_to_task(&current, None)));
                    }
                }
                connection.execute(
                    "UPDATE off_peak_tasks
                     SET model = ?, thought_level = ?, model_selection = NULL,
                         schedulable = 0, updated_at = ?
                     WHERE off_peak_task_id = ?",
                    params![
                        model_selection.model_id,
                        model_selection
                            .options
                            .as_ref()
                            .and_then(|options| options.reasoning_level.clone()),
                        now,
                        off_peak_task_id
                    ],
                )?;
                Ok(self
                    .get_row(connection, off_peak_task_id)?
                    .map(|row| row_to_task(&row, None)))
            })();
            match outcome {
                Ok(value) => {
                    connection.execute_batch("COMMIT")?;
                    Ok(value)
                }
                Err(error) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn delete(&self, off_peak_task_id: &str) -> Result<()> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            connection.execute(
                "DELETE FROM off_peak_tasks WHERE off_peak_task_id = ?",
                params![off_peak_task_id],
            )?;
            Ok(())
        })
    }

    pub fn mark_history_deleted(
        &self,
        off_peak_task_id: &str,
        options: &InvalidateOptions,
    ) -> Result<Option<OffPeakTask>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let Some(row) = self.get_row(connection, off_peak_task_id)? else {
                return Ok(None);
            };
            if row.started_at.is_none() || row.history_deleted_at.is_some() {
                return Ok(Some(row_to_task(&row, None)));
            }
            let now = options.now.unwrap_or_else(|| self.now());
            connection.execute(
                "UPDATE off_peak_tasks
                 SET history_deleted_at = ?, updated_at = ?
                 WHERE off_peak_task_id = ? AND started_at IS NOT NULL",
                params![now, now, off_peak_task_id],
            )?;
            Ok(Some(row_to_task(
                &self.require_row(connection, off_peak_task_id)?,
                None,
            )))
        })
    }

    pub fn count_non_terminal(&self) -> Result<i64> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            Ok(connection.query_row(
                &format!(
                    "SELECT COUNT(*) FROM off_peak_tasks WHERE status NOT IN ({TERMINAL_SQL_LIST})"
                ),
                [],
                |row| row.get(0),
            )?)
        })
    }

    pub fn has_active_bound_task(&self, workspace_key: &str, session_id: &str) -> Result<bool> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let hit: Option<i64> = connection
                .query_row(
                    &format!(
                        "SELECT 1 FROM off_peak_tasks
                         WHERE workspace_key = ? AND session_id = ?
                           AND status NOT IN ({TERMINAL_SQL_LIST})
                         LIMIT 1"
                    ),
                    params![workspace_key, session_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(hit.is_some())
        })
    }

    pub fn count_active(&self) -> Result<i64> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            Ok(connection.query_row(
                "SELECT COUNT(*) FROM off_peak_tasks WHERE status = 'running'",
                [],
                |row| row.get(0),
            )?)
        })
    }

    /// 编辑窗口期字段：仅 queued/paused 可编辑；`Clear` 直接拒绝。
    pub fn update_editable_fields(
        &self,
        off_peak_task_id: &str,
        fields: &EditableFields,
        options: &InvalidateOptions,
    ) -> Result<Option<OffPeakTask>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let Some(row) = self.get_row(connection, off_peak_task_id)? else {
                return Ok(None);
            };
            if row.status != "queued" && row.status != "paused" {
                return Ok(None);
            }
            let next_selection = match &fields.model_selection {
                Some(EditableModelSelection::Clear) => return Ok(None),
                Some(EditableModelSelection::Replace(selection)) => Some(selection.clone()),
                None => read_selection(&row),
            };
            let Some(next_selection) = next_selection else {
                return Err(OffPeakRepoError::Message(format!(
                    "Off-Peak task 缺少有效 ModelSelection: {off_peak_task_id}"
                )));
            };
            let model_selection = serialize_model_selection(&next_selection)
                .map_err(OffPeakRepoError::Message)?;
            let now = options.now.unwrap_or_else(|| self.now());
            connection.execute(
                "UPDATE off_peak_tasks SET
                   title = ?, prompt = ?, permission_mode = ?,
                   model_selection = ?, updated_at = ?
                 WHERE off_peak_task_id = ?",
                params![
                    fields.title.as_deref().unwrap_or(&row.title),
                    fields.prompt.as_deref().unwrap_or(&row.prompt),
                    fields
                        .permission_mode
                        .as_deref()
                        .unwrap_or(&row.permission_mode),
                    model_selection,
                    now,
                    off_peak_task_id
                ],
            )?;
            Ok(Some(row_to_task(
                &self.require_row(connection, off_peak_task_id)?,
                None,
            )))
        })
    }

    // ── 服务端同步快照 ────────────────────────────────────────────────────

    pub fn update_scheduling_snapshot(
        &self,
        off_peak_task_id: &str,
        patch: &SchedulingPatch,
    ) -> Result<()> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let Some(row) = self.get_row(connection, off_peak_task_id)? else {
                return Ok(());
            };
            let schedulable = patch
                .schedulable
                .map(|value| if value { 1 } else { 0 })
                .unwrap_or(row.schedulable);
            let queue_position = match patch.queue_position {
                None => row.queue_position,
                Some(value) => value,
            };
            let next_poll_at = match patch.next_poll_at {
                None => row.next_poll_at,
                Some(value) => value,
            };
            let server_ticket_id = patch
                .server_ticket_id
                .clone()
                .or_else(|| row.server_ticket_id.clone());
            let registered_at = patch.registered_at.or(row.registered_at);
            let now = patch.now.unwrap_or_else(|| self.now());
            connection.execute(
                "UPDATE off_peak_tasks SET
                   schedulable = ?, queue_position = ?, next_poll_at = ?,
                   server_ticket_id = ?, registered_at = ?, updated_at = ?
                 WHERE off_peak_task_id = ?",
                params![
                    schedulable,
                    queue_position,
                    next_poll_at,
                    server_ticket_id,
                    registered_at,
                    now,
                    off_peak_task_id
                ],
            )?;
            Ok(())
        })
    }

    // ── 调度状态机 ────────────────────────────────────────────────────────

    /// single-flight 认领：queued + schedulable=1 + 无在途认领；按 `queued_at` FIFO。
    pub fn claim_due(&self, now: i64) -> Result<Vec<OffPeakTask>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let outcome = (|| -> Result<Vec<OffPeakTask>> {
                connection.execute(
                    "UPDATE off_peak_tasks
                     SET claim_running = 0, claimed_at = NULL
                     WHERE claim_running = 1 AND claimed_at IS NOT NULL AND claimed_at <= ?",
                    params![now - OFF_PEAK_CLAIM_STALE_MS],
                )?;
                let due_rows = {
                    let mut statement = connection.prepare(&format!(
                        "SELECT {ROW_COLUMNS} FROM off_peak_tasks
                         WHERE status = 'queued' AND schedulable = 1 AND claim_running = 0
                         ORDER BY queued_at ASC, created_at ASC"
                    ))?;
                    let rows = statement
                        .query_map([], OffPeakRow::read)?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                let mut claimed = Vec::new();
                for row in due_rows {
                    // 历史行缺少 Provider 身份：留在列表等待修复，但不能被认领。
                    if read_selection(&row).is_none() {
                        continue;
                    }
                    let changes = connection.execute(
                        "UPDATE off_peak_tasks
                         SET claim_running = 1, claimed_at = ?, updated_at = ?
                         WHERE off_peak_task_id = ? AND claim_running = 0",
                        params![now, now, row.off_peak_task_id],
                    )?;
                    if changes == 1 {
                        let mut claimed_row = row.clone();
                        claimed_row.claim_running = 1;
                        claimed_row.claimed_at = Some(now);
                        claimed.push(row_to_task(&claimed_row, None));
                    }
                }
                Ok(claimed)
            })();
            match outcome {
                Ok(value) => {
                    connection.execute_batch("COMMIT")?;
                    Ok(value)
                }
                Err(error) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    /// 派发成功：queued→running；续跑段保留首段 `started_at`。
    pub fn mark_running(
        &self,
        off_peak_task_id: &str,
        options: &MarkRunningOptions,
    ) -> Result<Option<OffPeakTask>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let changes = connection.execute(
                "UPDATE off_peak_tasks
                 SET status = 'running',
                     started_at = COALESCE(started_at, ?),
                     conversation_id = COALESCE(?, conversation_id),
                     session_id = COALESCE(?, session_id),
                     server_ticket_id = COALESCE(?, server_ticket_id),
                     claim_running = 0, claimed_at = NULL,
                     last_error = NULL,
                     updated_at = ?
                 WHERE off_peak_task_id = ? AND status = 'queued'",
                params![
                    options.started_at,
                    options.conversation_id,
                    options.session_id,
                    options.server_ticket_id,
                    options.started_at,
                    off_peak_task_id
                ],
            )?;
            if changes != 1 {
                return Ok(None);
            }
            Ok(Some(row_to_task(
                &self.require_row(connection, off_peak_task_id)?,
                None,
            )))
        })
    }

    /// 终态落库；已终态的行拒绝二次迁移。
    pub fn mark_terminal(
        &self,
        off_peak_task_id: &str,
        options: &MarkTerminalOptions,
    ) -> Result<Option<OffPeakTask>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let changes = connection.execute(
                &format!(
                    "UPDATE off_peak_tasks
                     SET status = ?,
                         ended_at = ?,
                         failure_reason = ?,
                         files_changed = COALESCE(?, files_changed),
                         attempt_count = attempt_count + ?,
                         last_error = COALESCE(?, last_error),
                         schedulable = 0,
                         claim_running = 0, claimed_at = NULL,
                         updated_at = ?
                     WHERE off_peak_task_id = ? AND status NOT IN ({TERMINAL_SQL_LIST})"
                ),
                params![
                    options.status,
                    options.ended_at,
                    options.failure_reason,
                    options.files_changed,
                    if options.dispatch_error.is_some() { 1 } else { 0 },
                    options.dispatch_error,
                    options.ended_at,
                    off_peak_task_id
                ],
            )?;
            if changes != 1 {
                return Ok(None);
            }
            Ok(Some(row_to_task(
                &self.require_row(connection, off_peak_task_id)?,
                None,
            )))
        })
    }

    /// 用户 Pause / Continue：queued ⇄ paused；在途认领不可 Pause。
    pub fn set_paused(
        &self,
        off_peak_task_id: &str,
        paused: bool,
        options: &InvalidateOptions,
    ) -> Result<Option<OffPeakTask>> {
        self.ensure_ready()?;
        let now = options.now.unwrap_or_else(|| self.now());
        self.with_conn(|connection| {
            let (to, from) = if paused {
                ("paused", "queued")
            } else {
                ("queued", "paused")
            };
            let changes = connection.execute(
                "UPDATE off_peak_tasks
                 SET status = ?, updated_at = ?
                 WHERE off_peak_task_id = ? AND status = ? AND claim_running = 0",
                params![to, now, off_peak_task_id, from],
            )?;
            if changes != 1 {
                return Ok(None);
            }
            Ok(Some(row_to_task(
                &self.require_row(connection, off_peak_task_id)?,
                None,
            )))
        })
    }

    /// 释放认领；带 error 时累计 attempt_count 并记 last_error，不改 status。
    pub fn release_claim(
        &self,
        off_peak_task_id: &str,
        options: &ReleaseClaimOptions,
    ) -> Result<()> {
        self.ensure_ready()?;
        let now = options.now.unwrap_or_else(|| self.now());
        self.with_conn(|connection| {
            connection.execute(
                "UPDATE off_peak_tasks
                 SET claim_running = 0, claimed_at = NULL,
                     attempt_count = attempt_count + ?,
                     last_error = COALESCE(?, last_error),
                     updated_at = ?
                 WHERE off_peak_task_id = ? AND claim_running = 1",
                params![
                    if options.error.is_some() { 1 } else { 0 },
                    options.error,
                    now,
                    off_peak_task_id
                ],
            )?;
            Ok(())
        })
    }

    /// 启动回收：残留 running 置回 queued，并清理超时认领；返回回收条数。
    pub fn recover_interrupted(&self, now: i64) -> Result<i64> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let outcome = (|| -> Result<i64> {
                let recovered = connection.execute(
                    "UPDATE off_peak_tasks
                     SET status = 'queued', claim_running = 0, claimed_at = NULL, updated_at = ?
                     WHERE status = 'running'",
                    params![now],
                )?;
                connection.execute(
                    "UPDATE off_peak_tasks
                     SET claim_running = 0, claimed_at = NULL, updated_at = ?
                     WHERE claim_running = 1 AND claimed_at IS NOT NULL AND claimed_at <= ?",
                    params![now, now - OFF_PEAK_CLAIM_STALE_MS],
                )?;
                Ok(recovered as i64)
            })();
            match outcome {
                Ok(value) => {
                    connection.execute_batch("COMMIT")?;
                    Ok(value)
                }
                Err(error) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    /// 续跑回队：running → queued，保留会话与 started_at。
    pub fn requeue_for_continuation(
        &self,
        off_peak_task_id: &str,
        options: &InvalidateOptions,
    ) -> Result<Option<OffPeakTask>> {
        self.ensure_ready()?;
        let now = options.now.unwrap_or_else(|| self.now());
        self.with_conn(|connection| {
            let changes = connection.execute(
                "UPDATE off_peak_tasks
                 SET status = 'queued', schedulable = 0, queue_position = NULL,
                     claim_running = 0, claimed_at = NULL, updated_at = ?
                 WHERE off_peak_task_id = ? AND status = 'running'",
                params![now, off_peak_task_id],
            )?;
            if changes != 1 {
                return Ok(None);
            }
            Ok(Some(row_to_task(
                &self.require_row(connection, off_peak_task_id)?,
                None,
            )))
        })
    }

    pub fn list_non_terminal(&self) -> Result<Vec<OffPeakTask>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {ROW_COLUMNS} FROM off_peak_tasks
                 WHERE status NOT IN ({TERMINAL_SQL_LIST})
                 ORDER BY queued_at ASC"
            ))?;
            let rows = statement
                .query_map([], OffPeakRow::read)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.iter().map(|row| row_to_task(row, None)).collect())
        })
    }

    // ── 终态核销 outbox ──────────────────────────────────────────────────

    pub fn mark_settled(&self, off_peak_task_id: &str, settled_at: i64) -> Result<()> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            connection.execute(
                &format!(
                    "UPDATE off_peak_tasks
                     SET settled_at = ?, updated_at = ?
                     WHERE off_peak_task_id = ? AND status IN ({TERMINAL_SQL_LIST})"
                ),
                params![settled_at, settled_at, off_peak_task_id],
            )?;
            Ok(())
        })
    }

    pub fn list_unsettled_terminal(&self) -> Result<Vec<OffPeakTask>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {ROW_COLUMNS} FROM off_peak_tasks
                 WHERE status IN ({TERMINAL_SQL_LIST}) AND settled_at IS NULL
                 ORDER BY ended_at ASC"
            ))?;
            let rows = statement
                .query_map([], OffPeakRow::read)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.iter().map(|row| row_to_task(row, None)).collect())
        })
    }
}

