//! 闲时任务派发端口（`off-peak-runtime` 生产实现）。
//!
//! 规范见 `docs/specs/off-peak-runtime.md`。只经 mode-task 频道公开 call 面编排
//! （`createTask`/`resumeTask`/`sendPrompt`），不复制第二条索引写入路径；终态走同
//! server 的事件总线，且订阅必须先于 `sendPrompt`。

use std::sync::Arc;

use off_peak_repo::{ModelSelection, OffPeakTask};
use off_peak_runtime::{
    OffPeakDispatchPort, OffPeakRunType, OffPeakTerminal, PortDispatchRequest, PortDispatchResult,
    RuntimeFuture, TerminalCallback, Unsubscribe,
};
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value as WireValue;
use serde_json::{json, Map, Value};
use task_index_repo::{TaskIndexRepo, TaskRef};

use crate::mode_task::TaskEventBus;

/// 生产派发端口：mode-task 频道 + 同进程任务事件总线 + 任务索引（错误文本）。
pub struct OffPeakDispatch {
    mode_task: Arc<dyn ServerChannel>,
    task_events: Arc<TaskEventBus>,
    repo: Arc<TaskIndexRepo>,
}

impl OffPeakDispatch {
    pub fn new(
        mode_task: Arc<dyn ServerChannel>,
        task_events: Arc<TaskEventBus>,
        repo: Arc<TaskIndexRepo>,
    ) -> Self {
        Self {
            mode_task,
            task_events,
            repo,
        }
    }

    async fn call_json(&self, command: &str, params: Value) -> Result<Value, String> {
        let value = self
            .mode_task
            .call(
                "off-peak".into(),
                command.to_string(),
                WireValue::Json(params.to_string()),
                CancellationToken::default(),
            )
            .await
            .map_err(|error| match error {
                rpc_host::ChannelError::Error { message, .. } => message,
                other => format!("{other:?}"),
            })?;
        match value {
            WireValue::Json(text) => serde_json::from_str(&text).map_err(|error| error.to_string()),
            WireValue::Undefined => Ok(Value::Null),
            other => Err(format!("unexpected channel payload: {other:?}")),
        }
    }

    fn workspace_params(task: &OffPeakTask) -> Map<String, Value> {
        let mut params = Map::new();
        params.insert("workspacePath".to_string(), json!(task.workspace_path));
        if let Some(identity) = &task.workspace_identity {
            params.insert("workspaceIdentity".to_string(), json!(identity));
        }
        params
    }
}

/// 终态事件不带错误详情：从 task-index `lastError.message` 补齐（spec §4）。
fn last_error(repo: &TaskIndexRepo, task_id: &str, task: &OffPeakTask) -> Option<String> {
    let meta = repo
        .get_task_meta(&TaskRef {
            workspace_path: task.workspace_path.clone(),
            workspace_identity: task.workspace_identity.clone(),
            task_id: task_id.to_string(),
        })
        .ok()
        .flatten()?;
    meta.last_error.map(|error| error.message)
}

/// 创建生产派发端口（装配方持有 Arc）。
pub fn create_off_peak_dispatch(
    mode_task: Arc<dyn ServerChannel>,
    task_events: Arc<TaskEventBus>,
    repo: Arc<TaskIndexRepo>,
) -> Arc<OffPeakDispatch> {
    Arc::new(OffPeakDispatch::new(mode_task, task_events, repo))
}

fn selection_json(selection: &ModelSelection) -> Value {
    let mut value = json!({
        "providerId": selection.provider_id,
        "modelId": selection.model_id,
    });
    if let Some(level) = selection
        .options
        .as_ref()
        .and_then(|options| options.reasoning_level.as_ref())
    {
        value["options"] = json!({ "reasoningLevel": level });
    }
    value
}

/// permissionMode → V4 协作模式（与 `mode_task::mode_command_value` 同口径：
/// `build`/`autoEdit` → `build`，其余原值）。
fn mode_value(permission_mode: &str) -> &'static str {
    match permission_mode {
        "plan" => "plan",
        "edit" => "edit",
        "yolo" => "yolo",
        "auto" => "auto",
        _ => "build",
    }
}

