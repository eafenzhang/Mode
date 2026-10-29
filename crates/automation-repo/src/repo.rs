//! 定时任务仓库：`automations` 定义与调度状态、`automation_runs` 运行历史。
//!
//! 行为真源：`packages/services/src/session/automationRepo.ts`。规范见
//! `docs/specs/automation-repo.md`。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use model_selection::{parse_model_selection, serialize_model_selection, ModelSelection};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::{AutomationRepoError, Result};

/// 单个本地任务索引允许保留的定时任务定义总数（所有生命周期状态都计入）。
pub const AUTOMATION_CREATE_LIMIT: i64 = 20;
pub const AUTOMATION_CREATE_LIMIT_ERROR_CODE: &str = "AUTOMATION_CREATE_LIMIT_REACHED";
/// 派发失败退避常量。
pub const DISPATCH_RETRY_BASE_MS: i64 = 30_000;
pub const DISPATCH_RETRY_CAP_MS: i64 = 15 * 60_000;
pub const DISPATCH_MAX_ATTEMPTS: i64 = 5;
/// 认领超时回收：`running=1` 超过该时长仍未结算，视为持有者已崩溃。
pub const CLAIM_STALE_MS: i64 = 10 * 60_000;

const AUTOMATION_COLUMNS: &str = "automation_id, title, cron_expr, prompt, model, provider, mode, \
thought_level, model_selection, workspace_key, workspace_path, workspace_identity, \
target_task_id, bot_delivery_target, location_kind, recurring, max_runs, end_at, schedule_rule, \
schedule_edited_by_user, run_count, scheduled_run_count, enabled, lifecycle_status, next_run_at, \
last_run_at, running, claimed_at, dispatch_status, dispatch_attempts, retry_at, last_error, \
created_at, updated_at";

const RUN_COLUMNS: &str = "run_id, automation_id, workspace_key, scheduled_at, trigger, \
model_selection, dispatch_status, outcome, session_id, error, attempts, created_at, updated_at";

const VALID_MODES: [&str; 6] = ["yolo", "plan", "edit", "auto", "autoEdit", "build"];

const VALID_BOT_PROVIDERS: [&str; 6] = ["feishu", "lark", "weixin", "telegram", "wecom", "dingtalk"];

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
        .expect("automation registry poisoned")
        .contains(path)
}

/// 退避重试时间：`now + min(BASE * 2^(attempts-1), CAP)`。
pub fn compute_retry_at(now: i64, attempts: i64) -> i64 {
    let exponent = attempts.saturating_sub(1).max(0);
    let multiplier = 1_i64
        .checked_shl(u32::try_from(exponent).unwrap_or(u32::MAX))
        .unwrap_or(i64::MAX);
    let backoff = DISPATCH_RETRY_BASE_MS
        .saturating_mul(multiplier)
        .min(DISPATCH_RETRY_CAP_MS);
    now.saturating_add(backoff)
}

fn normalize_mode(mode: Option<&str>) -> Option<String> {
    mode.filter(|value| VALID_MODES.contains(value))
        .map(str::to_owned)
}

fn assert_valid_mode(mode: Option<&str>) -> Result<()> {
    if let Some(mode) = mode {
        if !VALID_MODES.contains(&mode) {
            return Err(AutomationRepoError::Message(format!(
                "Invalid automation mode: {mode}"
            )));
        }
    }
    Ok(())
}

