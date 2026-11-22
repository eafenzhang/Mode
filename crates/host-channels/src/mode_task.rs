//! `mode-task` 频道适配器（LT-16.1 首切：任务列表读面 + 分组管理写面）。
//!
//! 真源：`packages/services/src/session/modeTaskService.ts` 的读面与分组方法
//! （TS `IModeTaskService`）。底层 `task-index-repo` 已整体迁移并与 TS 对拍
//! （query_task_list/grouped 视图/归档/分组管理）；本频道只做形状转换与分发。
//! 生命周期/命令队列/permission/elicitation/snapshot 族属编排面（16.2），不在
//! 本切片——未知方法显式报错，不做静默降级。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use agent_process::{AgentControlClient, AgentWorkspaceTarget};
use crate::task_command_queue::{CancelResult, EnqueueResult, QueueCommandParams, TaskCommandQueue};
use agent_protocol::session_write::SessionPersistence;
use agent_protocol::snapshot::SessionStateSnapshot;
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value as WireValue;
use task_index_repo::{
    ArchiveStaleTasksParams, GroupedTaskGroupRefs, GroupedTaskRef, GroupedTaskViewOrderInput,
    GroupedTaskViewTopLevelNodeRef, ListDeletedTaskIdsParams, ListTaskMetasParams, ModeTaskMeta,
    TaskIndexRepo, TaskIndexStatePatch, TaskListKind, TaskListQuery, TaskListSortBy,
    TaskListWorkspaceScope, TaskRef,
};

pub const MODE_TASK_CHANNEL: &str = "mode-task";

pub struct ModeTaskChannel {
    repo: Arc<TaskIndexRepo>,
    /// Agent 控制面（16.2a 编排方法需要）；`None` 时编排方法显式 unsupported
    /// （server-mode 无 agent 面，与 usage-stats 频道同降级语义）。
    control: Option<Arc<AgentControlClient>>,
    /// 运行时命令队列（16.2b）：replayable 手机端的 host command queue；
    /// `None` 时队列方法显式 unsupported。
    queue: Option<Arc<TaskCommandQueue>>,
    /// 事件面（16.2c）：任务列表变更与任务收口（readyOutcome）广播。
    events: Arc<TaskEventBus>,
    /// 数据根（`.mode`）：Claude 导入的文件复制与 legacy 快照落盘需要。
    data_root: Option<std::path::PathBuf>,
}

/// TS `ModeWorkspaceTaskListChanged`（`onDynamicWorkspaceEvent` 的 wire 载荷）。
#[derive(Clone, Debug)]
pub struct WorkspaceTaskListChanged {
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub task_id: Option<String>,
    /// TS `TaskRealtimeReason`（task_created/task_meta_changed/task_status_changed/
    /// task_deleted/task_title_changed/task_pinned/task_unpinned/task_archived/
    /// task_unarchived）或 auto_archive/realtime_sync。
    pub reason: &'static str,
    pub task_meta: Option<serde_json::Value>,
}

impl WorkspaceTaskListChanged {
    pub fn new(workspace_path: impl Into<String>, reason: &'static str) -> Self {
        Self {
            workspace_path: workspace_path.into(),
            workspace_identity: None,
            task_id: None,
            reason,
            task_meta: None,
        }
    }

    fn to_json(&self) -> serde_json::Value {
        let mut payload = serde_json::json!({
            "type": "workspace_task_list_changed",
            "workspacePath": self.workspace_path,
            "reason": self.reason,
        });
        if let Some(identity) = &self.workspace_identity {
            payload["workspaceIdentity"] = serde_json::json!(identity);
        }
        if let Some(task_id) = &self.task_id {
            payload["taskId"] = serde_json::json!(task_id);
        }
        if let Some(meta) = &self.task_meta {
            payload["taskMeta"] = meta.clone();
        }
        payload
    }
}

/// `onDynamicWorkspaceEvent(workspace)` 的订阅 scope；键 = `identity?.trim() || path`。
#[derive(Clone, Debug, Default)]
pub struct WorkspaceEventScope {
    pub workspace_path: Option<String>,
    pub workspace_identity: Option<String>,
}

impl WorkspaceEventScope {
    fn key(&self) -> Option<String> {
        self.workspace_path.as_deref().map(|path| workspace_key_of(path, self.workspace_identity.as_deref()))
    }
}

/// `activePromptInputIds` 的 key：`workspaceKey\0taskId`。
fn active_input_key(workspace_path: &str, workspace_identity: Option<&str>, task_id: &str) -> String {
    format!(
        "{}\u{0}{task_id}",
        workspace_key_of(workspace_path, workspace_identity)
    )
}

fn workspace_key_of(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    workspace_identity
        .map(str::trim)
        .filter(|identity| !identity.is_empty())
        .unwrap_or(workspace_path)
        .to_string()
}

#[derive(Clone)]
struct WorkspaceEventListener {
    /// `None` = 不过滤（进程内/测试订阅）。
    scope_key: Option<String>,
    emit: rpc_host::EventCallback,
}

#[derive(Clone)]
struct ReadyEventListener {
    /// `onDynamicTaskReady(taskId)` 的 task 过滤；`None` = 不过滤。
    task_id: Option<String>,
    emit: rpc_host::EventCallback,
}

type WorkspaceEventListeners = Arc<Mutex<HashMap<String, WorkspaceEventListener>>>;
type ReadyEventListeners = Arc<Mutex<HashMap<String, ReadyEventListener>>>;

/// 进程内任务事件监听者（不经 wire）：window-controller 等频道据此刷新投影。
pub type TaskEventLocalListener = Arc<dyn Fn(&WorkspaceTaskListChanged) + Send + Sync>;
pub type TaskEventLocalListeners = Arc<Mutex<HashMap<String, TaskEventLocalListener>>>;

pub const ON_DYNAMIC_WORKSPACE_EVENT: &str = "onDynamicWorkspaceEvent";
pub const ON_DYNAMIC_TASK_READY: &str = "onDynamicTaskReady";
pub const ON_DYNAMIC_TASK_TERMINAL_OUTCOME: &str = "onDynamicTaskTerminalOutcome";

#[derive(Clone)]
struct TerminalEventListener {
    /// `onDynamicTaskTerminalOutcome(taskId)` 的 task 过滤；`None` = 不过滤。
    task_id: Option<String>,
    emit: rpc_host::EventCallback,
}

type TerminalEventListeners = Arc<Mutex<HashMap<String, TerminalEventListener>>>;
/// `activePromptInputIds`：sendPrompt 记录的 inputId，终态事件用它收口对应 run。
type ActivePromptInputs = Arc<Mutex<HashMap<String, String>>>;

/// mode-task 事件总线（broadcast 频道的 subscribe/notify 模式；TS Emitter 等价）。
/// wire 监听者（renderer）与进程内监听者（同 server 的其他频道）共用同一通知点。
#[derive(Default, Clone)]
pub struct TaskEventBus {
    task_list_listeners: WorkspaceEventListeners,
    ready_listeners: ReadyEventListeners,
    terminal_listeners: TerminalEventListeners,
    local_listeners: TaskEventLocalListeners,
    active_inputs: ActivePromptInputs,
}

impl TaskEventBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// `onDynamicWorkspaceEvent`：按 scope 过滤投递（TS workspace emitter 等价）。
    pub fn subscribe_task_list(
        &self,
        scope: WorkspaceEventScope,
        emit: rpc_host::EventCallback,
    ) -> rpc_host::Subscription {
        let id = uuid::Uuid::new_v4().to_string();
        self.task_list_listeners
            .lock()
            .expect("task event listeners poisoned")
            .insert(
                id.clone(),
                WorkspaceEventListener {
                    scope_key: scope.key(),
                    emit,
                },
            );
        let map = Arc::clone(&self.task_list_listeners);
        Box::new(move || {
            map.lock()
                .expect("task event listeners poisoned")
                .remove(&id);
        })
    }

    /// `onDynamicTaskReady(taskId)`：按 task 过滤投递。
    pub fn subscribe_ready(
        &self,
        task_id: Option<String>,
        emit: rpc_host::EventCallback,
    ) -> rpc_host::Subscription {
        let id = uuid::Uuid::new_v4().to_string();
        self.ready_listeners
            .lock()
            .expect("task event listeners poisoned")
            .insert(id.clone(), ReadyEventListener { task_id, emit });
        let map = Arc::clone(&self.ready_listeners);
        Box::new(move || {
            map.lock()
                .expect("task event listeners poisoned")
                .remove(&id);
        })
    }

    /// `onDynamicTaskTerminalOutcome(taskId)`：按 task 过滤投递终态（automation/bots 结算）。
    pub fn subscribe_terminal(
        &self,
        task_id: Option<String>,
        emit: rpc_host::EventCallback,
    ) -> rpc_host::Subscription {
        let id = uuid::Uuid::new_v4().to_string();
        self.terminal_listeners
            .lock()
            .expect("task event listeners poisoned")
            .insert(id.clone(), TerminalEventListener { task_id, emit });
        let map = Arc::clone(&self.terminal_listeners);
        Box::new(move || {
            map.lock()
                .expect("task event listeners poisoned")
                .remove(&id);
        })
    }

    /// `sendPrompt` 记录本次输入轮次 id（TS `activePromptInputIds.set`）。
    pub fn record_active_input(
        &self,
        workspace_path: &str,
        workspace_identity: Option<&str>,
        task_id: &str,
        input_id: &str,
    ) {
        self.active_inputs
            .lock()
            .expect("task active inputs poisoned")
            .insert(
                active_input_key(workspace_path, workspace_identity, task_id),
                input_id.to_string(),
            );
    }

    /// 终态收口时取出并清除本次 inputId（TS 只保留最近一次输入）。
    pub fn take_active_input(
        &self,
        workspace_path: &str,
        workspace_identity: Option<&str>,
        task_id: &str,
    ) -> Option<String> {
        self.active_inputs
            .lock()
            .expect("task active inputs poisoned")
            .remove(&active_input_key(
                workspace_path,
                workspace_identity,
                task_id,
            ))
    }

    /// TS `onDynamicTaskTerminalOutcome`：phase 终态迁移 → `{taskId, inputId?, outcome}`。
    pub fn notify_terminal(&self, task_id: &str, input_id: Option<&str>, outcome: &str) {
        let mut payload = serde_json::json!({ "taskId": task_id, "outcome": outcome });
        if let Some(input_id) = input_id {
            payload["inputId"] = serde_json::json!(input_id);
        }
        let Ok(text) = serde_json::to_string(&payload) else {
            return;
        };
        let listeners: Vec<TerminalEventListener> = self
            .terminal_listeners
            .lock()
            .expect("task event listeners poisoned")
            .values()
            .cloned()
            .collect();
        for listener in listeners {
            if listener
                .task_id
                .as_deref()
                .is_some_and(|filter| filter != task_id)
            {
                continue;
            }
            (listener.emit)(WireValue::Json(text.clone()));
        }
    }

    /// 进程内订阅 workspace 任务列表变更（TS Host 进程内 Emitter 等价）。
    pub fn subscribe_local(&self, listener: TaskEventLocalListener) -> rpc_host::Subscription {
        let id = uuid::Uuid::new_v4().to_string();
        self.local_listeners
            .lock()
            .expect("task event local listeners poisoned")
            .insert(id.clone(), listener);
        let map = Arc::clone(&self.local_listeners);
        Box::new(move || {
            map.lock()
                .expect("task event local listeners poisoned")
                .remove(&id);
        })
    }

    fn notify_local(&self, event: &WorkspaceTaskListChanged) {
        let listeners: Vec<TaskEventLocalListener> = self
            .local_listeners
            .lock()
            .expect("task event local listeners poisoned")
            .values()
            .cloned()
            .collect();
        for listener in listeners {
            listener(event);
        }
    }

    /// TS `emitWorkspaceTaskListChanged`：按 workspace scope 广播 `workspace_task_list_changed`。
    pub fn notify_task_list(&self, event: &WorkspaceTaskListChanged) {
        let Ok(text) = serde_json::to_string(&event.to_json()) else {
            return;
        };
        let event_key = workspace_key_of(&event.workspace_path, event.workspace_identity.as_deref());
        let listeners: Vec<WorkspaceEventListener> = self
            .task_list_listeners
            .lock()
            .expect("task event listeners poisoned")
            .values()
            .cloned()
            .collect();
        for listener in listeners {
            if listener
                .scope_key
                .as_deref()
                .is_some_and(|scope_key| scope_key != event_key)
            {
                continue;
            }
            (listener.emit)(WireValue::Json(text.clone()));
        }
        self.notify_local(event);
    }

    /// TS `onDynamicTaskReady`：任务收口（prompt_completed/failed）。
    pub fn notify_ready(&self, task_id: &str, reason: &str) {
        let payload = serde_json::json!({ "taskId": task_id, "reason": reason });
        let Ok(text) = serde_json::to_string(&payload) else {
            return;
        };
        let listeners: Vec<ReadyEventListener> = self
            .ready_listeners
            .lock()
            .expect("task event listeners poisoned")
            .values()
            .cloned()
            .collect();
        for listener in listeners {
            if listener
                .task_id
                .as_deref()
                .is_some_and(|filter| filter != task_id)
            {
                continue;
            }
            (listener.emit)(WireValue::Json(text.clone()));
        }
    }
}

