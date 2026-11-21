//! `window-controller` 频道适配器（LT-16.3：local 单源列表 + 变更面 + V4 帧订阅）。
//!
//! 真源：`packages/services/src/window-controller/windowController.ts` 的
//! `IWindowControllerService` 与 `apps/desktop/src/host-standalone/windowController/`
//! 的 Host 实现。TS 实现是多源投影（local + remote 的 taskService/agentService
//! 聚合 + sessions-index observer）；Rust 壳内只做 **local 单源**：
//! - `listTaskList`：任务索引查询 → items（sourceAvailability 恒 online，
//!   liveStatus 按 TS `defaultLiveStatus` 仅投影 completed/error，其余 idle）。
//! - `mutateTask`：pin/archive/delete/mark-read/mark-unread → 任务索引 patch；
//!   open/resume 只做地址校验（Controller 职责是验证唯一 source，TS 同语义）。
//! - `deleteArchivedTask(s)`：归档删除（单条/批量）。
//! - controller 帧订阅：`subscribeControllerV4`/`resyncControllerV4`/
//!   `unsubscribeControllerV4` + `onDynamicControllerFrame` 事件。两 topic 各自
//!   logEpoch 与单调 seq；订阅/重同步推 snapshot 帧，refresh 的结构 diff 推
//!   delta 帧（fromSeq 恒等于上一帧 toSeq）。壳内没有 sessions-index observer/
//!   DB 变更观察者，refresh 触发点暂为「本频道每次读写调用」——与 TS 事件驱动
//!   refresh 的差距登记在 spec `window-host-controller-service.md`。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
use rpc_wire::Value as WireValue;
use task_index_repo::{
    ListTaskMetasParams, ModeTaskMeta, TaskIndexRepo, TaskIndexStatePatch, TaskListKind,
    TaskListQuery, TaskListSortBy, TaskListWorkspaceScope, TaskRef,
};

pub const WINDOW_CONTROLLER_CHANNEL: &str = "window-controller";
pub const ON_DYNAMIC_CONTROLLER_FRAME: &str = "onDynamicControllerFrame";
pub const CONTROLLER_TASKS_INDEX_TOPIC: &str = "controller/tasks-index";
pub const CONTROLLER_WORKSPACES_TOPIC: &str = "controller/workspaces";

pub struct WindowControllerChannel {
    repo: Arc<TaskIndexRepo>,
    /// V4 controller 帧总线（订阅注册 + snapshot/delta 广播）。
    frames: Arc<ControllerFrameBus>,
    /// mode-task 任务事件桥（进程内）：创建/关闭等索引写入后刷新投影。
    /// 仅构造方持有（per-call 克隆为 `None`，不重复订阅）。
    _task_event_bridge: Option<Subscription>,
}

impl WindowControllerChannel {
    /// 独立频道（无跨频道事件桥）：refresh 只由本频道调用触发。
    pub fn new(repo: Arc<TaskIndexRepo>) -> Self {
        let frames = Arc::new(ControllerFrameBus::new(Arc::clone(&repo)));
        Self {
            repo,
            frames,
            _task_event_bridge: None,
        }
    }

    /// 装配用：订阅同一 server 的 mode-task 事件总线（TS Host 进程内 Emitter 等价），
    /// `onWorkspaceTaskListChanged` 到达时刷新 controller 投影并广播 delta 帧。
    pub fn with_task_events(
        repo: Arc<TaskIndexRepo>,
        task_events: Arc<crate::mode_task::TaskEventBus>,
    ) -> Self {
        let frames = Arc::new(ControllerFrameBus::new(Arc::clone(&repo)));
        let bridge_frames = Arc::clone(&frames);
        let bridge = task_events.subscribe_local(Arc::new(
            move |_kind: &str, _meta: Option<&JsonValue>| {
                // 索引已在事件源侧提交；这里只把本地投影与新行做 diff 并广播。
                let _ = bridge_frames.refresh();
            },
        ));
        Self {
            repo,
            frames,
            _task_event_bridge: Some(bridge),
        }
    }