// ── 公开类型 ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationBotDeliveryTarget {
    pub provider: String,
    pub bot_id: String,
    pub provider_user_id: String,
    pub chat_type: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    pub automation_id: String,
    pub title: String,
    pub cron_expr: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<ModelSelection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    pub workspace_key: String,
    pub workspace_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_task_id: Option<String>,
    pub location_kind: String,
    pub recurring: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_runs: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_rule: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule_edited_by_user: Option<bool>,
    pub run_count: i64,
    pub enabled: bool,
    pub lifecycle_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<i64>,
    pub dispatch_status: String,
    pub dispatch_attempts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRun {
    pub run_id: String,
    pub automation_id: String,
    pub workspace_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<i64>,
    pub trigger: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<ModelSelection>,
    pub dispatch_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub attempts: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClaimedManualAutomationRun {
    pub automation: Automation,
    pub run: AutomationRun,
}

#[derive(Clone, Debug, Default)]
pub struct AutomationCreateParams {
    pub title: String,
    pub cron_expr: String,
    pub prompt: String,
    pub model_selection: Option<ModelSelection>,
    pub mode: Option<String>,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub target_task_id: Option<String>,
    pub bot_delivery_target: Option<AutomationBotDeliveryTarget>,
    pub recurring: bool,
    pub max_runs: Option<i64>,
    pub end_at: Option<i64>,
    pub schedule_rule: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct AutomationCreateOptions {
    pub next_run_at: Option<i64>,
    pub lifecycle_status: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct AutomationUpdateParams {
    pub title: Option<String>,
    pub cron_expr: Option<String>,
    pub prompt: Option<String>,
    /// `None` = 不修改；`Some(None)` = 清空回退 Workspace 首选。
    pub model_selection: Option<Option<ModelSelection>>,
    /// `None` = 不修改；`Some(None)` = 清空回退 workspace 默认。
    pub mode: Option<Option<String>>,
    pub recurring: Option<bool>,
    pub max_runs: Option<Option<i64>>,
    pub end_at: Option<Option<i64>>,
    pub schedule_rule: Option<Option<Value>>,
    pub schedule_edited_by_user: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct AutomationUpdateOptions {
    /// `None` = 不修改；`Some(v)` = 写入（可为 null）。
    pub next_run_at: Option<Option<i64>>,
    pub lifecycle_status: Option<String>,
    pub reset_retry: bool,
}

#[derive(Clone, Debug, Default)]
pub struct AutomationScope {
    pub workspace_path: Option<String>,
    pub workspace_identity: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AutomationRestartOptions {
    pub next_run_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct RunNowOptions {
    pub now: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchFailureKind {
    Transient,
    Permanent,
}

#[derive(Clone, Debug)]
pub struct MarkDispatchedOptions {
    pub dispatched_at: i64,
    pub next_run_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct MarkDispatchFailedOptions {
    pub failed_at: i64,
    pub error: String,
    pub kind: DispatchFailureKind,
    pub next_run_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct SkipAndRescheduleParams {
    pub automation_id: String,
    pub run_id: String,
    pub workspace_key: String,
    pub scheduled_at: Option<i64>,
    pub reason: String,
    pub next_run_at: Option<i64>,
    pub finalize: bool,
}

#[derive(Clone, Debug)]
pub struct EnsureRunClaimedParams {
    pub run_id: String,
    pub automation_id: String,
    pub workspace_key: String,
    pub scheduled_at: Option<i64>,
    pub trigger: String,
}

#[derive(Clone, Debug)]
pub struct RecordSkippedRunParams {
    pub run_id: String,
    pub automation_id: String,
    pub workspace_key: String,
    pub scheduled_at: Option<i64>,
    pub trigger: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct UpsertRunClaimedParams {
    pub run_id: String,
    pub automation_id: String,
    pub workspace_key: String,
    pub scheduled_at: Option<i64>,
    pub trigger: String,
    pub model_selection: Option<ModelSelection>,
}

#[derive(Clone, Debug)]
pub struct MarkRunDispatchParams {
    pub run_id: String,
    pub dispatch_status: String,
    pub session_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MarkManualRunDispatchedParams {
    pub run_id: String,
    pub session_id: Option<String>,
    pub dispatched_at: i64,
}

// ── 行结构 ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct AutomationRow {
    automation_id: String,
    title: String,
    cron_expr: String,
    prompt: String,
    mode: Option<String>,
    model_selection: Option<String>,
    workspace_key: String,
    workspace_path: String,
    workspace_identity: Option<String>,
    target_task_id: Option<String>,
    bot_delivery_target: Option<String>,
    location_kind: String,
    recurring: i64,
    max_runs: Option<i64>,
    end_at: Option<i64>,
    schedule_rule: Option<String>,
    schedule_edited_by_user: i64,
    run_count: i64,
    scheduled_run_count: i64,
    enabled: i64,
    lifecycle_status: String,
    next_run_at: Option<i64>,
    last_run_at: Option<i64>,
    running: i64,
    claimed_at: Option<i64>,
    dispatch_status: String,
    dispatch_attempts: i64,
    retry_at: Option<i64>,
    last_error: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl AutomationRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            automation_id: row.get(0)?,
            title: row.get(1)?,
            cron_expr: row.get(2)?,
            prompt: row.get(3)?,
            // 4 model / 5 provider / 7 thought_level 只供回滚保留，读取不参与输出。
            mode: row.get(6)?,
            model_selection: row.get(8)?,
            workspace_key: row.get(9)?,
            workspace_path: row.get(10)?,
            workspace_identity: row.get(11)?,
            target_task_id: row.get(12)?,
            bot_delivery_target: row.get(13)?,
            location_kind: row.get(14)?,
            recurring: row.get(15)?,
            max_runs: row.get(16)?,
            end_at: row.get(17)?,
            schedule_rule: row.get(18)?,
            schedule_edited_by_user: row.get(19)?,
            run_count: row.get(20)?,
            scheduled_run_count: row.get(21)?,
            enabled: row.get(22)?,
            lifecycle_status: row.get(23)?,
            next_run_at: row.get(24)?,
            last_run_at: row.get(25)?,
            running: row.get(26)?,
            claimed_at: row.get(27)?,
            dispatch_status: row.get(28)?,
            dispatch_attempts: row.get(29)?,
            retry_at: row.get(30)?,
            last_error: row.get(31)?,
            created_at: row.get(32)?,
            updated_at: row.get(33)?,
        })
    }
}

#[derive(Clone, Debug)]
struct AutomationRunRow {
    run_id: String,
    automation_id: String,
    workspace_key: String,
    scheduled_at: Option<i64>,
    trigger: String,
    model_selection: Option<String>,
    dispatch_status: String,
    outcome: Option<String>,
    session_id: Option<String>,
    error: Option<String>,
    attempts: i64,
    created_at: i64,
    updated_at: i64,
}

impl AutomationRunRow {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            run_id: row.get(0)?,
            automation_id: row.get(1)?,
            workspace_key: row.get(2)?,
            scheduled_at: row.get(3)?,
            trigger: row.get(4)?,
            model_selection: row.get(5)?,
            dispatch_status: row.get(6)?,
            outcome: row.get(7)?,
            session_id: row.get(8)?,
            error: row.get(9)?,
            attempts: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    }
}

fn read_serialized_model_selection(value: Option<&str>) -> Option<ModelSelection> {
    value.and_then(parse_model_selection)
}

fn row_to_automation(row: &AutomationRow) -> Automation {
    Automation {
        automation_id: row.automation_id.clone(),
        title: row.title.clone(),
        cron_expr: row.cron_expr.clone(),
        prompt: row.prompt.clone(),
        model_selection: read_serialized_model_selection(row.model_selection.as_deref()),
        mode: normalize_mode(row.mode.as_deref()),
        workspace_key: row.workspace_key.clone(),
        workspace_path: row.workspace_path.clone(),
        workspace_identity: row.workspace_identity.clone(),
        target_task_id: row.target_task_id.clone(),
        location_kind: if row.location_kind == "remote" {
            "remote".to_owned()
        } else {
            "local".to_owned()
        },
        recurring: row.recurring == 1,
        max_runs: row.max_runs,
        end_at: row.end_at,
        schedule_rule: row
            .schedule_rule
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok()),
        schedule_edited_by_user: (row.schedule_edited_by_user == 1).then_some(true),
        run_count: row.run_count,
        enabled: row.enabled == 1,
        lifecycle_status: row.lifecycle_status.clone(),
        next_run_at: row.next_run_at,
        last_run_at: row.last_run_at,
        dispatch_status: row.dispatch_status.clone(),
        dispatch_attempts: row.dispatch_attempts,
        retry_at: row.retry_at,
        last_error: row.last_error.clone(),
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn row_to_run(row: &AutomationRunRow) -> AutomationRun {
    AutomationRun {
        run_id: row.run_id.clone(),
        automation_id: row.automation_id.clone(),
        workspace_key: row.workspace_key.clone(),
        scheduled_at: row.scheduled_at,
        trigger: row.trigger.clone(),
        model_selection: read_serialized_model_selection(row.model_selection.as_deref()),
        dispatch_status: row.dispatch_status.clone(),
        outcome: row.outcome.clone(),
        session_id: row.session_id.clone(),
        error: row.error.clone(),
        attempts: row.attempts,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn parse_bot_delivery_target(raw: &str) -> Option<AutomationBotDeliveryTarget> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    if object.len() != 4
        || object
            .keys()
            .any(|key| !matches!(key.as_str(), "provider" | "botId" | "providerUserId" | "chatType"))
    {
        return None;
    }
    let provider = object.get("provider")?.as_str()?;
    if !VALID_BOT_PROVIDERS.contains(&provider) {
        return None;
    }
    let chat_type = object.get("chatType")?.as_str()?;
    if chat_type != "private" && chat_type != "group" {
        return None;
    }
    let bot_id = object.get("botId")?.as_str()?.trim();
    let provider_user_id = object.get("providerUserId")?.as_str()?.trim();
    if bot_id.is_empty() || provider_user_id.is_empty() {
        return None;
    }
    Some(AutomationBotDeliveryTarget {
        provider: provider.to_owned(),
        bot_id: bot_id.to_owned(),
        provider_user_id: provider_user_id.to_owned(),
        chat_type: chat_type.to_owned(),
    })
}

fn serialize_selection(selection: &ModelSelection) -> Result<String> {
    serialize_model_selection(selection).map_err(AutomationRepoError::Message)
}

struct State {
    db: Option<Connection>,
    db_path: Option<PathBuf>,
}

pub struct AutomationRepo {
    resolved_db_path: Option<PathBuf>,
    startup_busy_timeout_ms: u64,
    state: Mutex<State>,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl AutomationRepo {
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
            AutomationRepoError::Message("AutomationRepo 需要显式 db 路径".to_owned())
        })
    }

    /// 打开库并完成一次性准备（迁移）。
    pub fn ensure_ready(&self) -> Result<()> {
        let path = self.resolve_db_path()?;
        let mut state = self.state.lock().expect("automation state poisoned");
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
        prepared_registry()
            .lock()
            .expect("automation registry poisoned")
            .insert(path);
        Ok(())
    }

    pub fn close(&self) {
        let mut state = self.state.lock().expect("automation state poisoned");
        if let Some(connection) = state.db.take() {
            let _ = connection.close();
        }
        state.db_path = None;
    }

    fn with_conn<T>(&self, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let state = self.state.lock().expect("automation state poisoned");
        let connection = state
            .db
            .as_ref()
            .ok_or(AutomationRepoError::NotInitialized)?;
        operation(connection)
    }

    fn get_row(
        &self,
        connection: &Connection,
        automation_id: &str,
        workspace_key: Option<&str>,
    ) -> Result<Option<AutomationRow>> {
        Ok(connection
            .query_row(
                &format!(
                    "SELECT {AUTOMATION_COLUMNS} FROM automations
                     WHERE automation_id = ?1 AND (?2 IS NULL OR workspace_key = ?2)"
                ),
                params![automation_id, workspace_key],
                AutomationRow::read,
            )
            .optional()?)
    }

    fn require_row(&self, connection: &Connection, automation_id: &str) -> Result<AutomationRow> {
        self.get_row(connection, automation_id, None)?.ok_or_else(|| {
            AutomationRepoError::Message(format!("Automation 不存在: {automation_id}"))
        })
    }

    // ── 管理 CRUD ────────────────────────────────────────────────────────

    pub fn create(
        &self,
        params: &AutomationCreateParams,
        options: &AutomationCreateOptions,
    ) -> Result<Automation> {
        assert_valid_mode(params.mode.as_deref())?;
        self.ensure_ready()?;
        let now = self.now();
        let automation_id = format!("automation-{}", Uuid::new_v4());
        let workspace_key = workspace_identity::workspace_key(
            &params.workspace_path,
            params.workspace_identity.as_deref(),
        );
        let model_selection = match &params.model_selection {
            Some(selection) => serialize_selection(selection)?,
            None => "null".to_owned(),
        };
        let bot_delivery_target = match &params.bot_delivery_target {
            Some(target) => Some(
                serde_json::to_string(target)?,
            ),
            None => None,
        };
        let schedule_rule = match &params.schedule_rule {
            Some(value) => Some(serde_json::to_string(value)?),
            None => None,
        };
        let lifecycle_status = options
            .lifecycle_status
            .clone()
            .unwrap_or_else(|| "active".to_owned());
        self.with_conn(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let outcome = (|| -> Result<()> {
                let count: i64 =
                    connection.query_row("SELECT COUNT(*) FROM automations", [], |row| row.get(0))?;
                if count >= AUTOMATION_CREATE_LIMIT {
                    return Err(AutomationRepoError::Message(format!(
                        "[{AUTOMATION_CREATE_LIMIT_ERROR_CODE}] At most {AUTOMATION_CREATE_LIMIT} automations may be retained. Delete an existing automation before creating another."
                    )));
                }
                connection.execute(
                    "INSERT INTO automations (
                       automation_id, title, cron_expr, prompt, model, provider, model_selection,
                       workspace_key, workspace_path, workspace_identity, target_task_id,
                       bot_delivery_target, location_kind,
                       recurring, max_runs, end_at, schedule_rule, schedule_edited_by_user,
                       run_count, enabled, lifecycle_status,
                       next_run_at, last_run_at, running, claimed_at,
                       dispatch_status, dispatch_attempts, retry_at, last_error,
                       mode, thought_level,
                       created_at, updated_at
                     ) VALUES (
                       ?, ?, ?, ?, NULL, NULL, ?,
                       ?, ?, ?, ?,
                       ?, 'local',
                       ?, ?, ?, ?, 0,
                       0, ?, ?,
                       ?, NULL, 0, NULL,
                       'idle', 0, NULL, NULL,
                       ?, NULL,
                       ?, ?
                     )",
                    params![
                        automation_id,
                        params.title,
                        params.cron_expr,
                        params.prompt,
                        model_selection,
                        workspace_key,
                        params.workspace_path,
                        params.workspace_identity,
                        params.target_task_id,
                        bot_delivery_target,
                        if params.recurring { 1 } else { 0 },
                        params.max_runs,
                        params.end_at,
                        schedule_rule,
                        if lifecycle_status == "completed" { 0 } else { 1 },
                        lifecycle_status,
                        options.next_run_at,
                        params.mode,
                        now,
                        now
                    ],
                )?;
                Ok(())
            })();
            match outcome {
                Ok(()) => {
                    connection.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(error) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        self.with_conn(|connection| Ok(row_to_automation(&self.require_row(connection, &automation_id)?)))
    }

    pub fn list(&self, scope: &AutomationScope) -> Result<Vec<Automation>> {
        self.ensure_ready()?;
        let workspace_key = scope.workspace_path.as_deref().map(|path| {
            workspace_identity::workspace_key(path, scope.workspace_identity.as_deref())
        });
        self.with_conn(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {AUTOMATION_COLUMNS} FROM automations
                 WHERE (?1 IS NULL OR workspace_key = ?1)
                 ORDER BY created_at DESC"
            ))?;
            let rows = statement
                .query_map(params![workspace_key], AutomationRow::read)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.iter().map(row_to_automation).collect())
        })
    }

    pub fn get_model_selection_for_dispatch(
        &self,
        automation_id: &str,
        workspace_key: &str,
    ) -> Result<Option<ModelSelection>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let Some(row) = self.get_row(connection, automation_id, Some(workspace_key))? else {
                return Err(AutomationRepoError::Message(
                    "Automation 不存在或不属于当前工作区".to_owned(),
                ));
            };
            if let Some(selection) = read_serialized_model_selection(row.model_selection.as_deref())
            {
                return Ok(Some(selection));
            }
            // 迁移已把旧默认写为 JSON null。SQL NULL 是缺配置，不能借旧列决定执行默认值。
            let follows_workspace = row.model_selection.as_deref() == Some("null");
            if !follows_workspace {
                return Err(AutomationRepoError::Message(
                    "Automation 模型选择不可用，请重新选择模型与思考档位".to_owned(),
                ));
            }
            Ok(None)
        })
    }

    pub fn get_bot_delivery_target(
        &self,
        automation_id: &str,
        workspace_key: Option<&str>,
    ) -> Result<Option<AutomationBotDeliveryTarget>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let Some(row) = self.get_row(connection, automation_id, workspace_key)? else {
                return Ok(None);
            };
            Ok(row
                .bot_delivery_target
                .as_deref()
                .and_then(parse_bot_delivery_target))
        })
    }

    pub fn has_task_binding(
        &self,
        workspace_path: &str,
        workspace_identity: Option<&str>,
        target_task_id: &str,
    ) -> Result<bool> {
        self.ensure_ready()?;
        let workspace_key = workspace_identity::workspace_key(workspace_path, workspace_identity);
        self.with_conn(|connection| {
            let hit: Option<i64> = connection
                .query_row(
                    "SELECT 1 FROM automations
                     WHERE workspace_key = ? AND target_task_id = ?
                     LIMIT 1",
                    params![workspace_key, target_task_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(hit.is_some())
        })
    }

    pub fn get(
        &self,
        automation_id: &str,
        workspace_key: Option<&str>,
    ) -> Result<Option<Automation>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            Ok(self
                .get_row(connection, automation_id, workspace_key)?
                .map(|row| row_to_automation(&row)))
        })
    }

    pub fn get_scheduled_run_count(
        &self,
        automation_id: &str,
        workspace_key: Option<&str>,
    ) -> Result<Option<i64>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            Ok(self
                .get_row(connection, automation_id, workspace_key)?
                .map(|row| row.scheduled_run_count))
        })
    }

    pub fn update(
        &self,
        automation_id: &str,
        params: &AutomationUpdateParams,
        options: &AutomationUpdateOptions,
        workspace_key: Option<&str>,
    ) -> Result<Option<Automation>> {
        if let Some(Some(mode)) = params.mode.as_ref() {
            assert_valid_mode(Some(mode))?;
        }
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            let Some(existing) = self.get_row(connection, automation_id, workspace_key)? else {
                return Ok(None);
            };
            let model_selection = match &params.model_selection {
                None => existing.model_selection.clone(),
                Some(Some(selection)) => Some(serialize_selection(selection)?),
                Some(None) => Some("null".to_owned()),
            };
            let mode = match &params.mode {
                None => existing.mode.clone(),
                Some(value) => value.clone(),
            };
            let recurring = params
                .recurring
                .map(|value| if value { 1 } else { 0 })
                .unwrap_or(existing.recurring);
            let max_runs = match params.max_runs {
                None => existing.max_runs,
                Some(value) => value,
            };
            let end_at = match params.end_at {
                None => existing.end_at,
                Some(value) => value,
            };
            let schedule_rule = match &params.schedule_rule {
                None => existing.schedule_rule.clone(),
                Some(None) => None,
                Some(Some(value)) => Some(serde_json::to_string(value)?),
            };
            let schedule_edited_by_user = params
                .schedule_edited_by_user
                .map(|value| if value { 1 } else { 0 })
                .unwrap_or(existing.schedule_edited_by_user);
            let next_run_at = match options.next_run_at {
                None => existing.next_run_at,
                Some(value) => value,
            };
            let lifecycle_status = options
                .lifecycle_status
                .clone()
                .unwrap_or_else(|| existing.lifecycle_status.clone());
            let enabled = match &options.lifecycle_status {
                Some(status) => {
                    if status == "active" {
                        1
                    } else {
                        0
                    }
                }
                None => existing.enabled,
            };
            connection.execute(
                "UPDATE automations SET
                   title = ?, cron_expr = ?, prompt = ?,
                   model_selection = ?,
                   mode = ?,
                   recurring = ?, max_runs = ?, end_at = ?,
                   schedule_rule = ?,
                   schedule_edited_by_user = ?,
                   next_run_at = ?, lifecycle_status = ?,
                   dispatch_attempts = ?, retry_at = ?, dispatch_status = ?,
                   enabled = ?, updated_at = ?
                 WHERE automation_id = ?",
                params![
                    params.title.as_deref().unwrap_or(&existing.title),
                    params.cron_expr.as_deref().unwrap_or(&existing.cron_expr),
                    params.prompt.as_deref().unwrap_or(&existing.prompt),
                    model_selection,
                    mode,
                    recurring,
                    max_runs,
                    end_at,
                    schedule_rule,
                    schedule_edited_by_user,
                    next_run_at,
                    lifecycle_status,
                    if options.reset_retry {
                        0
                    } else {
                        existing.dispatch_attempts
                    },
                    if options.reset_retry {
                        None
                    } else {
                        existing.retry_at
                    },
                    if options.reset_retry {
                        "idle".to_owned()
                    } else {
                        existing.dispatch_status.clone()
                    },
                    enabled,
                    now,
                    automation_id
                ],
            )?;
            Ok(Some(row_to_automation(&self.require_row(connection, automation_id)?)))
        })
    }

    pub fn delete(&self, automation_id: &str, workspace_key: Option<&str>) -> Result<bool> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let changes = connection.execute(
                "DELETE FROM automations
                 WHERE automation_id = ?1 AND (?2 IS NULL OR workspace_key = ?2)",
                params![automation_id, workspace_key],
            )?;
            Ok(changes > 0)
        })
    }

    pub fn set_enabled(
        &self,
        automation_id: &str,
        enabled: bool,
        workspace_key: Option<&str>,
    ) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            connection.execute(
                // 匿名 ? 与编号 ?N 混用时 ?N 指向序号 N 的绑定参数，会与前面的匿名位错位，必须全部编号。
                "UPDATE automations
                 SET enabled = ?1, lifecycle_status = ?2, updated_at = ?3
                 WHERE automation_id = ?4 AND (?5 IS NULL OR workspace_key = ?5)",
                params![
                    if enabled { 1 } else { 0 },
                    if enabled { "active" } else { "paused" },
                    now,
                    automation_id,
                    workspace_key
                ],
            )?;
            Ok(())
        })
    }

    pub fn restart(
        &self,
        automation_id: &str,
        options: &AutomationRestartOptions,
        workspace_key: Option<&str>,
    ) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            connection.execute(
                "UPDATE automations
                 SET lifecycle_status = 'active',
                     enabled = 1,
                     run_count = 0,
                     scheduled_run_count = 0,
                     dispatch_attempts = 0,
                     retry_at = NULL,
                     dispatch_status = 'idle',
                     running = 0,
                     claimed_at = NULL,
                     next_run_at = ?1,
                     last_error = NULL,
                     updated_at = ?2
                 WHERE automation_id = ?3 AND (?4 IS NULL OR workspace_key = ?4)",
                params![options.next_run_at, now, automation_id, workspace_key],
            )?;
            Ok(())
        })
    }

    /// 立即运行：写入由当前 host 直接持有的 manual run，不修改 cron 计划与生命周期。
    pub fn run_now(
        &self,
        automation_id: &str,
        options: &RunNowOptions,
        workspace_key: Option<&str>,
    ) -> Result<Option<ClaimedManualAutomationRun>> {
        self.ensure_ready()?;
        let now = options.now;
        self.with_conn(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let outcome = (|| -> Result<Option<ClaimedManualAutomationRun>> {
                connection.execute(
                    "UPDATE automations
                     SET running = 0, claimed_at = NULL
                     WHERE running = 1 AND claimed_at IS NOT NULL AND claimed_at <= ?",
                    params![now - CLAIM_STALE_MS],
                )?;
                let Some(row) = self.get_row(connection, automation_id, workspace_key)? else {
                    return Ok(None);
                };
                let claimed = connection.execute(
                    "UPDATE automations
                     SET running = 1, claimed_at = ?1, updated_at = ?2
                     WHERE automation_id = ?3 AND running = 0
                       AND (?4 IS NULL OR workspace_key = ?4)",
                    params![now, now, automation_id, workspace_key],
                )?;
                if claimed != 1 {
                    return Ok(None);
                }
                let run_id = format!("{automation_id}:manual:{}", Uuid::new_v4());
                connection.execute(
                    "INSERT INTO automation_runs (
                       run_id, automation_id, workspace_key, scheduled_at, trigger,
                       model_selection, dispatch_status, attempts, created_at, updated_at
                     ) VALUES (?, ?, ?, ?, 'manual', NULL, 'claimed', 1, ?, ?)",
                    params![run_id, automation_id, row.workspace_key, now, now, now],
                )?;
                let mut claimed_row = row.clone();
                claimed_row.running = 1;
                claimed_row.claimed_at = Some(now);
                claimed_row.updated_at = now;
                let run_row = AutomationRunRow {
                    run_id: run_id.clone(),
                    automation_id: automation_id.to_owned(),
                    workspace_key: row.workspace_key.clone(),
                    scheduled_at: Some(now),
                    trigger: "manual".to_owned(),
                    model_selection: None,
                    dispatch_status: "claimed".to_owned(),
                    outcome: None,
                    session_id: None,
                    error: None,
                    attempts: 1,
                    created_at: now,
                    updated_at: now,
                };
                Ok(Some(ClaimedManualAutomationRun {
                    automation: row_to_automation(&claimed_row),
                    run: row_to_run(&run_row),
                }))
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

    // ── 调度状态机 ────────────────────────────────────────────────────────

    pub fn claim_due(&self, now: i64) -> Result<Vec<Automation>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let outcome = (|| -> Result<Vec<Automation>> {
                // 截止日期是计划边界；过期任务先转终态，避免继续被正常 cron 或 retry 认领。
                connection.execute(
                    "UPDATE automations
                     SET lifecycle_status = 'completed', enabled = 0, next_run_at = NULL,
                         retry_at = NULL, running = 0, claimed_at = NULL, updated_at = ?
                     WHERE enabled = 1 AND end_at IS NOT NULL AND end_at < ?",
                    params![now, now],
                )?;
                connection.execute(
                    "UPDATE automations
                     SET running = 0, claimed_at = NULL
                     WHERE running = 1 AND claimed_at IS NOT NULL AND claimed_at <= ?",
                    params![now - CLAIM_STALE_MS],
                )?;
                let due_rows = {
                    let mut statement = connection.prepare(&format!(
                        "SELECT {AUTOMATION_COLUMNS} FROM automations
                         WHERE enabled = 1 AND running = 0
                           AND (
                             (retry_at IS NOT NULL AND retry_at <= ?)
                             OR (retry_at IS NULL AND next_run_at IS NOT NULL AND next_run_at <= ?)
                           )"
                    ))?;
                    let rows = statement
                        .query_map(params![now, now], AutomationRow::read)?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                let mut claimed = Vec::new();
                for row in due_rows {
                    let changes = connection.execute(
                        "UPDATE automations
                         SET running = 1, claimed_at = ?, dispatch_status = 'claimed', updated_at = ?
                         WHERE automation_id = ? AND running = 0",
                        params![now, now, row.automation_id],
                    )?;
                    if changes == 1 {
                        let mut claimed_row = row.clone();
                        claimed_row.running = 1;
                        claimed_row.claimed_at = Some(now);
                        claimed_row.dispatch_status = "claimed".to_owned();
                        claimed.push(row_to_automation(&claimed_row));
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

    /// 认领 UI「立即运行」产生的 manual run。
    pub fn claim_manual_runs(&self, now: i64) -> Result<Vec<ClaimedManualAutomationRun>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let outcome = (|| -> Result<Vec<ClaimedManualAutomationRun>> {
                connection.execute(
                    "UPDATE automations
                     SET running = 0, claimed_at = NULL
                     WHERE running = 1 AND claimed_at IS NOT NULL AND claimed_at <= ?",
                    params![now - CLAIM_STALE_MS],
                )?;
                let rows = {
                    let mut statement = connection.prepare(&format!(
                        "SELECT {RUN_COLUMNS} FROM automation_runs
                         WHERE trigger = 'manual'
                           AND dispatch_status = 'claimed'
                           AND (attempts = 0 OR updated_at <= ?)
                         ORDER BY created_at ASC"
                    ))?;
                    let rows = statement
                        .query_map(params![now - CLAIM_STALE_MS], AutomationRunRow::read)?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                let mut claimed = Vec::new();
                for run_row in rows {
                    let changes = connection.execute(
                        "UPDATE automations
                         SET running = 1, claimed_at = ?, updated_at = ?
                         WHERE automation_id = ? AND running = 0",
                        params![now, now, run_row.automation_id],
                    )?;
                    if changes != 1 {
                        continue;
                    }
                    connection.execute(
                        "UPDATE automation_runs
                         SET attempts = attempts + 1, updated_at = ?
                         WHERE run_id = ? AND trigger = 'manual' AND dispatch_status = 'claimed'",
                        params![now, run_row.run_id],
                    )?;
                    let automation_row = self.require_row(connection, &run_row.automation_id)?;
                    let mut claimed_run = run_row.clone();
                    claimed_run.attempts += 1;
                    claimed_run.updated_at = now;
                    claimed.push(ClaimedManualAutomationRun {
                        automation: row_to_automation(&automation_row),
                        run: row_to_run(&claimed_run),
                    });
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

    pub fn mark_dispatched(
        &self,
        automation_id: &str,
        options: &MarkDispatchedOptions,
    ) -> Result<()> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let Some(row) = self.get_row(connection, automation_id, None)? else {
                return Ok(());
            };
            let run_count = row.run_count + 1;
            let scheduled_run_count = row.scheduled_run_count + 1;
            let reached_max =
                row.recurring == 0 && scheduled_run_count >= row.max_runs.unwrap_or(1);
            let next = options.next_run_at.unwrap_or(i64::MAX);
            let reached_end = row.end_at.is_some_and(|end_at| next > end_at);
            let finished = reached_max || reached_end;
            connection.execute(
                "UPDATE automations
                 SET run_count = ?,
                     scheduled_run_count = ?,
                     last_run_at = ?,
                     dispatch_status = 'dispatched',
                     dispatch_attempts = 0,
                     retry_at = NULL,
                     last_error = NULL,
                     running = 0,
                     claimed_at = NULL,
                     lifecycle_status = ?,
                     enabled = ?,
                     next_run_at = ?,
                     updated_at = ?
                 WHERE automation_id = ?",
                params![
                    run_count,
                    scheduled_run_count,
                    options.dispatched_at,
                    if finished { "completed" } else { "active" },
                    if finished { 0 } else { 1 },
                    if finished { None } else { options.next_run_at },
                    options.dispatched_at,
                    automation_id
                ],
            )?;
            Ok(())
        })
    }

    pub fn mark_dispatch_failed(
        &self,
        automation_id: &str,
        options: &MarkDispatchFailedOptions,
    ) -> Result<()> {
        self.ensure_ready()?;
        let now = options.failed_at;
        self.with_conn(|connection| {
            let Some(row) = self.get_row(connection, automation_id, None)? else {
                return Ok(());
            };
            if options.kind == DispatchFailureKind::Permanent {
                connection.execute(
                    "UPDATE automations
                     SET dispatch_status = 'failed_to_dispatch', lifecycle_status = 'failed',
                         enabled = 0, running = 0, claimed_at = NULL,
                         last_error = ?, updated_at = ?
                     WHERE automation_id = ?",
                    params![options.error, now, automation_id],
                )?;
                return Ok(());
            }
            let attempts = row.dispatch_attempts + 1;
            if attempts >= DISPATCH_MAX_ATTEMPTS {
                if row.recurring == 1 {
                    connection.execute(
                        "UPDATE automations
                         SET dispatch_status = 'idle', dispatch_attempts = 0, retry_at = NULL,
                             running = 0, claimed_at = NULL, next_run_at = ?,
                             last_error = ?, updated_at = ?
                         WHERE automation_id = ?",
                        params![options.next_run_at, options.error, now, automation_id],
                    )?;
                } else {
                    connection.execute(
                        "UPDATE automations
                         SET dispatch_status = 'failed_to_dispatch', lifecycle_status = 'failed',
                             enabled = 0, running = 0, claimed_at = NULL,
                             last_error = ?, updated_at = ?
                         WHERE automation_id = ?",
                        params![options.error, now, automation_id],
                    )?;
                }
                return Ok(());
            }
            connection.execute(
                "UPDATE automations
                 SET dispatch_status = 'failed_to_dispatch', dispatch_attempts = ?,
                     retry_at = ?, running = 0, claimed_at = NULL,
                     last_error = ?, updated_at = ?
                 WHERE automation_id = ?",
                params![
                    attempts,
                    compute_retry_at(now, attempts),
                    options.error,
                    now,
                    automation_id
                ],
            )?;
            Ok(())
        })
    }

    pub fn release_claim(&self, automation_id: &str) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            connection.execute(
                "UPDATE automations
                 SET running = 0, claimed_at = NULL, dispatch_status = 'idle', updated_at = ?
                 WHERE automation_id = ? AND running = 1",
                params![now, automation_id],
            )?;
            Ok(())
        })
    }

    pub fn release_manual_claim(
        &self,
        automation_id: &str,
        workspace_key: &str,
    ) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            connection.execute(
                "UPDATE automations
                 SET running = 0, claimed_at = NULL, updated_at = ?
                 WHERE automation_id = ? AND workspace_key = ? AND running = 1",
                params![now, automation_id, workspace_key],
            )?;
            Ok(())
        })
    }

    pub fn touch_manual_claim(&self, automation_id: &str, workspace_key: &str) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            connection.execute(
                "UPDATE automations
                 SET claimed_at = ?, updated_at = ?
                 WHERE automation_id = ? AND workspace_key = ? AND running = 1",
                params![now, now, automation_id, workspace_key],
            )?;
            Ok(())
        })
    }

    pub fn skip_and_reschedule(&self, params_in: &SkipAndRescheduleParams) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let outcome = (|| -> Result<()> {
                connection.execute(
                    "INSERT INTO automation_runs (
                       run_id, automation_id, workspace_key, scheduled_at, trigger,
                       dispatch_status, error, attempts, created_at, updated_at
                     ) VALUES (?, ?, ?, ?, 'schedule', 'skipped', ?, 0, ?, ?)
                     ON CONFLICT(run_id) DO UPDATE SET
                       dispatch_status = 'skipped', error = excluded.error,
                       updated_at = excluded.updated_at",
                    params![
                        params_in.run_id,
                        params_in.automation_id,
                        params_in.workspace_key,
                        params_in.scheduled_at,
                        params_in.reason,
                        now,
                        now
                    ],
                )?;
                if params_in.finalize {
                    connection.execute(
                        "UPDATE automations
                         SET lifecycle_status = 'completed', enabled = 0, next_run_at = NULL,
                             running = 0, claimed_at = NULL,
                             dispatch_status = 'idle', dispatch_attempts = 0, retry_at = NULL,
                             updated_at = ?
                         WHERE automation_id = ?",
                        params![now, params_in.automation_id],
                    )?;
                } else {
                    connection.execute(
                        "UPDATE automations
                         SET next_run_at = ?, running = 0, claimed_at = NULL,
                             dispatch_status = 'idle', dispatch_attempts = 0, retry_at = NULL,
                             updated_at = ?
                         WHERE automation_id = ?",
                        params![params_in.next_run_at, now, params_in.automation_id],
                    )?;
                }
                Ok(())
            })();
            match outcome {
                Ok(()) => {
                    connection.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(error) => {
                    let _ = connection.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    // ── 运行历史 ──────────────────────────────────────────────────────────

    pub fn ensure_run_claimed(&self, params_in: &EnsureRunClaimedParams) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            connection.execute(
                "INSERT INTO automation_runs (
                   run_id, automation_id, workspace_key, scheduled_at, trigger,
                   dispatch_status, attempts, created_at, updated_at
                 ) VALUES (?, ?, ?, ?, ?, 'claimed', 0, ?, ?)
                 ON CONFLICT(run_id) DO NOTHING",
                params![
                    params_in.run_id,
                    params_in.automation_id,
                    params_in.workspace_key,
                    params_in.scheduled_at,
                    params_in.trigger,
                    now,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn upsert_run_claimed(&self, params_in: &UpsertRunClaimedParams) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        let model_selection = match &params_in.model_selection {
            Some(selection) => Some(serialize_selection(selection)?),
            None => None,
        };
        self.with_conn(|connection| {
            connection.execute(
                "INSERT INTO automation_runs (
                   run_id, automation_id, workspace_key, scheduled_at, trigger,
                   model_selection, dispatch_status, attempts, created_at, updated_at
                 ) VALUES (?, ?, ?, ?, ?, ?, 'claimed', 0, ?, ?)
                 ON CONFLICT(run_id) DO UPDATE SET
                   dispatch_status = 'claimed',
                   model_selection = COALESCE(automation_runs.model_selection, excluded.model_selection),
                   outcome = NULL,
                   error = NULL,
                   attempts = attempts + 1,
                   updated_at = excluded.updated_at",
                params![
                    params_in.run_id,
                    params_in.automation_id,
                    params_in.workspace_key,
                    params_in.scheduled_at,
                    params_in.trigger,
                    model_selection,
                    now,
                    now
                ],
            )?;
            Ok(())
        })
    }

    /// 首次形成 Submission 时原子固定 run Selection；之后只能读回原值。
    pub fn fix_run_model_selection(
        &self,
        run_id: &str,
        selection: &ModelSelection,
    ) -> Result<ModelSelection> {
        self.ensure_ready()?;
        let now = self.now();
        let serialized = serialize_selection(selection)?;
        self.with_conn(|connection| {
            connection.execute(
                "UPDATE automation_runs
                 SET model_selection = COALESCE(model_selection, ?), updated_at = ?
                 WHERE run_id = ?",
                params![serialized, now, run_id],
            )?;
            let raw: Option<Option<String>> = connection
                .query_row(
                    "SELECT model_selection FROM automation_runs WHERE run_id = ?",
                    params![run_id],
                    |row| row.get(0),
                )
                .optional()?;
            raw.flatten()
                .as_deref()
                .and_then(parse_model_selection)
                .ok_or_else(|| {
                    AutomationRepoError::Message(format!(
                        "Automation run 不存在或无法固定模型选择: {run_id}"
                    ))
                })
        })
    }

    pub fn mark_run_dispatch(&self, params_in: &MarkRunDispatchParams) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            connection.execute(
                "UPDATE automation_runs
                 SET dispatch_status = ?,
                     session_id = COALESCE(?, session_id),
                     error = ?,
                     updated_at = ?
                 WHERE run_id = ?",
                params![
                    params_in.dispatch_status,
                    params_in.session_id,
                    params_in.error,
                    now,
                    params_in.run_id
                ],
            )?;
            Ok(())
        })
    }

    pub fn mark_manual_run_dispatched(
        &self,
        params_in: &MarkManualRunDispatchedParams,
    ) -> Result<bool> {
        self.ensure_ready()?;
        let now = params_in.dispatched_at;
        self.with_conn(|connection| {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            let outcome = (|| -> Result<bool> {
                let run: Option<(String, String, String)> = connection
                    .query_row(
                        "SELECT automation_id, workspace_key, dispatch_status
                         FROM automation_runs
                         WHERE run_id = ? AND trigger = 'manual'",
                        params![params_in.run_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
                let Some((automation_id, workspace_key, dispatch_status)) = run else {
                    return Ok(false);
                };
                if dispatch_status == "dispatched" {
                    return Ok(false);
                }
                connection.execute(
                    "UPDATE automation_runs
                     SET dispatch_status = 'dispatched',
                         session_id = COALESCE(?, session_id),
                         error = NULL,
                         updated_at = ?
                     WHERE run_id = ? AND trigger = 'manual' AND dispatch_status <> 'dispatched'",
                    params![params_in.session_id, now, params_in.run_id],
                )?;
                let changes = connection.execute(
                    "UPDATE automations
                     SET run_count = run_count + 1,
                         last_run_at = ?,
                         updated_at = ?
                     WHERE automation_id = ? AND workspace_key = ?",
                    params![now, now, automation_id, workspace_key],
                )?;
                Ok(changes > 0)
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

    pub fn mark_run_outcome(
        &self,
        run_id: &str,
        outcome: &str,
        error: Option<&str>,
    ) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            connection.execute(
                "UPDATE automation_runs
                 SET outcome = CASE
                       WHEN ? = 'running' AND outcome IS NOT NULL AND outcome <> 'running' THEN outcome
                       ELSE ?
                     END,
                     error = CASE
                       WHEN ? = 'running' AND outcome IS NOT NULL AND outcome <> 'running' THEN error
                       ELSE COALESCE(?, error)
                     END,
                     updated_at = ?
                 WHERE run_id = ?",
                params![outcome, outcome, outcome, error, now, run_id],
            )?;
            Ok(())
        })
    }

    pub fn record_skipped_run(&self, params_in: &RecordSkippedRunParams) -> Result<()> {
        self.ensure_ready()?;
        let now = self.now();
        self.with_conn(|connection| {
            connection.execute(
                "INSERT INTO automation_runs (
                   run_id, automation_id, workspace_key, scheduled_at, trigger,
                   dispatch_status, error, attempts, created_at, updated_at
                 ) VALUES (?, ?, ?, ?, ?, 'skipped', ?, 0, ?, ?)
                 ON CONFLICT(run_id) DO UPDATE SET
                   dispatch_status = 'skipped', error = excluded.error,
                   updated_at = excluded.updated_at",
                params![
                    params_in.run_id,
                    params_in.automation_id,
                    params_in.workspace_key,
                    params_in.scheduled_at,
                    params_in.trigger,
                    params_in.reason,
                    now,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_runs(
        &self,
        automation_id: &str,
        workspace_key: Option<&str>,
    ) -> Result<Vec<AutomationRun>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let mut statement = connection.prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM automation_runs
                 WHERE automation_id = ?1 AND (?2 IS NULL OR workspace_key = ?2)
                 ORDER BY created_at DESC"
            ))?;
            let rows = statement
                .query_map(params![automation_id, workspace_key], AutomationRunRow::read)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows.iter().map(row_to_run).collect())
        })
    }

    pub fn get_run(&self, run_id: &str) -> Result<Option<AutomationRun>> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            let row: Option<AutomationRunRow> = connection
                .query_row(
                    &format!("SELECT {RUN_COLUMNS} FROM automation_runs WHERE run_id = ?"),
                    params![run_id],
                    AutomationRunRow::read,
                )
                .optional()?;
            Ok(row.as_ref().map(row_to_run))
        })
    }

    pub fn delete_run(&self, run_id: &str, workspace_key: Option<&str>) -> Result<()> {
        self.ensure_ready()?;
        self.with_conn(|connection| {
            connection.execute(
                "DELETE FROM automation_runs
                 WHERE run_id = ?1 AND (?2 IS NULL OR workspace_key = ?2)",
                params![run_id, workspace_key],
            )?;
            Ok(())
        })
    }

    pub fn prune_runs(&self, max_age_ms: i64) -> Result<i64> {
        self.ensure_ready()?;
        let cutoff = self.now() - max_age_ms;
        self.with_conn(|connection| {
            let changes = connection.execute(
                "DELETE FROM automation_runs WHERE created_at < ?",
                params![cutoff],
            )?;
            Ok(changes as i64)
        })
    }
}