impl ModeTaskChannel {
    pub fn new(
        repo: Arc<TaskIndexRepo>,
        control: Option<Arc<AgentControlClient>>,
        queue: Option<Arc<TaskCommandQueue>>,
        events: Arc<TaskEventBus>,
    ) -> Self {
        Self {
            repo,
            control,
            queue,
            events,
            data_root: None,
        }
    }

    /// 注入数据根（Claude 导入路径）；未注入时 importClaudeSessions 显式报错。
    pub fn with_data_root(mut self, data_root: std::path::PathBuf) -> Self {
        self.data_root = Some(data_root);
        self
    }
}

/// TS `snapshotToMeta`（modeTaskServiceAdapter.ts:1280）的直替：会话快照 →
/// 任务索引 meta。经 JSON 组装复用 `parse_mode_task_meta` 的 schema 校验；
/// lastError.code 取 `code ?? type`（TS 同映射）；overlay 属 Node Host 会话级
/// 状态，16.2c 事件面再对齐。
fn snapshot_to_meta(
    snapshot: &SessionStateSnapshot,
) -> Result<task_index_repo::ModeTaskMeta, ChannelError> {
    use agent_protocol::session_read::SessionStatus;
    let session = &snapshot.session;
    let status = match session.status {
        SessionStatus::Running => "running",
        SessionStatus::Waiting => "waiting",
        SessionStatus::Paused => "paused",
        SessionStatus::Idle => "idle",
        SessionStatus::Completed => "completed",
        SessionStatus::Error => "error",
    };
    let mode_wire = match session.mode {
        agent_protocol::session_read::SessionMode::Plan => "plan",
        agent_protocol::session_read::SessionMode::Build => "build",
        agent_protocol::session_read::SessionMode::Edit => "edit",
        agent_protocol::session_read::SessionMode::Yolo => "yolo",
        agent_protocol::session_read::SessionMode::Auto => "auto",
    };
    let mut value = serde_json::json!({
        "taskId": session.session_id,
        "traceId": session
            .trace_id
            .clone()
            .unwrap_or_else(|| format!("mode-{}", session.session_id)),
        "title": session.title,
        "workspacePath": session.workspace.workspace_path,
        "createdAt": session.created_at as i64,
        "updatedAt": session.updated_at as i64,
        "mode": mode_wire,
        "provider": task_index_repo::MODE_AGENT_PROVIDER,
        "status": status,
    });
    if let Some(identity) = &session.workspace.workspace_identity {
        value["workspaceIdentity"] = serde_json::json!(identity);
    }
    if let Some(model) = &session.model {
        value["model"] = serde_json::json!(format!("{}/{}", model.provider_id, model.model_id));
    }
    if let Some(thought_level) = &snapshot.settings.thought_level.current {
        value["thoughtLevel"] = serde_json::json!(thought_level);
    }
    if let Some(error) = &snapshot.projection.last_error {
        let mut error_value = serde_json::json!({
            "code": error.code.clone().unwrap_or_else(|| error.error_type.clone()),
            "message": error.message,
        });
        if let Some(detail) = &error.detail {
            error_value["detail"] = serde_json::json!(detail);
        }
        if let Some(attribution) = &error.attribution {
            if let Ok(attribution) = serde_json::to_value(attribution) {
                error_value["attribution"] = attribution;
            }
        }
        value["lastError"] = error_value;
    }
    if let Some(Some(goal)) = &snapshot.projection.target {
        if let Ok(goal_value) = serde_json::to_value(goal) {
            value["target"] = goal_value;
        }
    }
    task_index_repo::parse_mode_task_meta(&value)
        .ok_or_else(|| ChannelError::error("task meta snapshot conversion failed".to_string()))
}

// ── 参数形状（camelCase；TS modeTaskService 同名方法入参） ──

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceScopeParams {
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
}

/// TS `emitWorkspaceTaskListChanged` 的批量 scope 形态（grouped/排序保存等）。
fn emit_workspace_events(
    events: &TaskEventBus,
    scopes: &[WorkspaceScopeParams],
    reason: &'static str,
    task_meta: Option<serde_json::Value>,
) {
    for scope in scopes {
        let mut event = WorkspaceTaskListChanged::new(scope.workspace_path.clone(), reason);
        event.workspace_identity = scope.workspace_identity.clone();
        event.task_meta = task_meta.clone();
        events.notify_task_list(&event);
    }
}

/// 取 listen 参数的首个位置参数（wire body 可能是 Array，也可能是原样 JSON 文本）。
fn first_listen_arg(arg: &WireValue) -> Option<serde_json::Value> {
    let first = match arg {
        WireValue::Array(items) => items.first().cloned(),
        WireValue::Undefined => None,
        other => Some(other.clone()),
    }?;
    match first {
        WireValue::Str(text) => Some(serde_json::Value::String(text)),
        other => match other.to_json() {
            serde_json::Value::Array(items) => items.first().cloned(),
            json => Some(json),
        },
    }
}

/// 单任务 workspace 事件（带 meta，TS emitWorkspaceTaskListChanged 直接形态）。
fn task_workspace_event(
    workspace_path: &str,
    workspace_identity: Option<&str>,
    task_id: &str,
    reason: &'static str,
    meta: &ModeTaskMeta,
) -> WorkspaceTaskListChanged {
    WorkspaceTaskListChanged {
        workspace_path: workspace_path.to_string(),
        workspace_identity: workspace_identity.map(str::to_string),
        task_id: Some(task_id.to_string()),
        reason,
        task_meta: serde_json::to_value(meta).ok(),
    }
}

/// `onDynamicWorkspaceEvent(workspace)` 的参数：string path 或 `{workspacePath, workspaceIdentity}`。
fn parse_workspace_event_scope(arg: &WireValue) -> WorkspaceEventScope {
    match first_listen_arg(arg) {
        Some(serde_json::Value::String(path)) => WorkspaceEventScope {
            workspace_path: Some(path),
            workspace_identity: None,
        },
        Some(json) => WorkspaceEventScope {
            workspace_path: json
                .get("workspacePath")
                .and_then(|value| value.as_str())
                .map(str::to_string),
            workspace_identity: json
                .get("workspaceIdentity")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        },
        None => WorkspaceEventScope::default(),
    }
}