fn init_task_params(task: &OffPeakTask, selection: &ModelSelection) -> Value {
    let mut params = OffPeakDispatch::workspace_params(task);
    params.insert("model".to_string(), selection_json(selection));
    if let Some(level) = selection
        .options
        .as_ref()
        .and_then(|options| options.reasoning_level.clone())
    {
        params.insert("thoughtLevel".to_string(), json!(level));
    }
    params.insert("mode".to_string(), json!(mode_value(&task.permission_mode)));
    params.insert("offPeakTaskId".to_string(), json!(task.off_peak_task_id));
    Value::Object(params)
}

fn resume_task_params(task: &OffPeakTask, conversation_id: &str) -> Value {
    let mut params = OffPeakDispatch::workspace_params(task);
    params.insert("taskId".to_string(), json!(conversation_id));
    if let Some(level) = task
        .model_selection
        .as_ref()
        .and_then(|selection| selection.options.as_ref())
        .and_then(|options| options.reasoning_level.clone())
    {
        params.insert("thoughtLevel".to_string(), json!(level));
    }
    Value::Object(params)
}

fn send_prompt_params(task: &OffPeakTask, task_id: &str, run_type: OffPeakRunType) -> Value {
    let mut params = OffPeakDispatch::workspace_params(task);
    params.insert("taskId".to_string(), json!(task_id));
    params.insert("content".to_string(), json!(task.prompt));
    params.insert("inputId".to_string(), json!(task.off_peak_task_id));
    params.insert("offPeakTaskId".to_string(), json!(task.off_peak_task_id));
    params.insert("offPeakRunType".to_string(), json!(run_type.as_str()));
    Value::Object(params)
}

impl OffPeakDispatchPort for OffPeakDispatch {
    fn dispatch(
        &self,
        request: PortDispatchRequest,
        on_terminal: TerminalCallback,
    ) -> RuntimeFuture<'_, Result<(PortDispatchResult, Unsubscribe), String>> {
        Box::pin(async move {
            let task = &request.task;
            let selection = task
                .model_selection
                .as_ref()
                .ok_or_else(|| "off-peak task has no model selection".to_string())?;

            // conversationId 存在＝续跑（暂停恢复/票失效重取号后的继续）：
            // resume 原会话；否则新建 task 并把闲时归属盖章进索引。
            let task_id = match task
                .conversation_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(conversation_id) => {
                    self.call_json("resumeTask", resume_task_params(task, conversation_id))
                        .await?;
                    conversation_id.to_string()
                }
                None => {
                    let created = self
                        .call_json("createTask", init_task_params(task, selection))
                        .await?;
                    created
                        .get("taskId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "createTask 响应缺少 taskId".to_string())?
                        .to_string()
                }
            };

            // 终态订阅必须先于 sendPrompt 注册（快速完成的 turn 也不能漏结算）。
            let callback_task_id = task_id.clone();
            let callback_input_id = task.off_peak_task_id.clone();
            let callback_task = task.clone();
            let repo = Arc::clone(&self.repo);
            let subscription = self.task_events.subscribe_terminal(
                Some(task_id.clone()),
                Arc::new(move |value| {
                    let WireValue::Json(text) = value else {
                        return;
                    };
                    let Ok(payload) = serde_json::from_str::<Value>(&text) else {
                        return;
                    };
                    if payload.get("taskId").and_then(Value::as_str)
                        != Some(callback_task_id.as_str())
                    {
                        return;
                    }
                    let outcome = payload
                        .get("outcome")
                        .and_then(Value::as_str)
                        .unwrap_or("succeeded")
                        .to_string();
                    let input_id = payload
                        .get("inputId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| callback_input_id.clone());
                    let error = if outcome == "failed" {
                        last_error(repo.as_ref(), &callback_task_id, &callback_task)
                    } else {
                        None
                    };
                    on_terminal(OffPeakTerminal {
                        task_id: callback_task_id.clone(),
                        input_id,
                        outcome,
                        error,
                    });
                }),
            );

            if let Err(error) = self
                .call_json(
                    "sendPrompt",
                    send_prompt_params(task, &task_id, request.run_type),
                )
                .await
            {
                // prompt 未被接受：终态永远不会来，就地退订避免泄漏。
                subscription();
                return Err(error);
            }

            let unsubscribe: Unsubscribe = subscription;
            Ok((
                PortDispatchResult {
                    conversation_id: task_id.clone(),
                    session_id: Some(task_id),
                },
                unsubscribe,
            ))
        })
    }

