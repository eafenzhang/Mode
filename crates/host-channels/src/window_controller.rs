//! `window-controller` 频道适配器（LT-16.3 首切：local 单源列表 + 变更面）。
//!
//! 真源：`packages/services/src/window-controller/windowController.ts` 的
//! `IWindowControllerService` 与 `apps/desktop/src/host-standalone/windowController/`
//! 的 Host 实现。TS 实现是多源投影（local + remote 的 taskService/agentService
//! 聚合 + sessions-index observer）；Rust 壳内首切只做 **local 单源**：
//! - `listTaskList`：任务索引查询 → items（sourceAvailability 恒 online，
//!   liveStatus 从 meta.status 投影）。
//! - `mutateTask`：pin/archive/delete/mark-read/mark-unread → 任务索引 patch；
//!   open/resume 只做地址校验（Controller 职责是验证唯一 source，TS 同语义）。
//! - `deleteArchivedTask(s)`：归档删除（单条/批量）。
//! - controller 帧订阅（subscribe/resync/unsubscribe/onDynamicControllerFrame）：
//!   依赖 sessions-index 帧桥，留 16.3 续——显式报错，不静默降级。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value as WireValue;
use task_index_repo::{
    TaskIndexRepo, TaskIndexStatePatch, TaskListKind, TaskListQuery, TaskListSortBy,
    TaskListWorkspaceScope, TaskRef,
};

pub const WINDOW_CONTROLLER_CHANNEL: &str = "window-controller";

pub struct WindowControllerChannel {
    repo: Arc<TaskIndexRepo>,
}

impl WindowControllerChannel {
    pub fn new(repo: Arc<TaskIndexRepo>) -> Self {
        Self { repo }
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
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
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

fn live_status_of(meta: &task_index_repo::ModeTaskMeta) -> &'static str {
    match meta.status.as_deref() {
        Some("running") => "running",
        Some("waiting") => "waiting",
        Some("error") => "error",
        Some("completed") => "completed",
        _ => "idle",
    }
}

fn run(channel: &WindowControllerChannel, command: &str, arg: &WireValue) -> Result<WireValue, ChannelError> {
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
                        live_status: live_status_of(&item.meta),
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
            json_value(
                &serde_json::to_value(&result).map_err(|error| ChannelError::error(error.to_string()))?,
            )
        }
        "subscribeControllerV4" | "resyncControllerV4" | "unsubscribeControllerV4" => {
            Err(ChannelError::error(format!(
                "window-controller channel: {command} requires the sessions-index frame bridge (not assembled)"
            )))
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
        let repo = Arc::clone(&self.repo);
        let channel = WindowControllerChannel {
            repo: Arc::clone(&self.repo),
        };
        let _ = repo;
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run(&channel, &command, &arg))
                .await
                .map_err(|error| {
                    ChannelError::error(format!("window-controller channel task failed: {error}"))
                })?
        })
    }
}