/// `onDynamicTaskReady(taskId)` 的参数：首个位置参数为 taskId。
fn parse_task_id_arg(arg: &WireValue) -> Option<String> {
    first_listen_arg(arg)?.as_str().map(str::to_string)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceParams {
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TaskListKindParam {
    Pinned,
    Archived,
    Timeline,
    Active,
}

impl TaskListKindParam {
    fn into_repo(self) -> TaskListKind {
        match self {
            TaskListKindParam::Pinned => TaskListKind::Pinned,
            TaskListKindParam::Archived => TaskListKind::Archived,
            TaskListKindParam::Timeline => TaskListKind::Timeline,
            TaskListKindParam::Active => TaskListKind::Active,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TaskListSortByParam {
    Created,
    Updated,
}

impl TaskListSortByParam {
    fn into_repo(self) -> TaskListSortBy {
        match self {
            TaskListSortByParam::Created => TaskListSortBy::Created,
            TaskListSortByParam::Updated => TaskListSortBy::Updated,
        }
    }
}

fn repo_scopes(scopes: Vec<WorkspaceScopeParams>) -> Vec<TaskListWorkspaceScope> {
    scopes
        .into_iter()
        .map(|scope| TaskListWorkspaceScope {
            workspace_path: scope.workspace_path,
            workspace_identity: scope.workspace_identity,
            workspace_purpose: None,
        })
        .collect()
}

fn parse<T: serde::de::DeserializeOwned>(
    object: serde_json::Map<String, serde_json::Value>,
) -> Result<T, ChannelError> {
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

/// TS `getTaskTarget`：显式 workspace 优先，缺省从任务索引回读任务归属。
fn resolve_task_workspace(
    repo: &TaskIndexRepo,
    workspace_path: Option<&str>,
    workspace_identity: Option<&str>,
    task_id: &str,
) -> Result<(String, Option<String>), ChannelError> {
    if let Some(path) = workspace_path {
        return Ok((path.to_string(), workspace_identity.map(str::to_string)));
    }
    let indexed = repo
        .get_task_meta(&TaskRef {
            workspace_path: String::new(),
            workspace_identity: None,
            task_id: task_id.to_string(),
        })
        .map_err(|error| ChannelError::error(error.to_string()))?;
    match indexed {
        Some(meta) => Ok((meta.workspace_path, meta.workspace_identity)),
        None => Err(ChannelError::error(format!(
            "task not indexed and workspacePath missing: {task_id}"
        ))),
    }
}

/// 与 mode-agent `disposeWorkspace` 同口径的回收宽限。
const DISPOSE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

fn now_ms_i64() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn json_value(value: &serde_json::Value) -> Result<WireValue, ChannelError> {
    match serde_json::to_string(value) {
        Ok(text) => Ok(WireValue::Json(text)),
        Err(error) => Err(ChannelError::error(error.to_string())),
    }
}

// ── 分发 ──

fn run_read(
    repo: &TaskIndexRepo,
    events: &TaskEventBus,
    command: &str,
    arg: &WireValue,
) -> Result<WireValue, ChannelError> {
    let object = crate::params::first_arg_object(arg)?;
    match command {
        "listTaskList" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                kind: TaskListKindParam,
                #[serde(default)]
                workspace_scopes: Vec<WorkspaceScopeParams>,
                sort_by: TaskListSortByParam,
                #[serde(default)]
                search: Option<String>,
                #[serde(default)]
                limit: Option<f64>,
            }
            let parsed: P = parse(object)?;
            let result = repo
                .query_task_list(&TaskListQuery {
                    kind: parsed.kind.into_repo(),
                    workspace_scopes: repo_scopes(parsed.workspace_scopes),
                    sort_by: parsed.sort_by.into_repo(),
                    search: parsed.search,
                    provider: None,
                    limit: parsed.limit,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(
                &serde_json::to_value(&result)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "listTasks" | "listPinnedTasks" | "listArchivedTasks" => {
            let parsed: WorkspaceParams = parse(object)?;
            // listTasks = 全量；listPinnedTasks = pinned；listArchivedTasks = archived。
            let (pinned, archived) = match command {
                "listTasks" => (None, None),
                "listPinnedTasks" => (Some(true), None),
                _ => (None, Some(true)),
            };
            let metas = repo
                .list_task_metas(&ListTaskMetasParams {
                    workspace_path: Some(parsed.workspace_path),
                    workspace_identity: parsed.workspace_identity,
                    provider: None,
                    pinned,
                    archived,
                    include_deleted: false,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(
                &serde_json::to_value(&metas)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "listPinnedTaskIds" => {
            let ids: Vec<String> = repo
                .list_task_metas(&ListTaskMetasParams {
                    workspace_path: None,
                    workspace_identity: None,
                    provider: None,
                    pinned: Some(true),
                    archived: None,
                    include_deleted: false,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?
                .into_iter()
                .map(|meta| meta.task_id)
                .collect();
            json_value(
                &serde_json::to_value(&ids)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "listDeletedTaskIds" => {
            let parsed: WorkspaceParams = parse(object)?;
            let ids = repo
                .list_deleted_task_ids(&ListDeletedTaskIdsParams {
                    workspace_path: parsed.workspace_path,
                    workspace_identity: parsed.workspace_identity,
                    provider: None,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(
                &serde_json::to_value(&ids)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "listGroupedTaskViewStructure" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                #[serde(default)]
                workspace_scopes: Vec<WorkspaceScopeParams>,
            }
            let parsed: P = parse(object)?;
            let structure = repo
                .query_grouped_task_view_structure(&repo_scopes(parsed.workspace_scopes))
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(
                &serde_json::to_value(&structure)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "applyGroupedTaskViewOrder" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                #[serde(default)]
                workspace_scopes: Vec<WorkspaceScopeParams>,
                #[serde(default)]
                top_level_nodes: Vec<TopLevelNodeParam>,
                #[serde(default)]
                groups: Vec<GroupRefsParam>,
            }
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct TopLevelNodeParam {
                #[serde(rename = "type")]
                kind: String,
                #[serde(default)]
                group_id: Option<String>,
                #[serde(default)]
                task: Option<TaskRefParam>,
                #[serde(default)]
                workspace_key: Option<String>,
                #[serde(default)]
                task_id: Option<String>,
                // TS 顶层节点可能带 sortOrder——repo 的排序由 node_orders 表持有，
                // 该字段透传兼容（不参与写入）。
                #[serde(default)]
                #[allow(dead_code)]
                sort_order: Option<f64>,
            }
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct TaskRefParam {
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
                task_id: String,
            }
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct GroupRefsParam {
                group_id: String,
                #[serde(default)]
                task_refs: Vec<TaskRefParam>,
            }
            let parsed: P = parse(object)?;
            let top_level_nodes = parsed
                .top_level_nodes
                .into_iter()
                .map(|node| match node.kind.as_str() {
                    "group" => GroupedTaskViewTopLevelNodeRef::Group {
                        group_id: node.group_id.unwrap_or_default(),
                    },
                    _ => {
                        // task 节点：TS 有两种形状——完整 task 引用与结构化
                        // {workspaceKey, taskId}（后者从 workspace_key 还原 path）。
                        if let Some(task) = node.task {
                            GroupedTaskViewTopLevelNodeRef::Task {
                                task: GroupedTaskRef {
                                    workspace_path: task.workspace_path,
                                    workspace_identity: task.workspace_identity,
                                    task_id: task.task_id,
                                },
                            }
                        } else {
                            GroupedTaskViewTopLevelNodeRef::Task {
                                task: GroupedTaskRef {
                                    workspace_path: node.workspace_key.unwrap_or_default(),
                                    workspace_identity: None,
                                    task_id: node.task_id.unwrap_or_default(),
                                },
                            }
                        }
                    }
                })
                .collect();
            let groups = parsed
                .groups
                .into_iter()
                .map(|group| GroupedTaskGroupRefs {
                    group_id: group.group_id,
                    task_refs: group
                        .task_refs
                        .into_iter()
                        .map(|task| GroupedTaskRef {
                            workspace_path: task.workspace_path,
                            workspace_identity: task.workspace_identity,
                            task_id: task.task_id,
                        })
                        .collect(),
                })
                .collect();
            // TS adapter：排序保存后按每个 scope 广播 task_meta_changed，驱动 grouped 重拉。
            let scopes = parsed.workspace_scopes.clone();
            let view = repo
                .apply_grouped_task_view_order(&GroupedTaskViewOrderInput {
                    workspace_scopes: repo_scopes(parsed.workspace_scopes),
                    top_level_nodes,
                    groups,
                    provider: None,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            emit_workspace_events(events, &scopes, "task_meta_changed", None);
            json_value(
                &serde_json::to_value(&view)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "createTaskGroup" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                #[serde(default)]
                title: Option<String>,
                #[serde(default)]
                color: Option<String>,
            }
            let parsed: P = parse(object)?;
            let group = repo
                .create_task_group(parsed.title.as_deref(), parsed.color.as_deref())
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(
                &serde_json::to_value(&group)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "renameTaskGroup" | "updateTaskGroupColor" | "deleteTaskGroup" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                group_id: String,
                #[serde(default)]
                title: Option<String>,
                #[serde(default)]
                color: Option<String>,
                #[serde(default)]
                workspace_scopes: Vec<WorkspaceScopeParams>,
            }
            let parsed: P = parse(object)?;
            let workspace_scopes = parsed.workspace_scopes;
            let result = match command {
                "renameTaskGroup" => {
                    let title = parsed
                        .title
                        .ok_or_else(|| ChannelError::error("renameTaskGroup requires title"))?;
                    let group = repo
                        .rename_task_group(&parsed.group_id, &title)
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    json_value(
                        &serde_json::to_value(&group)
                            .map_err(|error| ChannelError::error(error.to_string()))?,
                    )?
                }
                "updateTaskGroupColor" => {
                    let color = parsed.color.ok_or_else(|| {
                        ChannelError::error("updateTaskGroupColor requires color")
                    })?;
                    let group = repo
                        .update_task_group_color(&parsed.group_id, &color)
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    json_value(
                        &serde_json::to_value(&group)
                            .map_err(|error| ChannelError::error(error.to_string()))?,
                    )?
                }
                _ => {
                    repo.delete_task_group(&parsed.group_id)
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    WireValue::Undefined
                }
            };
            // TS adapter：分组结构变更无单任务 meta，按 scope 广播 task_meta_changed。
            emit_workspace_events(events, &workspace_scopes, "task_meta_changed", None);
            Ok(result)
        }
        "archiveWorkspaceTasks" => {
            // TS adapter 同语义：列出未归档 → 逐个 patch archived → 返回归档后列表。
            // provider 固定 MODE_AGENT_PROVIDER（glm），与 TS GLM_PROVIDER 一致。
            let parsed: WorkspaceParams = parse(object)?;
            let metas = repo
                .list_task_metas(&ListTaskMetasParams {
                    workspace_path: Some(parsed.workspace_path.clone()),
                    workspace_identity: parsed.workspace_identity.clone(),
                    provider: Some(task_index_repo::MODE_AGENT_PROVIDER.to_string()),
                    pinned: None,
                    archived: Some(false),
                    include_deleted: false,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            for meta in &metas {
                repo.update_task_state(
                    &task_index_repo::TaskRef {
                        workspace_path: meta.workspace_path.clone(),
                        workspace_identity: meta.workspace_identity.clone(),
                        task_id: meta.task_id.clone(),
                    },
                    &task_index_repo::TaskIndexStatePatch {
                        archived: Some(true),
                        ..task_index_repo::TaskIndexStatePatch::default()
                    },
                )
                .map_err(|error| ChannelError::error(error.to_string()))?;
            }
            // TS adapter：批量归档无逐任务 meta，一次 scope 事件驱动 membership 重拉。
            events.notify_task_list(&WorkspaceTaskListChanged {
                workspace_path: parsed.workspace_path.clone(),
                workspace_identity: parsed.workspace_identity.clone(),
                task_id: None,
                reason: "task_meta_changed",
                task_meta: None,
            });
            let archived = repo
                .list_task_metas(&ListTaskMetasParams {
                    workspace_path: Some(parsed.workspace_path),
                    workspace_identity: parsed.workspace_identity,
                    provider: Some(task_index_repo::MODE_AGENT_PROVIDER.to_string()),
                    pinned: None,
                    archived: Some(true),
                    include_deleted: false,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(
                &serde_json::to_value(&archived)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "archiveStaleTasks" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
                older_than_days: f64,
            }
            let parsed: P = parse(object)?;
            let metas = repo
                .archive_stale_tasks(&ArchiveStaleTasksParams {
                    workspace_path: parsed.workspace_path.clone(),
                    workspace_identity: parsed.workspace_identity.clone(),
                    older_than_days: parsed.older_than_days,
                    provider: None,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            // TS adapter：逐任务带 meta 广播 task_meta_changed（membership 重拉收敛）。
            for meta in &metas {
                events.notify_task_list(&WorkspaceTaskListChanged {
                    workspace_path: meta.workspace_path.clone(),
                    workspace_identity: meta.workspace_identity.clone(),
                    task_id: Some(meta.task_id.clone()),
                    reason: "task_meta_changed",
                    task_meta: serde_json::to_value(meta).ok(),
                });
            }
            json_value(
                &serde_json::to_value(&metas)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        // ── 任务状态写面（renderer 侧菜单/归档/未读直接调用 mode-task）──
        "renameTask" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
                title: String,
            }
            let parsed: P = parse(object)?;
            // TS renameTask：同时推进 updatedAt + titleOverridden，避免运行中
            // optimistic merge 把同 updatedAt 的旧长标题当成更强 meta。
            let meta = repo
                .update_task_state(
                    &TaskRef {
                        workspace_path: parsed.workspace_path.clone(),
                        workspace_identity: parsed.workspace_identity.clone(),
                        task_id: parsed.task_id.clone(),
                    },
                    &TaskIndexStatePatch {
                        title: Some(parsed.title.clone()),
                        title_overridden: Some(true),
                        updated_at: Some(now_ms_i64()),
                        ..TaskIndexStatePatch::default()
                    },
                )
                .map_err(|error| ChannelError::error(error.to_string()))?;
            // 已知差距：TS 还会 best-effort 发 v4 renameSession 同步 CLI session store；
            // 壳内先只保 tasks-index，v4 侧边栏标题同步待 16.2 续。
            events.notify_task_list(&task_workspace_event(
                &parsed.workspace_path,
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
                "task_title_changed",
                &meta,
            ));
            json_value(
                &serde_json::to_value(&meta)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "getTaskMeta" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let meta = repo
                .get_task_meta(&TaskRef {
                    workspace_path: parsed.workspace_path,
                    workspace_identity: parsed.workspace_identity,
                    task_id: parsed.task_id,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            match meta {
                Some(meta) => json_value(
                    &serde_json::to_value(&meta)
                        .map_err(|error| ChannelError::error(error.to_string()))?,
                ),
                None => json_value(&serde_json::Value::Null),
            }
        }
        "scanImportableClaudeSessions" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                #[serde(default)]
                workspace_path: Option<String>,
                // 仅影响导入落盘目录；扫描按 jsonl 内 cwd 与 workspacePath 过滤。
                #[serde(default)]
                #[allow(dead_code)]
                workspace_identity: Option<String>,
                #[serde(default)]
                modified_since: Option<i64>,
                #[serde(default)]
                limit: Option<f64>,
            }
            let parsed: P = parse(object)?;
            let candidates =
                crate::claude_import::scan_importable_sessions(&crate::claude_import::ScanParams {
                    workspace_path: parsed.workspace_path,
                    modified_since: parsed.modified_since,
                    limit: parsed.limit,
                });
            json_value(&serde_json::Value::Array(candidates))
        }
        "getModelTrajectory" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                #[serde(default)]
                limit: Option<f64>,
            }
            let parsed: P = parse(object)?;
            // TS：按 sessionId(=taskId) 读 model-io JSONL；runtime 未落盘时返回空 records。
            let trajectory =
                crate::model_trajectory::read_model_trajectory(&parsed.task_id, parsed.limit);
            json_value(&trajectory)
        }
        "getTaskNativeSessionLogFile" => {
            // TS：MODE_LOG_DIR 或 {home}/.mode/cli/log/mode-YYYY-MM-DD.jsonl（本地日期）。
            let (path, exists) =
                crate::mode_config_options::native_session_log_file(chrono::Local::now())
                    .map_err(ChannelError::error)?;
            json_value(&serde_json::json!({
                "provider": task_index_repo::MODE_AGENT_PROVIDER,
                "path": path,
                "exists": exists,
            }))
        }
        "getTaskSessionFilePath" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                workspace_path: String,
            }
            let parsed: P = parse(object)?;
            // TS 同实现：legacy ACP 下线后无真实 session 文件，恒 exists=false。
            json_value(&serde_json::json!({
                "path": format!("{}/{}.mode-session", parsed.workspace_path, parsed.task_id),
                "exists": false,
            }))
        }
        "setTaskPinned" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
                pinned: bool,
            }
            let parsed: P = parse(object)?;
            let meta = repo
                .update_task_state(
                    &TaskRef {
                        workspace_path: parsed.workspace_path.clone(),
                        workspace_identity: parsed.workspace_identity.clone(),
                        task_id: parsed.task_id.clone(),
                    },
                    &TaskIndexStatePatch {
                        pinned: Some(parsed.pinned),
                        ..TaskIndexStatePatch::default()
                    },
                )
                .map_err(|error| ChannelError::error(error.to_string()))?;
            events.notify_task_list(&task_workspace_event(
                &parsed.workspace_path,
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
                if parsed.pinned {
                    "task_pinned"
                } else {
                    "task_unpinned"
                },
                &meta,
            ));
            json_value(
                &serde_json::to_value(&meta)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "setTaskUnread" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
                unread: bool,
                #[serde(default)]
                expected_unread_at: Option<i64>,
            }
            let parsed: P = parse(object)?;
            let task_ref = TaskRef {
                workspace_path: parsed.workspace_path.clone(),
                workspace_identity: parsed.workspace_identity.clone(),
                task_id: parsed.task_id.clone(),
            };
            let (meta, emit) = if !parsed.unread {
                if let Some(expected) = parsed.expected_unread_at {
                    // CAS 未命中不能先乐观清除（旧手机点击可能晚于新的终态未读）。
                    let result = repo
                        .clear_task_unread_if_matches(&task_ref, expected)
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    (result.meta, result.cleared.then_some("task_meta_changed"))
                } else {
                    let meta = repo
                        .update_task_state(
                            &task_ref,
                            &TaskIndexStatePatch {
                                unread_at: Some(None),
                                ..TaskIndexStatePatch::default()
                            },
                        )
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    (meta, Some("task_meta_changed"))
                }
            } else {
                let meta = repo
                    .update_task_state(
                        &task_ref,
                        &TaskIndexStatePatch {
                            unread_at: Some(Some(now_ms_i64())),
                            ..TaskIndexStatePatch::default()
                        },
                    )
                    .map_err(|error| ChannelError::error(error.to_string()))?;
                (meta, Some("task_meta_changed"))
            };
            if let Some(reason) = emit {
                events.notify_task_list(&task_workspace_event(
                    &parsed.workspace_path,
                    parsed.workspace_identity.as_deref(),
                    &parsed.task_id,
                    reason,
                    &meta,
                ));
            }
            json_value(
                &serde_json::to_value(&meta)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "archiveTask" | "unarchiveTask" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let archived = command == "archiveTask";
            let meta = repo
                .update_task_state(
                    &TaskRef {
                        workspace_path: parsed.workspace_path.clone(),
                        workspace_identity: parsed.workspace_identity.clone(),
                        task_id: parsed.task_id.clone(),
                    },
                    &TaskIndexStatePatch {
                        archived: Some(archived),
                        ..TaskIndexStatePatch::default()
                    },
                )
                .map_err(|error| ChannelError::error(error.to_string()))?;
            events.notify_task_list(&task_workspace_event(
                &parsed.workspace_path,
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
                if archived {
                    "task_archived"
                } else {
                    "task_unarchived"
                },
                &meta,
            ));
            json_value(
                &serde_json::to_value(&meta)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "deleteTask" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let meta = repo
                .update_task_state(
                    &TaskRef {
                        workspace_path: parsed.workspace_path.clone(),
                        workspace_identity: parsed.workspace_identity.clone(),
                        task_id: parsed.task_id.clone(),
                    },
                    &TaskIndexStatePatch {
                        deleted: Some(true),
                        ..TaskIndexStatePatch::default()
                    },
                )
                .map_err(|error| ChannelError::error(error.to_string()))?;
            // TS deleteTask：持久删除后发 task_deleted（带 meta），UI 立即移除并换代 tombstone。
            events.notify_task_list(&task_workspace_event(
                &parsed.workspace_path,
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
                "task_deleted",
                &meta,
            ));
            Ok(WireValue::Undefined)
        }
        "deleteArchivedTask" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let deleted = repo
                .delete_archived_task(&TaskRef {
                    workspace_path: parsed.workspace_path.clone(),
                    workspace_identity: parsed.workspace_identity.clone(),
                    task_id: parsed.task_id.clone(),
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            if let Some(meta) = &deleted {
                events.notify_task_list(&task_workspace_event(
                    &parsed.workspace_path,
                    parsed.workspace_identity.as_deref(),
                    &parsed.task_id,
                    "task_deleted",
                    meta,
                ));
            }
            json_value(&serde_json::json!(deleted.is_some()))
        }
        "deleteArchivedTasks" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
                task_ids: Vec<String>,
            }
            let parsed: P = parse(object)?;
            let mut seen = std::collections::HashSet::new();
            let task_ids: Vec<String> = parsed
                .task_ids
                .into_iter()
                .filter(|task_id| seen.insert(task_id.clone()))
                .collect();
            let mut deleted = Vec::new();
            let mut skipped = Vec::new();
            let mut failed = Vec::new();
            for task_id in task_ids {
                let single = TaskRef {
                    workspace_path: parsed.workspace_path.clone(),
                    workspace_identity: parsed.workspace_identity.clone(),
                    task_id,
                };
                match repo.delete_archived_task(&single) {
                    Ok(Some(meta)) => deleted.push(meta.task_id),
                    Ok(None) => skipped.push(single.task_id),
                    Err(_) => failed.push(single.task_id),
                }
            }
            // TS：整批成功后只广播一次 task_deleted（无逐任务 meta），避免逐项全量重读。
            if !deleted.is_empty() {
                events.notify_task_list(&WorkspaceTaskListChanged {
                    workspace_path: parsed.workspace_path,
                    workspace_identity: parsed.workspace_identity,
                    task_id: None,
                    reason: "task_deleted",
                    task_meta: None,
                });
            }
            json_value(&serde_json::json!({
                "deletedTaskIds": deleted,
                "skippedTaskIds": skipped,
                "failedTaskIds": failed,
            }))
        }
        // TS 空桩（legacy ACP 下线后 Body/Ref/ToolCallsSlice 恒 null）——同语义。
        "getTaskSnapshotBody" | "getTaskSnapshotRef" | "getTaskSnapshotToolCallsSlice" => {
            Ok(WireValue::Undefined)
        }
        other => Err(ChannelError::error(format!(
            "mode-task channel: unknown or not-yet-migrated command {other}"
        ))),
    }
}

// ── 16.2a 编排面（async：AgentControlClient 的会话面调用）──
async fn run_orchestration(
    channel: &ModeTaskChannel,
    repo: &TaskIndexRepo,
    control: &AgentControlClient,
    command: &str,
    object: serde_json::Map<String, serde_json::Value>,
) -> Result<WireValue, ChannelError> {
    match command {
        "createTask" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
                #[serde(default)]
                mode: Option<agent_protocol::session_read::SessionMode>,
                #[serde(default)]
                model: Option<agent_protocol::message::MessageModel>,
                #[serde(default)]
                thought_level: Option<String>,
                #[serde(default)]
                automation_id: Option<String>,
                #[serde(default)]
                off_peak_task_id: Option<String>,
                // replayable 首发复用 draft session（手机端草稿；TS createTask 首分支）。
                #[serde(default)]
                draft_session_id: Option<String>,
            }
            let parsed: P = parse(object)?;
            // control 已由 call 分流保证存在
            let target = AgentWorkspaceTarget {
                workspace_path: parsed.workspace_path.clone(),
                workspace_identity: parsed.workspace_identity.clone(),
                remote_session_id: None,
            };
            control
                .ensure_provider_config(&target)
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;

            // draft 复用（TS createTask 首分支）：draft session 存在 → 按请求同步
            // model/thoughtLevel → 复用 snapshot；session missing → 降级新建
            // （远端重连/agent 重启后旧 draftSessionId 可能失效，避免用户卡死）。
            // mcpServers 载荷不在本切片（None 时才尝试复用，TS 同条件）。
            let mut reused_snapshot: Option<SessionStateSnapshot> = None;
            let draft_session_id = parsed
                .draft_session_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            if let Some(draft_id) = &draft_session_id {
                match control
                    .read_session(&target, draft_id, None, None, None)
                    .await
                {
                    Ok(draft) => {
                        let mut current = draft;
                        if let Some(selection) = &parsed.model {
                            // replayable 首发复用 draft 时必须以 UI 当前模型为准，
                            // 否则手机远控显示新模型但真实请求仍用旧模型（TS 同因注释）。
                            let same = current
                                .settings
                                .model
                                .current
                                .as_ref()
                                .map(|existing| {
                                    existing.provider_id == selection.provider_id
                                        && existing.model_id == selection.model_id
                                        && existing.options == selection.options
                                })
                                .unwrap_or(false);
                            if !same {
                                current = control
                                    .set_session_model(&target, draft_id, selection, None, None)
                                    .await
                                    .map_err(|error| ChannelError::error(error.to_string()))?;
                            }
                        }
                        if let Some(selection) = &parsed.model {
                            if let Some(reasoning) = selection
                                .options
                                .as_ref()
                                .and_then(|options| options.reasoning_level.as_deref())
                            {
                                if current.settings.thought_level.current.as_deref()
                                    != Some(reasoning)
                                {
                                    current = control
                                        .set_session_thought_level(
                                            &target,
                                            draft_id,
                                            Some(reasoning),
                                            None,
                                            None,
                                        )
                                        .await
                                        .map_err(|error| ChannelError::error(error.to_string()))?;
                                }
                            }
                        }
                        reused_snapshot = Some(current);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        // TS isSessionMissingError：/Session (not found|is not active):/i
                        let missing = message.contains("Session not found")
                            || message.contains("Session is not active");
                        if !missing {
                            return Err(ChannelError::error(message));
                        }
                        // 降级新建（TS logger.warn 同义）。
                    }
                }
            }

            let snapshot: SessionStateSnapshot = match reused_snapshot {
                Some(snapshot) => snapshot,
                None => {
                    // automation / 闲时任务新建空 session 后立即 sendText——V4 admission
                    // 必须在首发前统一持久化 session 主记录（TS deferred persistence 同因）。
                    let persistence = if parsed.automation_id.is_some() {
                        Some(SessionPersistence::Deferred)
                    } else {
                        None
                    };
                    control
                        .create_session(
                            &target,
                            agent_process::CreateSessionOptions {
                                session_id: None,
                                parent_session_id: None,
                                mode: parsed.mode,
                                model: parsed.model,
                                persistence,
                                thought_level: parsed.thought_level,
                                title_generation_enabled: parsed
                                    .automation_id
                                    .as_ref()
                                    .map(|_| false),
                                mcp_servers: None,
                                tool_allowlist: None,
                                tool_denylist: None,
                                imported_history: None,
                                off_peak_tool_enabled: false,
                                dynamic_workflow_enabled: false,
                            },
                        )
                        .await
                        .map_err(|error| ChannelError::error(error.to_string()))?
                }
            };
            let mut meta = snapshot_to_meta(&snapshot)?;
            if parsed.automation_id.is_some() || parsed.off_peak_task_id.is_some() {
                meta.cron_automation_id = parsed.automation_id.clone();
                meta.off_peak_task_id = parsed.off_peak_task_id.clone();
            }
            repo.sync_task_meta(task_index_repo::SyncTaskMetaParams {
                meta: meta.clone(),
                pinned: None,
                archived: None,
                deleted: None,
                title_overridden: None,
                searchable_text: None,
            })
            .map_err(|error| ChannelError::error(error.to_string()))?;
            let _ = repo.initialize_grouped_task_at_top(&GroupedTaskRef {
                workspace_path: meta.workspace_path.clone(),
                workspace_identity: meta.workspace_identity.clone(),
                task_id: meta.task_id.clone(),
            });
            let mut result = serde_json::to_value(&meta)
                .map_err(|error| ChannelError::error(error.to_string()))?;
            // task 创建结果携带 agent 协议快照里的命令列表（TS initialSlashCommands）。
            if let Some(commands) = &snapshot.slash_commands {
                if let Ok(value) = serde_json::to_value(commands) {
                    result["initialSlashCommands"] = value;
                }
            }
            // create 事件保留 task_created 语义（远控首页靠它插入新任务）。
            channel.events.notify_task_list(&WorkspaceTaskListChanged {
                workspace_path: meta.workspace_path.clone(),
                workspace_identity: meta.workspace_identity.clone(),
                task_id: Some(meta.task_id.clone()),
                reason: "task_created",
                task_meta: Some(result.clone()),
            });
            json_value(&result)
        }
        "closeTask" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            // control 已由 call 分流保证存在
            let target = AgentWorkspaceTarget {
                workspace_path: parsed.workspace_path.clone(),
                workspace_identity: parsed.workspace_identity.clone(),
                remote_session_id: None,
            };
            control
                .close_session(&target, &parsed.task_id, None)
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            repo.update_task_state(
                &task_index_repo::TaskRef {
                    workspace_path: parsed.workspace_path.clone(),
                    workspace_identity: parsed.workspace_identity,
                    task_id: parsed.task_id.clone(),
                },
                &task_index_repo::TaskIndexStatePatch {
                    deleted: Some(true),
                    ..task_index_repo::TaskIndexStatePatch::default()
                },
            )
            .map_err(|error| ChannelError::error(error.to_string()))?;
            // 删除列表收敛由 sessions-index session.removed 驱动；广播沿用旧语义兜底。
            channel.events.notify_task_list(&WorkspaceTaskListChanged {
                workspace_path: target.workspace_path.clone(),
                workspace_identity: target.workspace_identity.clone(),
                task_id: Some(parsed.task_id.clone()),
                reason: "task_meta_changed",
                task_meta: None,
            });
            Ok(WireValue::Undefined)
        }
        "resumeTask" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
                #[serde(default)]
                thought_level: Option<String>,
            }
            let parsed: P = parse(object)?;
            // control 已由 call 分流保证存在
            let target = AgentWorkspaceTarget {
                workspace_path: parsed.workspace_path.clone(),
                workspace_identity: parsed.workspace_identity.clone(),
                remote_session_id: None,
            };
            let snapshot: SessionStateSnapshot = control
                .resume_session(
                    &target,
                    agent_protocol::session_write::SessionResumeParams {
                        session_id: parsed.task_id,
                        workspace: None,
                        thought_level: parsed.thought_level,
                        mcp_servers: None,
                        tool_allowlist: None,
                        tool_denylist: None,
                        off_peak_tool_enabled: None,
                        dynamic_workflow_enabled: None,
                    },
                )
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            let meta = snapshot_to_meta(&snapshot)?;
            repo.sync_task_meta(task_index_repo::SyncTaskMetaParams {
                meta: meta.clone(),
                pinned: None,
                archived: None,
                deleted: None,
                title_overridden: None,
                searchable_text: None,
            })
            .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(
                &serde_json::to_value(&meta)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "sendPrompt" => {
            // TS sendPrompt = sessionSend 直转发（meta 更新事件在 16.2c 事件面）。
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                content: String,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
                #[serde(default)]
                input_id: Option<String>,
                #[serde(default)]
                query_id: Option<String>,
                #[serde(default)]
                automation_id: Option<String>,
            }
            let parsed: P = parse(object)?;
            // control 已由 call 分流保证存在
            // TS getTaskTarget：任务归属 workspace 由任务索引读出（task_id 即 session id）。
            let indexed = repo
                .get_task_meta(&task_index_repo::TaskRef {
                    workspace_path: parsed.workspace_path.clone().unwrap_or_default(),
                    workspace_identity: parsed.workspace_identity.clone(),
                    task_id: parsed.task_id.clone(),
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            let (workspace_path, workspace_identity) = match (&parsed.workspace_path, &indexed) {
                (Some(path), _) => (path.clone(), parsed.workspace_identity.clone()),
                (None, Some(meta)) => {
                    (meta.workspace_path.clone(), meta.workspace_identity.clone())
                }
                (None, None) => {
                    return Err(ChannelError::error(
                        "sendPrompt requires workspacePath or an indexed task",
                    ))
                }
            };
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            // 终态事件用本地记录的 inputId 收口本次 run（TS 在 dispatch 前记录）。
            if let Some(input_id) = &parsed.input_id {
                channel.events.record_active_input(
                    &target.workspace_path,
                    target.workspace_identity.as_deref(),
                    &parsed.task_id,
                    input_id,
                );
            }
            let result = control
                .send_prompt(
                    &target,
                    agent_protocol::session::SessionSendParams {
                        session_id: parsed.task_id,
                        model_selection: None,
                        model_execution: None,
                        input_id: parsed.input_id,
                        query_id: parsed.query_id,
                        content: parsed.content,
                        attachments: None,
                        browser_ambient_context: None,
                        expected_revision: None,
                        expected_provider_revision: None,
                        automation_id: parsed.automation_id.clone(),
                        off_peak_task_id: None,
                        off_peak_run_type: None,
                        bot_delivery_target: None,
                        tool_denylist: None,
                    },
                )
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(
                &serde_json::to_value(&result)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "enqueueTaskCommand" | "promoteTaskCommand" | "cancelTaskCommand" => {
            let queue = channel.queue.as_ref().ok_or_else(|| {
                ChannelError::error(format!(
                    "mode-task channel: {command} requires the task command queue (not assembled)"
                ))
            })?;
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                command_id: String,
                task_id: String,
                #[serde(default)]
                trace_id: Option<String>,
                #[serde(default)]
                query_id: Option<String>,
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
                #[serde(default)]
                content: String,
                #[serde(default)]
                automation_id: Option<String>,
                #[serde(default)]
                owner_run_id: Option<String>,
                #[serde(default)]
                client_id: Option<String>,
                #[serde(default)]
                client_label: Option<String>,
            }
            let parsed: P = parse(object)?;
            let params = QueueCommandParams {
                command_id: parsed.command_id.clone(),
                task_id: parsed.task_id.clone(),
                trace_id: parsed.trace_id.clone(),
                query_id: parsed.query_id.clone(),
                workspace_path: parsed.workspace_path.clone(),
                workspace_identity: parsed.workspace_identity.clone(),
                content: parsed.content.clone(),
                automation_id: parsed.automation_id.clone(),
                attachments: None,
            };
            match command {
                "enqueueTaskCommand" => {
                    let result: EnqueueResult = queue.enqueue(
                        &params,
                        parsed.owner_run_id.as_deref(),
                        parsed.client_id.as_deref(),
                        parsed.client_label.as_deref(),
                    )?;
                    json_value(&serde_json::to_value(&result).map_err(|error| ChannelError::error(error.to_string()))?)
                }
                "promoteTaskCommand" => {
                    let result: EnqueueResult = queue.promote(
                        &params,
                        &parsed.command_id,
                        parsed.owner_run_id.as_deref(),
                    )?;
                    json_value(&serde_json::to_value(&result).map_err(|error| ChannelError::error(error.to_string()))?)
                }
                _ => {
                    let result: CancelResult = queue.cancel(
                        &params,
                        &parsed.command_id,
                        parsed.owner_run_id.as_deref(),
                    )?;
                    json_value(&serde_json::to_value(&result).map_err(|error| ChannelError::error(error.to_string()))?)
                }
            }
        }
        "respondPermission" => {
            // permission 回执收敛 v4 resolveInteraction（TS respondPermission 同语义）：
            // interactionId = 业务 requestId；optionId 由 CLI 侧精确回映 response。
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                request_id: String,
                option_id: String,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let control = channel.control.clone().ok_or_else(|| {
                ChannelError::error(
                    "mode-task channel: respondPermission requires the agent control plane",
                )
            })?;
            let (workspace_path, workspace_identity) =
                resolve_task_workspace(repo, parsed.workspace_path.as_deref(), parsed.workspace_identity.as_deref(), &parsed.task_id)?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            let envelope = agent_protocol::v4::ConversationCommandEnvelope {
                ttft: None,
                command_id: uuid::Uuid::new_v4().to_string(),
                client_id: "mode-host".to_string(),
                session_id: agent_protocol::v4::CommandSessionId(Some(parsed.task_id.clone())),
                base_revision: None,
                base_log_epoch: None,
                command_type: agent_protocol::v4::CommandType::ResolveInteraction,
                payload: serde_json::json!({
                    "interactionId": parsed.request_id,
                    "answer": { "optionId": parsed.option_id },
                }),
                issued_at: serde_json::Number::from(now_ms_i64()),
            };
            let ack = control
                .send_conversation_command_v4(&target, &envelope)
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(&serde_json::json!({ "accepted": true, "ack": ack }))
        }
        "respondElicitation" => {
            // AskUserQuestion/plan-approval 回执收敛 v4 resolveInteraction：
            // answer.action/content 是 additive 扩展（TS 同注释）。
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                request_id: String,
                #[serde(default)]
                action: Option<String>,
                #[serde(default)]
                content: Option<String>,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let control = channel.control.clone().ok_or_else(|| {
                ChannelError::error(
                    "mode-task channel: respondElicitation requires the agent control plane",
                )
            })?;
            let (workspace_path, workspace_identity) =
                resolve_task_workspace(repo, parsed.workspace_path.as_deref(), parsed.workspace_identity.as_deref(), &parsed.task_id)?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            let mut answer = serde_json::Map::new();
            if let Some(action) = &parsed.action {
                answer.insert("action".to_string(), serde_json::json!(action));
            }
            if let Some(content) = &parsed.content {
                answer.insert("content".to_string(), serde_json::json!(content));
            }
            let envelope = agent_protocol::v4::ConversationCommandEnvelope {
                ttft: None,
                command_id: uuid::Uuid::new_v4().to_string(),
                client_id: "mode-host".to_string(),
                session_id: agent_protocol::v4::CommandSessionId(Some(parsed.task_id.clone())),
                base_revision: None,
                base_log_epoch: None,
                command_type: agent_protocol::v4::CommandType::ResolveInteraction,
                payload: serde_json::json!({
                    "interactionId": parsed.request_id,
                    "answer": answer,
                }),
                issued_at: serde_json::Number::from(now_ms_i64()),
            };
            let ack = control
                .send_conversation_command_v4(&target, &envelope)
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(&serde_json::json!({ "accepted": true, "ack": ack }))
        }
        "getTaskSnapshot" | "getTaskSnapshotWithEtag" => {
            // TS getTaskSnapshot（modeTaskServiceAdapter :2494）：resume 快照 →
            // messageLimit 截断 → syncTaskIndexMeta 回源 → 返回 snapshot+meta；
            // WithEtag 变体（:2547）：sha256(JSON) 作 ETag，ifNoneMatch 命中返回
            // {snapshot:null, etag, notModified:true}（缓存协商）。
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
                #[serde(default)]
                message_limit: Option<u64>,
                #[serde(default)]
                thought_level: Option<String>,
                #[serde(default)]
                if_none_match: Option<String>,
            }
            let parsed: P = parse(object)?;
            let control = channel.control.clone().ok_or_else(|| {
                ChannelError::error(format!(
                    "mode-task channel: {command} requires the agent control plane (not assembled)"
                ))
            })?;
            let (workspace_path, workspace_identity) = resolve_task_workspace(
                repo,
                parsed.workspace_path.as_deref(),
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
            )?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            let mut snapshot: SessionStateSnapshot = control
                .resume_session(
                    &target,
                    agent_protocol::session_write::SessionResumeParams {
                        session_id: parsed.task_id.clone(),
                        workspace: None,
                        thought_level: parsed.thought_level,
                        mcp_servers: None,
                        tool_allowlist: None,
                        tool_denylist: None,
                        off_peak_tool_enabled: None,
                        dynamic_workflow_enabled: None,
                    },
                )
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            // messageLimit：截断 messages（保尾部——最新上下文优先）。
            if let Some(limit) = parsed.message_limit.filter(|limit| *limit > 0) {
                let total = snapshot.messages.len();
                if total > limit as usize {
                    snapshot.messages = snapshot.messages.split_off(total - limit as usize);
                }
            }
            // session owner 的快照刷新索引投影（用户手动标题由 sync 保留）。
            let meta = snapshot_to_meta(&snapshot)?;
            let indexed_meta = repo
                .sync_task_meta(task_index_repo::SyncTaskMetaParams {
                    meta: meta.clone(),
                    pinned: None,
                    archived: None,
                    deleted: None,
                    title_overridden: None,
                    searchable_text: None,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            let mut result = serde_json::to_value(&snapshot)
                .map_err(|error| ChannelError::error(error.to_string()))?;
            if let Ok(meta_value) = serde_json::to_value(&indexed_meta) {
                result["meta"] = meta_value;
            }
            if command == "getTaskSnapshotWithEtag" {
                use sha2::Digest as _;
                let text = serde_json::to_string(&result)
                    .map_err(|error| ChannelError::error(error.to_string()))?;
                let etag = format!("{:x}", sha2::Sha256::digest(text.as_bytes()));
                if parsed.if_none_match.as_deref() == Some(etag.as_str()) {
                    return json_value(&serde_json::json!({
                        "snapshot": null,
                        "etag": etag,
                        "notModified": true,
                    }));
                }
                return json_value(&serde_json::json!({
                    "snapshot": result,
                    "etag": etag,
                }));
            }
            json_value(&result)
        }
        "initialize" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let target = AgentWorkspaceTarget {
                workspace_path: parsed.workspace_path,
                workspace_identity: parsed.workspace_identity,
                remote_session_id: None,
            };
            let result = control.initialize(&target).await;
            // TS adapter：{ available, version: protocolName/protocolVersion }。
            json_value(&serde_json::json!({
                "available": result.available,
                "version": format!("{}/{}", result.protocol_name, result.protocol_version),
            }))
        }
        "releaseWorkspacePreparation" | "restartWorkspaceProcess" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let target = AgentWorkspaceTarget {
                workspace_path: parsed.workspace_path,
                workspace_identity: parsed.workspace_identity,
                remote_session_id: None,
            };
            // TS 两个入口都收敛到 modeAgentService.disposeWorkspace：关闭 workspace UI
            // 只释放 RPC 使用方，runtime 必须按 workspaceKey 显式回收（WSL Host 共享）。
            control
                .dispose_workspace(&target, DISPOSE_GRACE)
                .await;
            Ok(WireValue::Undefined)
        }
        "getTaskConfigOptions" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let (workspace_path, workspace_identity) = resolve_task_workspace(
                repo,
                parsed.workspace_path.as_deref(),
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
            )?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            let snapshot = resume_session_snapshot(control, &target, &parsed.task_id).await?;
            let options = crate::mode_config_options::settings_to_config_options(&snapshot.settings);
            json_value(&serde_json::Value::Array(options))
        }
        "getTaskModelSelection" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let (workspace_path, workspace_identity) = resolve_task_workspace(
                repo,
                parsed.workspace_path.as_deref(),
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
            )?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            let snapshot = control
                .read_session(&target, &parsed.task_id, None, None, None)
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            match snapshot.settings.model.current {
                Some(model) => json_value(
                    &serde_json::to_value(&model)
                        .map_err(|error| ChannelError::error(error.to_string()))?,
                ),
                None => json_value(&serde_json::Value::Null),
            }
        }
        "setMode" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                mode: String,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let (workspace_path, workspace_identity) = resolve_task_workspace(
                repo,
                parsed.workspace_path.as_deref(),
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
            )?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            let mode = mode_command_value(&parsed.mode, "setMode")?;
            send_host_cas_command_v4(
                control,
                &target,
                agent_protocol::v4::CommandType::SwitchCollaborationMode,
                serde_json::json!({ "mode": mode }),
                &parsed.task_id,
                &format!("setMode session={} mode={mode}", parsed.task_id),
            )
            .await?;
            let snapshot = resume_session_snapshot(control, &target, &parsed.task_id).await?;
            sync_snapshot_to_index(repo, &snapshot)?;
            Ok(WireValue::Undefined)
        }
        "setConfigOption" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                config_id: String,
                value: String,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let (workspace_path, workspace_identity) = resolve_task_workspace(
                repo,
                parsed.workspace_path.as_deref(),
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
            )?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            match parsed.config_id.as_str() {
                "model" => {
                    // TS 委托 service.setModel；Rust 直接走 setModel 的 v4 CAS 路径。
                    let (provider, model, level) =
                        crate::mode_config_options::parse_model_picker_value(&parsed.value)
                            .map_err(ChannelError::error)?;
                    control
                        .ensure_provider_config(&target)
                        .await
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    send_host_cas_command_v4(
                        control,
                        &target,
                        agent_protocol::v4::CommandType::SwitchModelConfig,
                        serde_json::json!({
                            "provider": provider,
                            "model": model,
                            "thought": level.as_deref().unwrap_or(""),
                        }),
                        &parsed.task_id,
                        &format!(
                            "setConfigOption model session={} value={}",
                            parsed.task_id, parsed.value
                        ),
                    )
                    .await?;
                }
                "thought_level" => {
                    // v4 无独立思考深度命令：provider/model 取当前会话选型，thought 字段承载。
                    let current = control
                        .read_session(&target, &parsed.task_id, None, None, None)
                        .await
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    let model = current
                        .settings
                        .model
                        .current
                        .ok_or_else(|| ChannelError::error("请先选择模型，再设置思考档位"))?;
                    send_host_cas_command_v4(
                        control,
                        &target,
                        agent_protocol::v4::CommandType::SwitchModelConfig,
                        serde_json::json!({
                            "provider": model.provider_id,
                            "model": model.model_id,
                            "thought": parsed.value,
                        }),
                        &parsed.task_id,
                        &format!(
                            "setConfigOption thought session={} value={}",
                            parsed.task_id, parsed.value
                        ),
                    )
                    .await?;
                }
                "mode" => {
                    let mode = mode_command_value(&parsed.value, "setConfigOption")?;
                    send_host_cas_command_v4(
                        control,
                        &target,
                        agent_protocol::v4::CommandType::SwitchCollaborationMode,
                        serde_json::json!({ "mode": mode }),
                        &parsed.task_id,
                        &format!("setConfigOption mode session={} mode={mode}", parsed.task_id),
                    )
                    .await?;
                }
                _ => {}
            }
            let snapshot = resume_session_snapshot(control, &target, &parsed.task_id).await?;
            sync_snapshot_to_index(repo, &snapshot)?;
            let options = crate::mode_config_options::settings_to_config_options(&snapshot.settings);
            json_value(&serde_json::Value::Array(options))
        }
        "setAutomationSessionConfig" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                model_selection: SelectionParam,
                #[serde(default)]
                thought_level: Option<String>,
                #[serde(default)]
                mode: Option<String>,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct SelectionParam {
                provider_id: String,
                model_id: String,
            }
            let parsed: P = parse(object)?;
            let (workspace_path, workspace_identity) = resolve_task_workspace(
                repo,
                parsed.workspace_path.as_deref(),
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
            )?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            let thought_level = parsed
                .thought_level
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            // 一条 V4 switchModelConfig 原子更新 runtime 与投影；thoughtLevel 非空时
            // 再以同模型命令显式收敛（跨模型切换会先采用目标模型默认档位，TS 同）。
            send_host_cas_command_v4(
                control,
                &target,
                agent_protocol::v4::CommandType::SwitchModelConfig,
                serde_json::json!({
                    "provider": parsed.model_selection.provider_id,
                    "model": parsed.model_selection.model_id,
                    "thought": thought_level,
                }),
                &parsed.task_id,
                &format!(
                    "automation session={} model={}/{} thought={thought_level}",
                    parsed.task_id,
                    parsed.model_selection.provider_id,
                    parsed.model_selection.model_id
                ),
            )
            .await?;
            if !thought_level.is_empty() {
                send_host_cas_command_v4(
                    control,
                    &target,
                    agent_protocol::v4::CommandType::SwitchModelConfig,
                    serde_json::json!({
                        "provider": parsed.model_selection.provider_id,
                        "model": parsed.model_selection.model_id,
                        "thought": thought_level,
                    }),
                    &parsed.task_id,
                    &format!(
                        "automation session={} thought={thought_level}",
                        parsed.task_id
                    ),
                )
                .await?;
            }
            if let Some(mode) = parsed.mode.as_deref().map(str::trim).filter(|mode| !mode.is_empty()) {
                let mode = mode_command_value(mode, "setAutomationSessionConfig")?;
                send_host_cas_command_v4(
                    control,
                    &target,
                    agent_protocol::v4::CommandType::SwitchCollaborationMode,
                    serde_json::json!({ "mode": mode }),
                    &parsed.task_id,
                    &format!(
                        "automation session={} mode={mode}",
                        parsed.task_id
                    ),
                )
                .await?;
            }
            let snapshot = resume_session_snapshot(control, &target, &parsed.task_id).await?;
            sync_snapshot_to_index(repo, &snapshot)?;
            let options = crate::mode_config_options::settings_to_config_options(&snapshot.settings);
            json_value(&serde_json::Value::Array(options))
        }
        "setModel" => {
            // TS setModel 仍走 legacy session/setModel（runtimeModel/凭据解析）；Rust 侧
            // 以「provider config 同步 + v4 CAS switchModelConfig + 快照回源」为同语义
            // 替代路径（CLI 负责最终 provider 凭据校验），返回 configOptions。
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                model_selection: SelectionParam,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct SelectionParam {
                provider_id: String,
                model_id: String,
                #[serde(default)]
                options: Option<ReasoningParam>,
            }
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct ReasoningParam {
                #[serde(default)]
                reasoning_level: Option<String>,
            }
            let parsed: P = parse(object)?;
            let (workspace_path, workspace_identity) = resolve_task_workspace(
                repo,
                parsed.workspace_path.as_deref(),
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
            )?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            control
                .ensure_provider_config(&target)
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            let thought = parsed
                .model_selection
                .options
                .as_ref()
                .and_then(|options| options.reasoning_level.as_deref())
                .unwrap_or("");
            send_host_cas_command_v4(
                control,
                &target,
                agent_protocol::v4::CommandType::SwitchModelConfig,
                serde_json::json!({
                    "provider": parsed.model_selection.provider_id,
                    "model": parsed.model_selection.model_id,
                    "thought": thought,
                }),
                &parsed.task_id,
                &format!(
                    "setModel session={} model={}/{} thought={thought}",
                    parsed.task_id,
                    parsed.model_selection.provider_id,
                    parsed.model_selection.model_id
                ),
            )
            .await?;
            let snapshot = resume_session_snapshot(control, &target, &parsed.task_id).await?;
            sync_snapshot_to_index(repo, &snapshot)?;
            let options = crate::mode_config_options::settings_to_config_options(&snapshot.settings);
            json_value(&serde_json::Value::Array(options))
        }
        "stopGeneration" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let (workspace_path, workspace_identity) = resolve_task_workspace(
                repo,
                parsed.workspace_path.as_deref(),
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
            )?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            // TS stopGeneration：session/stop → v4 stop（goal-pause barrier 由 CLI 承接）。
            let envelope = agent_protocol::v4::ConversationCommandEnvelope {
                ttft: None,
                command_id: uuid::Uuid::new_v4().to_string(),
                client_id: "mode-host".to_string(),
                session_id: agent_protocol::v4::CommandSessionId(Some(parsed.task_id.clone())),
                base_revision: None,
                base_log_epoch: None,
                command_type: agent_protocol::v4::CommandType::Stop,
                payload: serde_json::json!({}),
                issued_at: serde_json::Number::from(now_ms_i64()),
            };
            control
                .send_conversation_command_v4(&target, &envelope)
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            // TS `stopGeneration(): Promise<void>`。
            Ok(WireValue::Undefined)
        }
        "getTaskTokenUsage" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                task_id: String,
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = parse(object)?;
            let (workspace_path, workspace_identity) = resolve_task_workspace(
                repo,
                parsed.workspace_path.as_deref(),
                parsed.workspace_identity.as_deref(),
                &parsed.task_id,
            )?;
            let target = AgentWorkspaceTarget {
                workspace_path,
                workspace_identity,
                remote_session_id: None,
            };
            // TS getTaskTokenUsage：只读观察 v4/conversation/usage，不拉起 runtime。
            let usage = control
                .conversation_usage_v4(
                    &target,
                    agent_protocol::v4_queries::ConversationUsageParams {
                        session_id: parsed.task_id,
                    },
                    agent_process::RuntimePolicy::ExistingOnly,
                )
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(
                &serde_json::to_value(&usage)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "importClaudeSessions" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                #[serde(default)]
                workspace_path: Option<String>,
                #[serde(default)]
                workspace_identity: Option<String>,
                #[serde(default)]
                session_ids: Vec<String>,
            }
            let parsed: P = parse(object)?;
            let Some(data_root) = channel.data_root.as_ref() else {
                return Err(ChannelError::error(
                    "mode-task channel: importClaudeSessions requires data_root",
                ));
            };
            let mut seen = std::collections::HashSet::new();
            let session_ids: Vec<String> = parsed
                .session_ids
                .into_iter()
                .map(|session_id| session_id.trim().to_string())
                .filter(|session_id| !session_id.is_empty())
                .filter(|session_id| seen.insert(session_id.clone()))
                .collect();
            // 仅当显式 workspacePath 过滤时才绑定 identity（避免把当前 tab 的 identity
            // 套到其它路径的导入目标）。
            let target_identity = if parsed.workspace_path.is_some() {
                parsed.workspace_identity.as_deref()
            } else {
                None
            };
            let mut imported: Vec<serde_json::Value> = Vec::new();
            let mut skipped: Vec<serde_json::Value> = Vec::new();
            let mut failed: Vec<serde_json::Value> = Vec::new();
            for session_id in session_ids {
                match import_claude_session(
                    repo,
                    control,
                    data_root,
                    parsed.workspace_path.as_deref(),
                    target_identity,
                    &session_id,
                )
                .await
                {
                    ImportOutcome::Imported(meta) => {
                        channel.events.notify_task_list(&task_workspace_event(
                            &meta.workspace_path,
                            meta.workspace_identity.as_deref(),
                            &meta.task_id,
                            "task_meta_changed",
                            &meta,
                        ));
                        imported.push(serde_json::json!({
                            "provider": "claude",
                            "sessionId": session_id,
                            "taskId": meta.task_id,
                            "workspacePath": meta.workspace_path,
                        }));
                    }
                    ImportOutcome::Skipped {
                        reason,
                        workspace_path,
                    } => {
                        let mut value = serde_json::json!({
                            "provider": "claude",
                            "sessionId": session_id,
                            "reason": reason,
                        });
                        if let Some(workspace_path) = workspace_path {
                            value["workspacePath"] = serde_json::json!(workspace_path);
                        }
                        skipped.push(value);
                    }
                    ImportOutcome::Failed {
                        reason,
                        workspace_path,
                    } => {
                        let mut value = serde_json::json!({
                            "provider": "claude",
                            "sessionId": session_id,
                            "reason": reason,
                        });
                        if let Some(workspace_path) = workspace_path {
                            value["workspacePath"] = serde_json::json!(workspace_path);
                        }
                        failed.push(value);
                    }
                }
            }
            json_value(&serde_json::json!({
                "imported": imported,
                "skipped": skipped,
                "failed": failed,
            }))
        }
        other => Err(ChannelError::error(format!(
            "mode-task channel: unknown orchestration command {other}"
        ))),
    }
}

/// `session/resume` 只读快照（TS `resumeSnapshot`：settings/configOptions 回源）。
async fn resume_session_snapshot(
    control: &AgentControlClient,
    target: &AgentWorkspaceTarget,
    task_id: &str,
) -> Result<SessionStateSnapshot, ChannelError> {
    control
        .resume_session(
            target,
            agent_protocol::session_write::SessionResumeParams {
                session_id: task_id.to_string(),
                workspace: None,
                thought_level: None,
                mcp_servers: None,
                tool_allowlist: None,
                tool_denylist: None,
                off_peak_tool_enabled: None,
                dynamic_workflow_enabled: None,
            },
        )
        .await
        .map_err(|error| ChannelError::error(error.to_string()))
}

/// TS `syncTaskIndexSnapshot`（设置面写后回写索引投影）。
fn sync_snapshot_to_index(
    repo: &TaskIndexRepo,
    snapshot: &SessionStateSnapshot,
) -> Result<(), ChannelError> {
    let meta = snapshot_to_meta(snapshot)?;
    repo.sync_task_meta(task_index_repo::SyncTaskMetaParams {
        meta,
        pinned: None,
        archived: None,
        deleted: None,
        title_overridden: None,
        searchable_text: None,
    })
    .map_err(|error| ChannelError::error(error.to_string()))?;
    Ok(())
}

/// TS `toModeMode(...) ?? "build"`；`auto` 仍依赖 legacy session/setMode，显式报错。
fn mode_command_value(mode: &str, command: &str) -> Result<&'static str, ChannelError> {
    match mode {
        "build" | "autoEdit" => Ok("build"),
        "edit" => Ok("edit"),
        "plan" => Ok("plan"),
        "yolo" => Ok("yolo"),
        "auto" => Err(ChannelError::error(format!(
            "{command}(auto) 需要 legacy session/setMode，尚未迁移"
        ))),
        _ => Ok("build"),
    }
}

/// TS `sendHostCasCommandV4`：首发 baseRevision=0 探测，stale 用 revisionAtDecision
/// 收敛重试（每次新 commandId），最多 4 次。
async fn send_host_cas_command_v4(
    control: &AgentControlClient,
    target: &AgentWorkspaceTarget,
    command_type: agent_protocol::v4::CommandType,
    payload: serde_json::Value,
    session_id: &str,
    context: &str,
) -> Result<(), ChannelError> {
    use agent_protocol::v4::CommandAckStatus;

    let mut base_revision = serde_json::Number::from(0);
    let mut last_status = String::new();
    for _ in 0..4 {
        let envelope = agent_protocol::v4::ConversationCommandEnvelope {
            ttft: None,
            command_id: uuid::Uuid::new_v4().to_string(),
            client_id: "mode-host".to_string(),
            session_id: agent_protocol::v4::CommandSessionId(Some(session_id.to_string())),
            base_revision: Some(base_revision.clone()),
            base_log_epoch: None,
            command_type,
            payload: payload.clone(),
            issued_at: serde_json::Number::from(now_ms_i64()),
        };
        let ack = control
            .send_conversation_command_v4(target, &envelope)
            .await
            .map_err(|error| ChannelError::error(error.to_string()))?;
        if ack.status == CommandAckStatus::Stale {
            base_revision = ack.revision_at_decision.clone();
            last_status = ack.status.as_str().to_string();
            continue;
        }
        if ack.status.is_success() {
            return Ok(());
        }
        let reason = ack.reason_code.unwrap_or_default();
        let message = ack.message.unwrap_or_default();
        return Err(ChannelError::error(format!(
            "v4 command {} ({reason}) {message} — {context}",
            ack.status.as_str()
        )));
    }
    Err(ChannelError::error(format!(
        "v4 command 连续 stale，放弃重试（last={last_status}）— {context}"
    )))
}

enum ImportOutcome {
    Imported(Box<ModeTaskMeta>),
    Skipped {
        reason: &'static str,
        workspace_path: Option<String>,
    },
    Failed {
        reason: String,
        workspace_path: Option<String>,
    },
}

fn ch_error_text(error: ChannelError) -> String {
    match error {
        ChannelError::Error { message, .. } => message,
        other => format!("{other:?}"),
    }
}

fn cleanup_copied_file(created: bool, path: &std::path::Path) {
    if created {
        let _ = std::fs::remove_file(path);
    }
}

/// 单个 Claude session 导入：查找 → 复制 → 解析 → createSession(importedHistory)
/// → legacy 快照 → 索引回写。任一步失败清理本次新建的副本（已创建 session 不回滚，TS 同）。
async fn import_claude_session(
    repo: &TaskIndexRepo,
    control: &AgentControlClient,
    data_root: &std::path::Path,
    workspace_filter: Option<&str>,
    target_identity: Option<&str>,
    session_id: &str,
) -> ImportOutcome {
    use crate::claude_import_persist as persist;
    use agent_protocol::session_create::{
        ClaudeCodeImport, ClaudeCodeSource, ImportHistory, ImportMessage, ImportMessageRole,
    };
    use agent_protocol::session_write::SessionPersistence;

    let Some(candidate) = crate::claude_import_persist::find_importable_session(workspace_filter, session_id)
    else {
        return ImportOutcome::Skipped {
            reason: "session_not_found_or_workspace_mismatch",
            workspace_path: workspace_filter.map(str::to_string),
        };
    };
    let workspace_path = candidate.workspace_path.clone();
    if std::fs::metadata(&workspace_path).is_err() {
        return ImportOutcome::Skipped {
            reason: "workspace_path_missing",
            workspace_path: Some(workspace_path),
        };
    }
    if let Some(filter) = workspace_filter {
        if crate::claude_import::normalize_path_for_comparison(&workspace_path)
            != crate::claude_import::normalize_path_for_comparison(filter)
        {
            return ImportOutcome::Skipped {
                reason: "session_not_found_or_workspace_mismatch",
                workspace_path: Some(workspace_path),
            };
        }
    }

    let (copied_path, created) = match persist::copy_session_file(
        data_root,
        &workspace_path,
        target_identity,
        &candidate.source_path,
    ) {
        Ok(result) => result,
        Err(reason) => {
            return ImportOutcome::Failed {
                reason,
                workspace_path: Some(workspace_path),
            }
        }
    };

    let source = match persist::parse_session_file(
        &candidate.source_path,
        &workspace_path,
        session_id,
        Some(&candidate.source_path),
        candidate.created_at,
        Some(candidate.updated_at),
    ) {
        Ok(source) => source,
        Err(reason) => {
            cleanup_copied_file(created, &copied_path);
            return ImportOutcome::Failed {
                reason,
                workspace_path: Some(workspace_path),
            };
        }
    };

    let target = AgentWorkspaceTarget {
        workspace_path: source.workspace_path.clone(),
        workspace_identity: target_identity.map(str::to_string),
        remote_session_id: None,
    };
    let imported_history = ImportHistory::ClaudeCode(ClaudeCodeImport {
        source: ClaudeCodeSource::ClaudeCode,
        title: source.title.clone(),
        created_at: Some(source.created_at.max(0) as u64),
        updated_at: Some(source.updated_at.max(0) as u64),
        messages: source
            .messages
            .iter()
            .map(|message| ImportMessage {
                role: if message.role == "assistant" {
                    ImportMessageRole::Assistant
                } else {
                    ImportMessageRole::User
                },
                content: message.content.clone(),
                timestamp: Some(message.timestamp.max(0) as u64),
            })
            .collect(),
    });
    let snapshot = match control
        .create_session(
            &target,
            agent_process::CreateSessionOptions {
                session_id: Some(persist::build_imported_task_id(
                    &source.workspace_path,
                    session_id,
                )),
                persistence: Some(SessionPersistence::Immediate),
                imported_history: Some(imported_history),
                ..Default::default()
            },
        )
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            cleanup_copied_file(created, &copied_path);
            return ImportOutcome::Failed {
                reason: error.to_string(),
                workspace_path: Some(workspace_path),
            };
        }
    };
    let meta = match snapshot_to_meta(&snapshot) {
        Ok(meta) => meta,
        Err(error) => {
            cleanup_copied_file(created, &copied_path);
            return ImportOutcome::Failed {
                reason: ch_error_text(error),
                workspace_path: Some(workspace_path),
            };
        }
    };

    // legacy 快照：过滤来源运行态（meta.mode/model/provider、messages[].model）。
    let mut task_file = match persist::build_task_file(&source, Some(&meta.task_id)) {
        Ok(file) => file,
        Err(reason) => {
            cleanup_copied_file(created, &copied_path);
            return ImportOutcome::Failed {
                reason,
                workspace_path: Some(workspace_path),
            };
        }
    };
    if let Some(identity) = target_identity {
        task_file["meta"]["workspaceIdentity"] = serde_json::json!(identity);
    }
    if let Err(reason) = persist::write_legacy_snapshot(
        data_root,
        &meta.workspace_path,
        target_identity,
        &meta.task_id,
        &task_file,
    ) {
        cleanup_copied_file(created, &copied_path);
        return ImportOutcome::Failed {
            reason,
            workspace_path: Some(workspace_path),
        };
    }

    let mut index_meta = meta.clone();
    index_meta.migration_source = Some("claudeCode".to_string());
    if let Some(identity) = target_identity {
        index_meta.workspace_identity = Some(identity.to_string());
    }
    let synced = match repo.sync_task_meta(task_index_repo::SyncTaskMetaParams {
        meta: index_meta,
        pinned: None,
        // 重新导入要把会话恢复到列表：显式取消旧的隐藏状态。
        archived: Some(false),
        deleted: Some(false),
        title_overridden: None,
        searchable_text: Some(persist::build_searchable_text(&source.messages)),
    }) {
        Ok(meta) => meta,
        Err(error) => {
            cleanup_copied_file(created, &copied_path);
            return ImportOutcome::Failed {
                reason: error.to_string(),
                workspace_path: Some(workspace_path),
            };
        }
    };
    ImportOutcome::Imported(Box::new(synced))
}

impl ServerChannel for ModeTaskChannel {
    fn listen(
        &self,
        _ctx: String,
        event: String,
        arg: WireValue,
        emit: rpc_host::EventCallback,
    ) -> Result<rpc_host::Subscription, ChannelError> {
        match event.as_str() {
            ON_DYNAMIC_WORKSPACE_EVENT => Ok(self
                .events
                .subscribe_task_list(parse_workspace_event_scope(&arg), emit)),
            ON_DYNAMIC_TASK_READY => Ok(self.events.subscribe_ready(parse_task_id_arg(&arg), emit)),
            ON_DYNAMIC_TASK_TERMINAL_OUTCOME => Ok(self
                .events
                .subscribe_terminal(parse_task_id_arg(&arg), emit)),
            other => Err(ChannelError::error(format!(
                "mode-task channel: unknown event {other}"
            ))),
        }
    }

    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: WireValue,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<WireValue, ChannelError>> {
        let repo = Arc::clone(&self.repo);
        let events = Arc::clone(&self.events);
        let control = self.control.clone();
        let channel = ModeTaskChannel {
            repo: Arc::clone(&self.repo),
            control: self.control.clone(),
            queue: self.queue.clone(),
            events: Arc::clone(&self.events),
            data_root: self.data_root.clone(),
        };
        Box::pin(async move {
            match command.as_str() {
                // 编排面（async：AgentControlClient 会话调用）。
                "createTask" | "closeTask" | "resumeTask" | "sendPrompt"
                | "enqueueTaskCommand" | "promoteTaskCommand" | "cancelTaskCommand"
                | "respondPermission" | "respondElicitation"
                | "getTaskSnapshot" | "getTaskSnapshotWithEtag"
                | "stopGeneration" | "getTaskTokenUsage"
                | "releaseWorkspacePreparation" | "restartWorkspaceProcess"
                | "initialize" | "importClaudeSessions"
                | "getTaskConfigOptions" | "getTaskModelSelection"
                | "setMode" | "setConfigOption" | "setAutomationSessionConfig"
                | "setModel" => {
                    let object = crate::params::first_arg_object(&arg)?;
                    let control = control.ok_or_else(|| {
                        ChannelError::error(format!(
                            "mode-task channel: {command} requires the agent control plane (not assembled)"
                        ))
                    })?;
                    run_orchestration(&channel, &repo, &control, &command, object).await
                }
                // 读面（同步 repo，spawn_blocking 防阻塞 runtime）。
                _ => tokio::task::spawn_blocking(move || {
                    run_read(&repo, &events, &command, &arg)
                })
                    .await
                    .map_err(|error| {
                        ChannelError::error(format!("mode-task channel task failed: {error}"))
                    })?,
            }
        })
    }
}