    /// per-call 克隆：共享 repo/frames，但不持有事件桥订阅。
    fn handle(&self) -> Self {
        Self {
            repo: Arc::clone(&self.repo),
            frames: Arc::clone(&self.frames),
            _task_event_bridge: None,
        }
    }
}

/// TS `WindowHostTaskAddress`。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskAddress {
    task_id: String,
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
    #[serde(default)]
    remote_session_id: Option<String>,
}

impl TaskAddress {
    fn into_ref(self) -> TaskRef {
        TaskRef {
            workspace_path: self.workspace_path,
            workspace_identity: self.workspace_identity,
            task_id: self.task_id,
        }
    }
}

/// TS `WindowHostControllerMutation` 的 tag 统一表示。
#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
enum Mutation {
    Pin {
        pinned: bool,
    },
    Archive {
        archived: bool,
    },
    Delete,
    DeleteArchived,
    DeleteArchivedBatch {
        task_ids: Vec<String>,
    },
    MarkRead {
        #[serde(default)]
        expected_unread_at: Option<i64>,
    },
    MarkUnread,
    Open,
    Resume,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskListItem {
    #[serde(flatten)]
    meta: serde_json::Value,
    source_availability: &'static str,
    live_status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskListResult {
    items: Vec<TaskListItem>,
    total: usize,
    has_more: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchivedDeletionResult {
    deleted_task_ids: Vec<String>,
    skipped_task_ids: Vec<String>,
    failed_task_ids: Vec<String>,
}

/// TS `defaultLiveStatus`：没有 sessions-index overlay 时，SQLite 里的
/// running/waiting 可能是上次进程退出残留，不能重新解释成当前运行/等待事实；
/// 只有 completed/error 是持久终态，其余一律 idle。
fn default_live_status(meta: &ModeTaskMeta) -> &'static str {
    match meta.status.as_deref() {
        Some("completed") => "completed",
        Some("error") => "error",
        _ => "idle",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControllerTopic {
    TasksIndex,
    Workspaces,
}

impl ControllerTopic {
    fn parse(value: &str) -> Option<Self> {
        match value {
            CONTROLLER_TASKS_INDEX_TOPIC => Some(Self::TasksIndex),
            CONTROLLER_WORKSPACES_TOPIC => Some(Self::Workspaces),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::TasksIndex => CONTROLLER_TASKS_INDEX_TOPIC,
            Self::Workspaces => CONTROLLER_WORKSPACES_TOPIC,
        }
    }
}

struct TopicState {
    log_epoch: String,
    seq: u64,
}

impl TopicState {
    fn new() -> Self {
        Self {
            log_epoch: uuid::Uuid::new_v4().to_string(),
            seq: 0,
        }
    }
}

#[derive(Clone)]
struct TaskRowRecord {
    key: String,
    updated_at: i64,
    json: JsonValue,
}

struct ControllerFrameState {
    tasks: TopicState,
    workspaces: TopicState,
    subscribers: HashMap<String, ControllerTopic>,
    /// 上次发布的快照，用于计算 delta（TS `buildSourceRows` 结果的结构 diff）。
    last_tasks: BTreeMap<String, TaskRowRecord>,
    last_workspaces: BTreeMap<String, JsonValue>,
}

impl ControllerFrameState {
    fn new() -> Self {
        Self {
            tasks: TopicState::new(),
            workspaces: TopicState::new(),
            subscribers: HashMap::new(),
            last_tasks: BTreeMap::new(),
            last_workspaces: BTreeMap::new(),
        }
    }

    fn topic_state(&self, topic: ControllerTopic) -> &TopicState {
        match topic {
            ControllerTopic::TasksIndex => &self.tasks,
            ControllerTopic::Workspaces => &self.workspaces,
        }
    }
}

/// controller 帧总线：TS `windowHostControllerProjection` 的 subscribe/resync/
/// publish 语义（local 单源）。事件监听者相当于 TS 的帧 emitter——帧按
/// subscriptionId 寻址，客户端自行按 cursor 过滤。
struct ControllerFrameBus {
    repo: Arc<TaskIndexRepo>,
    state: Mutex<ControllerFrameState>,
    listeners: Arc<Mutex<HashMap<String, EventCallback>>>,
}

impl ControllerFrameBus {
    fn new(repo: Arc<TaskIndexRepo>) -> Self {
        Self {
            repo,
            state: Mutex::new(ControllerFrameState::new()),
            listeners: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 注册 `onDynamicControllerFrame` 事件监听者。
    fn subscribe_listener(&self, emit: EventCallback) -> Subscription {
        let id = uuid::Uuid::new_v4().to_string();
        self.listeners
            .lock()
            .expect("controller frame listeners poisoned")
            .insert(id.clone(), emit);
        let listeners = Arc::clone(&self.listeners);
        Box::new(move || {
            listeners
                .lock()
                .expect("controller frame listeners poisoned")
                .remove(&id);
        })
    }

    fn emit(&self, frame: &JsonValue) {
        let Ok(text) = serde_json::to_string(frame) else {
            return;
        };
        // mode_task 的事件总线同款：快照监听者集合后锁外发送，避免回调重入死锁。
        let callbacks: Vec<EventCallback> = self
            .listeners
            .lock()
            .expect("controller frame listeners poisoned")
            .values()
            .cloned()
            .collect();
        for emit in callbacks {
            emit(WireValue::Json(text.clone()));
        }
    }

    /// 重新读取索引并与上次发布的快照做结构 diff；有变化才推进 seq 并广播
    /// delta 帧（TS `publishTaskDeltas`/`publishWorkspaceDeltas` 同语义：
    /// 即使没有订阅者也要推进 seq）。
    fn refresh(&self) -> Result<(), String> {
        let (next_tasks, next_workspaces) = collect_frame_rows(&self.repo)?;
        let mut frames: Vec<JsonValue> = Vec::new();
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "controller frame state poisoned".to_string())?;
            let task_deltas = diff_tasks(&state.last_tasks, &next_tasks);
            if !task_deltas.is_empty() {
                let from_seq = state.tasks.seq;
                state.tasks.seq += 1;
                let to_seq = state.tasks.seq;
                let log_epoch = state.tasks.log_epoch.clone();
                let sent_at = now_ms();
                for (subscription_id, topic) in &state.subscribers {
                    if *topic == ControllerTopic::TasksIndex {
                        frames.push(delta_frame(
                            subscription_id,
                            ControllerTopic::TasksIndex,
                            &log_epoch,
                            from_seq,
                            to_seq,
                            sent_at,
                            task_deltas.clone(),
                        ));
                    }
                }
                state.last_tasks = next_tasks;
            }
            let workspace_deltas = diff_workspaces(&state.last_workspaces, &next_workspaces);
            if !workspace_deltas.is_empty() {
                let from_seq = state.workspaces.seq;
                state.workspaces.seq += 1;
                let to_seq = state.workspaces.seq;
                let log_epoch = state.workspaces.log_epoch.clone();
                let sent_at = now_ms();
                for (subscription_id, topic) in &state.subscribers {
                    if *topic == ControllerTopic::Workspaces {
                        frames.push(delta_frame(
                            subscription_id,
                            ControllerTopic::Workspaces,
                            &log_epoch,
                            from_seq,
                            to_seq,
                            sent_at,
                            workspace_deltas.clone(),
                        ));
                    }
                }
                state.last_workspaces = next_workspaces;
            }
        }
        for frame in &frames {
            self.emit(frame);
        }
        Ok(())
    }

    /// TS `projection.subscribe`：注册订阅并立即向该订阅推送 snapshot 帧。
    fn subscribe(&self, topic: ControllerTopic) -> Result<JsonValue, String> {
        let (ack, frame) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "controller frame state poisoned".to_string())?;
            let subscription_id = uuid::Uuid::new_v4().to_string();
            state.subscribers.insert(subscription_id.clone(), topic);
            let log_epoch = state.topic_state(topic).log_epoch.clone();
            let ack = serde_json::json!({
                "subscriptionId": subscription_id,
                "mode": "snapshot",
                "logEpoch": log_epoch,
            });
            let frame = snapshot_frame(&state, &subscription_id, topic, now_ms());
            (ack, frame)
        };
        self.emit(&frame);
        Ok(ack)
    }

    /// TS `projection.resync`：未知 subscriptionId 显式报错，已知则重推 snapshot。
    fn resync(&self, subscription_id: &str) -> Result<JsonValue, String> {
        let (ack, frame) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "controller frame state poisoned".to_string())?;
            let topic = *state.subscribers.get(subscription_id).ok_or_else(|| {
                format!("未找到 Controller subscription，subscriptionId={subscription_id}")
            })?;
            let log_epoch = state.topic_state(topic).log_epoch.clone();
            let ack = serde_json::json!({
                "subscriptionId": subscription_id,
                "mode": "snapshot",
                "logEpoch": log_epoch,
            });
            let frame = snapshot_frame(&state, subscription_id, topic, now_ms());
            (ack, frame)
        };
        self.emit(&frame);
        Ok(ack)
    }

