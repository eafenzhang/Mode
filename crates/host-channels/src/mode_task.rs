//! `mode-task` 频道适配器（LT-16.1 首切：任务列表读面 + 分组管理写面）。
//!
//! 真源：`packages/services/src/session/modeTaskService.ts` 的读面与分组方法
//! （TS `IModeTaskService`）。底层 `task-index-repo` 已整体迁移并与 TS 对拍
//! （query_task_list/grouped 视图/归档/分组管理）；本频道只做形状转换与分发。
//! 生命周期/命令队列/permission/elicitation/snapshot 族属编排面（16.2），不在
//! 本切片——未知方法显式报错，不做静默降级。

use std::sync::Arc;

use serde::Deserialize;

use agent_process::{AgentControlClient, AgentWorkspaceTarget};
use agent_protocol::session_write::SessionPersistence;
use agent_protocol::snapshot::SessionStateSnapshot;
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
    /// Agent 控制面（16.2a 编排方法需要）；`None` 时编排方法显式 unsupported
    /// （server-mode 无 agent 面，与 usage-stats 频道同降级语义）。
    control: Option<Arc<AgentControlClient>>,
}

impl ModeTaskChannel {
    pub fn new(repo: Arc<TaskIndexRepo>, control: Option<Arc<AgentControlClient>>) -> Self {
        Self { repo, control }
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

fn run_read(
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

// ── 16.2a 编排面（async：AgentControlClient 的会话面调用）──
async fn run_orchestration(
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
            // automation / 闲时任务新建空 session 后立即 sendText——V4 admission
            // 必须在首发前统一持久化 session 主记录（TS deferred persistence 同因）。
            let persistence = if parsed.automation_id.is_some() {
                Some(SessionPersistence::Deferred)
            } else {
                None
            };
            let snapshot: SessionStateSnapshot = control
                .create_session(
                    &target,
                    agent_process::CreateSessionOptions {
                        session_id: None,
                        parent_session_id: None,
                        mode: parsed.mode,
                        model: parsed.model,
                        persistence,
                        thought_level: parsed.thought_level,
                        title_generation_enabled: parsed.automation_id.as_ref().map(|_| false),
                        mcp_servers: None,
                        tool_allowlist: None,
                        tool_denylist: None,
                        imported_history: None,
                        off_peak_tool_enabled: false,
                        dynamic_workflow_enabled: false,
                    },
                )
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
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
            let mut result =
                serde_json::to_value(&meta).map_err(|error| ChannelError::error(error.to_string()))?;
            // task 创建结果携带 agent 协议快照里的命令列表（TS initialSlashCommands）。
            if let Some(commands) = &snapshot.slash_commands {
                if let Ok(value) = serde_json::to_value(commands) {
                    result["initialSlashCommands"] = value;
                }
            }
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
                    workspace_path: parsed.workspace_path,
                    workspace_identity: parsed.workspace_identity,
                    task_id: parsed.task_id,
                },
                &task_index_repo::TaskIndexStatePatch {
                    deleted: Some(true),
                    ..task_index_repo::TaskIndexStatePatch::default()
                },
            )
            .map_err(|error| ChannelError::error(error.to_string()))?;
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
            json_value(&serde_json::to_value(&meta).map_err(|error| ChannelError::error(error.to_string()))?)
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
                        automation_id: None,
                        off_peak_task_id: None,
                        off_peak_run_type: None,
                        bot_delivery_target: None,
                        tool_denylist: None,
                    },
                )
                .await
                .map_err(|error| ChannelError::error(error.to_string()))?;
            json_value(&serde_json::to_value(&result).map_err(|error| ChannelError::error(error.to_string()))?)
        }
        other => Err(ChannelError::error(format!(
            "mode-task channel: unknown orchestration command {other}"
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
        let control = self.control.clone();
        Box::pin(async move {
            match command.as_str() {
                // 编排面（async：AgentControlClient 会话调用）。
                "createTask" | "closeTask" | "resumeTask" | "sendPrompt" => {
                    let object = crate::params::first_arg_object(&arg)?;
                    let control = control.ok_or_else(|| {
                        ChannelError::error(format!(
                            "mode-task channel: {command} requires the agent control plane (not assembled)"
                        ))
                    })?;
                    run_orchestration(&repo, &control, &command, object).await
                }
                // 读面（同步 repo，spawn_blocking 防阻塞 runtime）。
                _ => tokio::task::spawn_blocking(move || run_read(&repo, &command, &arg))
                    .await
                    .map_err(|error| {
                        ChannelError::error(format!("mode-task channel task failed: {error}"))
                    })?,
            }
        })
    }
}
