//! `mode-task` 频道适配器（LT-16.1 首切：任务列表读面 + 分组管理写面）。
//!
//! 真源：`packages/services/src/session/modeTaskService.ts` 的读面与分组方法
//! （TS `IModeTaskService`）。底层 `task-index-repo` 已整体迁移并与 TS 对拍
//! （query_task_list/grouped 视图/归档/分组管理）；本频道只做形状转换与分发。
//! 生命周期/命令队列/permission/elicitation/snapshot 族属编排面（16.2），不在
//! 本切片——未知方法显式报错，不做静默降级。

use std::sync::Arc;

use serde::Deserialize;

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value as WireValue;
use task_index_repo::{
    ArchiveStaleTasksParams, GroupedTaskRef, GroupedTaskGroupRefs, GroupedTaskViewOrderInput,
    GroupedTaskViewTopLevelNodeRef, ListDeletedTaskIdsParams, ListTaskMetasParams, TaskIndexRepo,
    TaskListKind, TaskListQuery, TaskListSortBy, TaskListWorkspaceScope,
};

pub const MODE_TASK_CHANNEL: &str = "mode-task";

pub struct ModeTaskChannel {
    repo: Arc<TaskIndexRepo>,
}

impl ModeTaskChannel {
    pub fn new(repo: Arc<TaskIndexRepo>) -> Self {
        Self { repo }
    }
}

// ── 参数形状（camelCase；TS modeTaskService 同名方法入参） ──

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceScopeParams {
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
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

fn json_value(value: &serde_json::Value) -> Result<WireValue, ChannelError> {
    match serde_json::to_string(value) {
        Ok(text) => Ok(WireValue::Json(text)),
        Err(error) => Err(ChannelError::error(error.to_string())),
    }
}

// ── 分发 ──

fn run(
    repo: &TaskIndexRepo,
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
            json_value(&serde_json::to_value(&result).map_err(|error| ChannelError::error(error.to_string()))?)
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
            json_value(&serde_json::to_value(&metas).map_err(|error| ChannelError::error(error.to_string()))?)
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
            json_value(&serde_json::to_value(&ids).map_err(|error| ChannelError::error(error.to_string()))?)
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
            json_value(&serde_json::to_value(&ids).map_err(|error| ChannelError::error(error.to_string()))?)
        }
        "listGroupedTaskViewStructure" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct P {
                #[serde(default)]
                workspace_scopes: Vec<WorkspaceScopeParams>,
            }
            let parsed: P = parse(object)?;
            let structure = repo.query_grouped_task_view_structure(&repo_scopes(
                parsed.workspace_scopes,
            ))
            .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(&serde_json::to_value(&structure).map_err(|error| ChannelError::error(error.to_string()))?)
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
            let view = repo.apply_grouped_task_view_order(&GroupedTaskViewOrderInput {
                    workspace_scopes: repo_scopes(parsed.workspace_scopes),
                    top_level_nodes,
                    groups,
                    provider: None,
                },
            )
            .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(&serde_json::to_value(&view).map_err(|error| ChannelError::error(error.to_string()))?)
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
            json_value(&serde_json::to_value(&group).map_err(|error| ChannelError::error(error.to_string()))?)
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
            }
            let parsed: P = parse(object)?;
            match command {
                "renameTaskGroup" => {
                    let title = parsed
                        .title
                        .ok_or_else(|| ChannelError::error("renameTaskGroup requires title"))?;
                    let group = repo
                        .rename_task_group(&parsed.group_id, &title)
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    json_value(
                        &serde_json::to_value(&group).map_err(|error| ChannelError::error(error.to_string()))?,
                    )
                }
                "updateTaskGroupColor" => {
                    let color = parsed
                        .color
                        .ok_or_else(|| ChannelError::error("updateTaskGroupColor requires color"))?;
                    let group = repo
                        .update_task_group_color(&parsed.group_id, &color)
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    json_value(
                        &serde_json::to_value(&group).map_err(|error| ChannelError::error(error.to_string()))?,
                    )
                }
                _ => {
                    repo.delete_task_group(&parsed.group_id)
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    Ok(WireValue::Undefined)
                }
            }
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
            json_value(&serde_json::to_value(&archived).map_err(|error| ChannelError::error(error.to_string()))?)
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
                    workspace_path: parsed.workspace_path,
                    workspace_identity: parsed.workspace_identity,
                    older_than_days: parsed.older_than_days,
                    provider: None,
                })
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(&serde_json::to_value(&metas).map_err(|error| ChannelError::error(error.to_string()))?)
        }
        other => Err(ChannelError::error(format!(
            "mode-task channel: unknown or not-yet-migrated command {other}"
        ))),
    }
}

impl ServerChannel for ModeTaskChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: WireValue,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<WireValue, ChannelError>> {
        let repo = Arc::clone(&self.repo);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run(&repo, &command, &arg))
                .await
                .map_err(|error| {
                    ChannelError::error(format!("mode-task channel task failed: {error}"))
                })?
        })
    }
}