    /// TS `projection.unsubscribe`：幂等移除。
    fn unsubscribe(&self, subscription_id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.subscribers.remove(subscription_id);
        }
    }
}

/// local 单源的投影行：TS `readSourceTaskIndex` 三列表合并。
/// archived 行按 TS 固定 `membership.pinned = false`（即使 DB pinned 标志为真）。
type FrameRows = (BTreeMap<String, TaskRowRecord>, BTreeMap<String, JsonValue>);

fn collect_frame_rows(repo: &TaskIndexRepo) -> Result<FrameRows, String> {
    let mut tasks: BTreeMap<String, TaskRowRecord> = BTreeMap::new();
    for (pinned, archived) in [(false, false), (true, false)] {
        for meta in list_metas(repo, Some(pinned), Some(archived))? {
            let record = task_row_record(&meta, pinned, archived);
            tasks.insert(record.key.clone(), record);
        }
    }
    for meta in list_metas(repo, None, Some(true))? {
        let record = task_row_record(&meta, false, true);
        tasks.insert(record.key.clone(), record);
    }
    let workspaces = collect_workspace_facts(&tasks);
    Ok((tasks, workspaces))
}

fn list_metas(
    repo: &TaskIndexRepo,
    pinned: Option<bool>,
    archived: Option<bool>,
) -> Result<Vec<ModeTaskMeta>, String> {
    repo.list_task_metas(&ListTaskMetasParams {
        pinned,
        archived,
        ..ListTaskMetasParams::default()
    })
    .map_err(|error| error.to_string())
}