    fn mark_task_unread(&self, task: &OffPeakTask) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mode_task = Arc::clone(&self.mode_task);
        let params = json!({
            "taskId": task.conversation_id,
            "workspacePath": task.workspace_path,
            "workspaceIdentity": task.workspace_identity,
            "unread": true,
        });
        handle.spawn(async move {
            let _ = mode_task
                .call(
                    "off-peak".into(),
                    "setTaskUnread".into(),
                    WireValue::Json(params.to_string()),
                    CancellationToken::default(),
                )
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use off_peak_repo::SelectionOptions;
    use task_index_repo::{parse_mode_task_meta, SyncTaskMetaParams};

    fn task() -> OffPeakTask {
        OffPeakTask {
            off_peak_task_id: "op-1".to_string(),
            server_ticket_id: Some("ticket-1".to_string()),
            title: "夜间整理".to_string(),
            conversation_id: None,
            session_id: None,
            session_title: None,
            prompt: "整理今天的笔记".to_string(),
            permission_mode: "autoEdit".to_string(),
            model_selection: Some(ModelSelection {
                provider_id: "zai".to_string(),
                model_id: "glm-4.6".to_string(),
                options: Some(SelectionOptions {
                    reasoning_level: Some("high".to_string()),
                }),
            }),
            model_selection_issue: None,
            workspace_key: "/work/a".to_string(),
            workspace_path: "/work/a".to_string(),
            workspace_identity: None,
            status: "queued".to_string(),
            queued_at: 1,
            started_at: None,
            ended_at: None,
            failure_reason: None,
            files_changed: None,
            settled_at: None,
            history_deleted_at: None,
            registered_at: None,
            schedulable: true,
            queue_position: None,
            next_poll_at: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    struct TempDb(PathBuf);

    impl TempDb {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "off-peak-host-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos() as u64)
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("dir");
            TempDb(dir)
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn mode_mapping_matches_session_modes() {
        assert_eq!(mode_value("plan"), "plan");
        assert_eq!(mode_value("edit"), "edit");
        assert_eq!(mode_value("yolo"), "yolo");
        assert_eq!(mode_value("auto"), "auto");
        assert_eq!(mode_value("build"), "build");
        // autoEdit 在 V4 是 build（同 mode_command_value）。
        assert_eq!(mode_value("autoEdit"), "build");
    }

    #[test]
    fn params_carry_off_peak_identity_and_selection() {
        let task = task();
        let selection = task.model_selection.clone().expect("selection");
        let init = init_task_params(&task, &selection);
        assert_eq!(init["offPeakTaskId"], "op-1");
        assert_eq!(init["mode"], "build");
        assert_eq!(init["model"]["providerId"], "zai");
        assert_eq!(init["model"]["options"]["reasoningLevel"], "high");
        assert_eq!(init["thoughtLevel"], "high");
        assert_eq!(init["workspacePath"], "/work/a");

        let send = send_prompt_params(&task, "task-9", OffPeakRunType::Resume);
        assert_eq!(send["taskId"], "task-9");
        assert_eq!(send["content"], "整理今天的笔记");
        assert_eq!(send["inputId"], "op-1");
        assert_eq!(send["offPeakTaskId"], "op-1");
        assert_eq!(send["offPeakRunType"], "resume");

        let resume = resume_task_params(&task, "task-9");
        assert_eq!(resume["taskId"], "task-9");
        assert_eq!(resume["thoughtLevel"], "high");
    }

    #[test]
    fn last_error_reads_task_index_meta() {
        let db = TempDb::new("last-error");
        let repo = Arc::new(TaskIndexRepo::new(db.0.join("tasks-index.sqlite")));
        let meta = parse_mode_task_meta(&json!({
            "taskId": "task-1",
            "traceId": "trace-1",
            "title": "夜间整理",
            "workspacePath": "/work/a",
            "createdAt": 1,
            "updatedAt": 1,
            "mode": "build",
            "provider": "glm",
            "lastError": { "message": "off-peak-ticket-expired" }
        }))
        .expect("meta");
        repo.sync_task_meta(SyncTaskMetaParams {
            meta,
            pinned: None,
            archived: None,
            deleted: None,
            title_overridden: None,
            searchable_text: None,
        })
        .expect("sync");

        assert_eq!(
            last_error(repo.as_ref(), "task-1", &task()).as_deref(),
            Some("off-peak-ticket-expired")
        );
        assert_eq!(last_error(repo.as_ref(), "task-missing", &task()), None);
    }
}