fn workspace_identity_key(identity: Option<&str>, workspace_path: &str) -> String {
    identity
        .map(str::trim)
        .filter(|identity| !identity.is_empty())
        .unwrap_or(workspace_path)
        .to_string()
}

fn task_row_record(meta: &ModeTaskMeta, pinned: bool, archived: bool) -> TaskRowRecord {
    let key = format!(
        "local\u{0}{}\u{0}{}",
        workspace_identity_key(meta.workspace_identity.as_deref(), &meta.workspace_path),
        meta.task_id
    );
    let mut address = serde_json::json!({
        "workspacePath": meta.workspace_path,
        "taskId": meta.task_id,
    });
    if let Some(identity) = &meta.workspace_identity {
        address["workspaceIdentity"] = serde_json::json!(identity);
    }
    let meta_json = serde_json::to_value(meta).unwrap_or(JsonValue::Null);
    TaskRowRecord {
        key,
        updated_at: meta.updated_at,
        json: serde_json::json!({
            "address": address,
            "meta": meta_json,
            "membership": {
                "pinned": pinned,
                "archived": archived,
                "active": !archived,
            },
            "sourceAvailability": "online",
            "liveStatus": default_live_status(meta),
        }),
    }
}

/// workspace facts 由索引中出现的 workspace 去重派生（local 单源恒 online/online）。
/// 空 workspace 不会出现——壳内当前没有 source 注册面（TS 由注册表驱动）。
fn collect_workspace_facts(tasks: &BTreeMap<String, TaskRowRecord>) -> BTreeMap<String, JsonValue> {
    let mut workspaces: BTreeMap<String, JsonValue> = BTreeMap::new();
    for record in tasks.values() {
        let meta = &record.json["meta"];
        let Some(workspace_path) = meta["workspacePath"].as_str() else {
            continue;
        };
        let identity = meta["workspaceIdentity"].as_str();
        let key = format!("local\u{0}{}", identity.unwrap_or(workspace_path));
        let mut fact = serde_json::json!({
            "workspacePath": workspace_path,
            "sourceAvailability": "online",
            "connectionState": "online",
        });
        if let Some(identity) = identity {
            fact["workspaceIdentity"] = serde_json::json!(identity);
        }
        workspaces.insert(key, fact);
    }
    workspaces
}

fn diff_tasks(
    previous: &BTreeMap<String, TaskRowRecord>,
    next: &BTreeMap<String, TaskRowRecord>,
) -> Vec<JsonValue> {
    let mut deltas = Vec::new();
    for (key, record) in previous {
        if !next.contains_key(key) {
            deltas.push(serde_json::json!({
                "op": "task.removed",
                "address": record.json["address"].clone(),
            }));
        }
    }
    for (key, record) in next {
        match previous.get(key) {
            Some(previous_record) if previous_record.json == record.json => {}
            _ => deltas.push(serde_json::json!({
                "op": "task.upserted",
                "task": record.json.clone(),
            })),
        }
    }
    deltas
}

fn diff_workspaces(
    previous: &BTreeMap<String, JsonValue>,
    next: &BTreeMap<String, JsonValue>,
) -> Vec<JsonValue> {
    let mut deltas = Vec::new();
    for (key, fact) in previous {
        if next.contains_key(key) {
            continue;
        }
        let mut delta = serde_json::json!({
            "op": "workspace.removed",
            "workspacePath": fact["workspacePath"].clone(),
        });
        for field in ["workspaceIdentity", "remoteSessionId"] {
            if !fact[field].is_null() {
                delta[field] = fact[field].clone();
            }
        }
        deltas.push(delta);
    }
    for (key, fact) in next {
        match previous.get(key) {
            Some(previous_fact) if previous_fact == fact => {}
            _ => deltas.push(serde_json::json!({
                "op": "workspace.upserted",
                "workspace": fact.clone(),
            })),
        }
    }
    deltas
}

fn snapshot_frame(
    state: &ControllerFrameState,
    subscription_id: &str,
    topic: ControllerTopic,
    sent_at: i64,
) -> JsonValue {
    match topic {
        ControllerTopic::TasksIndex => {
            // TS `allRows`：updatedAt 降序，同值按 taskKey 升序。
            let mut rows: Vec<&TaskRowRecord> = state.last_tasks.values().collect();
            rows.sort_by(|left, right| {
                right
                    .updated_at
                    .cmp(&left.updated_at)
                    .then_with(|| left.key.cmp(&right.key))
            });
            let tasks: Vec<JsonValue> = rows.into_iter().map(|row| row.json.clone()).collect();
            serde_json::json!({
                "topic": ControllerTopic::TasksIndex.as_str(),
                "subscriptionId": subscription_id,
                "logEpoch": state.tasks.log_epoch,
                "fromSeq": 0,
                "toSeq": state.tasks.seq,
                "sentAt": sent_at,
                "payload": {
                    "kind": "snapshot",
                    "snapshot": {
                        "protocolVersion": 1,
                        "logEpoch": state.tasks.log_epoch,
                        "tasks": tasks,
                    },
                },
            })
        }
        ControllerTopic::Workspaces => {
            let workspaces: Vec<JsonValue> = state.last_workspaces.values().cloned().collect();
            serde_json::json!({
                "topic": ControllerTopic::Workspaces.as_str(),
                "subscriptionId": subscription_id,
                "logEpoch": state.workspaces.log_epoch,
                "fromSeq": 0,
                "toSeq": state.workspaces.seq,
                "sentAt": sent_at,
                "payload": {
                    "kind": "snapshot",
                    "snapshot": {
                        "protocolVersion": 1,
                        "logEpoch": state.workspaces.log_epoch,
                        "workspaces": workspaces,
                    },
                },
            })
        }
    }
}

fn delta_frame(
    subscription_id: &str,
    topic: ControllerTopic,
    log_epoch: &str,
    from_seq: u64,
    to_seq: u64,
    sent_at: i64,
    deltas: Vec<JsonValue>,
) -> JsonValue {
    serde_json::json!({
        "topic": topic.as_str(),
        "subscriptionId": subscription_id,
        "logEpoch": log_epoch,
        "fromSeq": from_seq,
        "toSeq": to_seq,
        "sentAt": sent_at,
        "payload": {
            "kind": "deltas",
            "deltas": deltas,
        },
    })
}

fn run(
    channel: &WindowControllerChannel,
    command: &str,
    arg: &WireValue,
) -> Result<WireValue, ChannelError> {
    let object = crate::params::first_arg_object(arg)?;
    let parse = |object: serde_json::Map<String, serde_json::Value>| {
        serde_json::from_value::<serde_json::Value>(serde_json::Value::Object(object))
            .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
    };
    let _ = parse;
    match command {
        "listTaskList" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                kind: String,
                #[serde(default)]
                workspace_scopes: Vec<ScopeParam>,
                sort_by: String,
                #[serde(default)]
                search: Option<String>,
                #[serde(default)]
                limit: Option<f64>,
            }
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct ScopeParam {
                workspace_path: String,
                #[serde(default)]
                workspace_identity: Option<String>,
            }
            let parsed: P = serde_json::from_value(serde_json::Value::Object(object))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            // 壳内无 DB 观察者：每次读取前刷新投影，让其他频道写入的索引变更
            // 通过 delta 帧收敛（TS 由 workspace 事件触发 refresh）。
            channel.frames.refresh().map_err(ChannelError::error)?;
            let kind = match parsed.kind.as_str() {
                "pinned" => TaskListKind::Pinned,
                "archived" => TaskListKind::Archived,
                "timeline" => TaskListKind::Timeline,
                _ => TaskListKind::Active,
            };
            let sort_by = match parsed.sort_by.as_str() {
                "created" => TaskListSortBy::Created,
                _ => TaskListSortBy::Updated,
            };
            let result = channel
                .repo
                .query_task_list(&TaskListQuery {
                    kind,
                    workspace_scopes: parsed
                        .workspace_scopes
                        .into_iter()
                        .map(|scope| TaskListWorkspaceScope {
                            workspace_path: scope.workspace_path,
                            workspace_identity: scope.workspace_identity,
                            workspace_purpose: None,
                        })
                        .collect(),
                    sort_by,
                    search: parsed.search,
                    provider: None,
                    limit: parsed.limit,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            let items: Vec<TaskListItem> = result
                .items
                .iter()
                .filter_map(|item| {
                    let meta = serde_json::to_value(&item.meta).ok()?;
                    Some(TaskListItem {
                        live_status: default_live_status(&item.meta),
                        meta,
                        source_availability: "online",
                    })
                })
                .collect();
            // repo 的 query_task_list 已给出全量 total 与 has_more
            // （COUNT 独立于 LIMIT 截断），频道层不重算。
            let total = result.total.max(0) as usize;
            let has_more = result.has_more;
            json_value(
                &serde_json::to_value(TaskListResult {
                    items,
                    total,
                    has_more,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "mutateTask" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                address: TaskAddress,
                mutation: Mutation,
            }
            let parsed: P = serde_json::from_value(serde_json::Value::Object(object))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            // TS 多源投影：remote address 在壳内 local 单源不可达（无 remote source）。
            if parsed.address.remote_session_id.is_some() {
                return Err(ChannelError::error(
                    "远程 source 当前离线，禁止列表写操作".to_string(),
                ));
            }
            let task_ref = TaskAddress {
                task_id: parsed.address.task_id.clone(),
                workspace_path: parsed.address.workspace_path.clone(),
                workspace_identity: parsed.address.workspace_identity.clone(),
                remote_session_id: None,
            }
            .into_ref();
            match parsed.mutation {
                Mutation::Pin { pinned } => {
                    channel
                        .repo
                        .update_task_state(
                            &task_ref,
                            &TaskIndexStatePatch {
                                pinned: Some(pinned),
                                ..TaskIndexStatePatch::default()
                            },
                        )
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                }
                Mutation::Archive { archived } => {
                    channel
                        .repo
                        .update_task_state(
                            &task_ref,
                            &TaskIndexStatePatch {
                                archived: Some(archived),
                                ..TaskIndexStatePatch::default()
                            },
                        )
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                }
                Mutation::Delete => {
                    channel
                        .repo
                        .update_task_state(
                            &task_ref,
                            &TaskIndexStatePatch {
                                deleted: Some(true),
                                ..TaskIndexStatePatch::default()
                            },
                        )
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                }
                Mutation::DeleteArchived => {
                    // TS executeMutation 返回 bool，但窗口 mutateTask 统一返回
                    // projection 里的 meta（或 null）——删除后任务不再可见即 null。
                    channel
                        .repo
                        .delete_archived_task(&task_ref)
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                }
                Mutation::DeleteArchivedBatch { task_ids } => {
                    for task_id in task_ids {
                        let _ = channel.repo.delete_archived_task(&TaskRef {
                            workspace_path: task_ref.workspace_path.clone(),
                            workspace_identity: task_ref.workspace_identity.clone(),
                            task_id,
                        });
                    }
                }
                Mutation::MarkRead { expected_unread_at } => {
                    // TS setTaskUnread({unread:false, expectedUnreadAt})：带期望值的
                    // CAS 未命中（当前 unreadAt 不同）时不覆盖——repo patch 语义
                    // 无法表达 CAS，这里以 expected 与当前值比对后决定是否清除。
                    if let Some(expected) = expected_unread_at {
                        let current = channel
                            .repo
                            .get_task_meta(&task_ref)
                            .map_err(|error| ChannelError::error(error.to_string()))?;
                        if current.and_then(|meta| meta.unread_at) != Some(expected) {
                            // CAS 未命中：不写入，但 mutateTask 仍返回当前 meta（TS 同）。
                        } else {
                            channel
                                .repo
                                .update_task_state(
                                    &task_ref,
                                    &TaskIndexStatePatch {
                                        unread_at: Some(None),
                                        ..TaskIndexStatePatch::default()
                                    },
                                )
                                .map_err(|error| ChannelError::error(error.to_string()))?;
                        }
                    }
                }
                Mutation::MarkUnread => {
                    channel
                        .repo
                        .update_task_state(
                            &task_ref,
                            &TaskIndexStatePatch {
                                unread_at: Some(Some(now_ms())),
                                ..TaskIndexStatePatch::default()
                            },
                        )
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                }
                Mutation::Open | Mutation::Resume => {
                    // Controller 职责是验证唯一 source；conversation 随后仍走
                    // scoped facade（TS 同注释）。
                    let exists = channel
                        .repo
                        .get_task_meta(&task_ref)
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    if exists.is_none() {
                        return Err(ChannelError::error(
                            "没有与任务地址匹配的 source".to_string(),
                        ));
                    }
                }
            }
            // 写入成功后刷新投影：变更以 delta 帧广播给 controller 订阅者。
            channel.frames.refresh().map_err(ChannelError::error)?;
            // mutate 结果：返回更新后的 meta（TS projection.getTasks 查找同语义）。
            let meta = channel
                .repo
                .get_task_meta(&task_ref)
                .map_err(|error| ChannelError::error(error.to_string()))?;
            match meta {
                Some(meta) => json_value(
                    &serde_json::to_value(&meta)
                        .map_err(|error| ChannelError::error(error.to_string()))?,
                ),
                None => json_value(&JsonValue::Null),
            }
        }
        "deleteArchivedTask" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                address: TaskAddress,
            }
            let parsed: P = serde_json::from_value(serde_json::Value::Object(object))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            let deleted = channel
                .repo
                .delete_archived_task(&parsed.address.into_ref())
                .map_err(|error| ChannelError::error(error.to_string()))?;
            channel.frames.refresh().map_err(ChannelError::error)?;
            json_value(&serde_json::json!(deleted.is_some()))
        }
        "deleteArchivedTasks" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                address: TaskAddress,
                #[serde(default)]
                task_ids: Vec<String>,
            }
            let parsed: P = serde_json::from_value(serde_json::Value::Object(object))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            let base = parsed.address.into_ref();
            let mut result = ArchivedDeletionResult {
                deleted_task_ids: Vec::new(),
                skipped_task_ids: Vec::new(),
                failed_task_ids: Vec::new(),
            };
            for task_id in parsed.task_ids {
                let single = TaskRef {
                    workspace_path: base.workspace_path.clone(),
                    workspace_identity: base.workspace_identity.clone(),
                    task_id,
                };
                match channel.repo.delete_archived_task(&single) {
                    Ok(Some(meta)) => result.deleted_task_ids.push(meta.task_id),
                    Ok(None) => result.skipped_task_ids.push(single.task_id),
                    Err(_) => result.failed_task_ids.push(single.task_id),
                }
            }
            channel.frames.refresh().map_err(ChannelError::error)?;
            json_value(
                &serde_json::to_value(&result)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "subscribeControllerV4" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                topic: String,
            }
            let parsed: P = serde_json::from_value(serde_json::Value::Object(object))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            let topic = ControllerTopic::parse(&parsed.topic).ok_or_else(|| {
                ChannelError::error(format!("unknown controller topic: {}", parsed.topic))
            })?;
            // 先刷新再建订阅：snapshot 覆盖到调用时刻的索引状态。
            channel.frames.refresh().map_err(ChannelError::error)?;
            let ack = channel
                .frames
                .subscribe(topic)
                .map_err(ChannelError::error)?;
            json_value(&serde_json::json!({ "ack": ack }))
        }
        "resyncControllerV4" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                subscription_id: String,
            }
            let parsed: P = serde_json::from_value(serde_json::Value::Object(object))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            channel.frames.refresh().map_err(ChannelError::error)?;
            let ack = channel
                .frames
                .resync(&parsed.subscription_id)
                .map_err(ChannelError::error)?;
            json_value(&serde_json::json!({ "ack": ack }))
        }
        "unsubscribeControllerV4" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                subscription_id: String,
            }
            let parsed: P = serde_json::from_value(serde_json::Value::Object(object))
                .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            channel.frames.unsubscribe(&parsed.subscription_id);
            // TS `unsubscribeControllerV4(): Promise<void>`。
            Ok(WireValue::Undefined)
        }
        other => Err(ChannelError::error(format!(
            "window-controller channel: unknown command {other}"
        ))),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn json_value(value: &JsonValue) -> Result<WireValue, ChannelError> {
    match serde_json::to_string(value) {
        Ok(text) => Ok(WireValue::Json(text)),
        Err(error) => Err(ChannelError::error(error.to_string())),
    }
}

impl ServerChannel for WindowControllerChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: WireValue,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<WireValue, ChannelError>> {
        let channel = self.handle();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run(&channel, &command, &arg))
                .await
                .map_err(|error| {
                    ChannelError::error(format!("window-controller channel task failed: {error}"))
                })?
        })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        _arg: WireValue,
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        match event.as_str() {
            ON_DYNAMIC_CONTROLLER_FRAME => Ok(self.frames.subscribe_listener(emit)),
            other => Err(ChannelError::error(format!(
                "window-controller channel: unknown event {other}"
            ))),
        }
    }
}
