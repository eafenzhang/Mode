//! Automation 宿主装配（G2.2/G2.3）：`AutomationRepo` + 生产 `DispatchPort`。
//!
//! 调度状态机与结算在 `automation-runtime`；本模块只把「建/恢复 task → 终态订阅
//! （先于 sendPrompt）→ sendPrompt(inputId=runId) → mark unread」接到既有
//! mode-task 频道（复用其 createTask/resumeTask/sendPrompt/setAutomationSessionConfig
//! 编排，避免第二条写入路径）。
//!
//! `ModelSelectionPort` 由宿主注入：当前 provider 选择面（facade getView）尚未迁
//! Rust，Tauri 装配需提供实现；本模块不伪造模型解析。

use std::sync::Arc;

use automation_repo::AutomationRepo;
use automation_runtime::{
    AutomationRuntime, AutomationRuntimeOptions, DispatchPort, ModelSelectionPort,
    PortDispatchRequest, PortDispatchResult, RuntimeFuture, RuntimeLog, TerminalOutcome,
    Unsubscribe,
};
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value as WireValue;
use serde_json::{json, Value};

use crate::mode_task::TaskEventBus;

pub struct AutomationHostOptions {
    /// 与 tasks-index 同库（Node 侧 AutomationRepo 亦使用 tasks-index.sqlite）。
    pub db_path: std::path::PathBuf,
    /// mode-task 频道（装配方持有的 Arc；本模块只经频道公开 call 面调用）。
    pub mode_task: Arc<dyn ServerChannel>,
    /// 任务事件总线：终态订阅（inputId 对账）。
    pub task_events: Arc<TaskEventBus>,
    pub utc_offset_secs: i32,
    pub poll_interval_ms: u64,
    pub log: RuntimeLog,
}

pub struct AutomationHost {
    pub runtime: Arc<AutomationRuntime>,
    pub repo: Arc<AutomationRepo>,
}

/// 生产 DispatchPort：经 mode-task 频道公开方法编排 task，终态走同 server 事件总线。
pub struct AutomationDispatch {
    mode_task: Arc<dyn ServerChannel>,
    task_events: Arc<TaskEventBus>,
}

impl AutomationDispatch {
    async fn call_json(&self, command: &str, params: Value) -> Result<Value, String> {
        let value = self
            .mode_task
            .call(
                "automation".into(),
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
            WireValue::Json(text) => {
                serde_json::from_str(&text).map_err(|error| error.to_string())
            }
            // Promise<void>（sendPrompt 之外的 void 方法）按 null 处理。
            WireValue::Undefined => Ok(Value::Null),
            other => Err(format!("unexpected channel payload: {other:?}")),
        }
    }

    fn workspace_params(
        request: &PortDispatchRequest,
    ) -> serde_json::Map<String, Value> {
        let mut params = serde_json::Map::new();
        params.insert(
            "workspacePath".to_string(),
            json!(request.workspace_path),
        );
        if let Some(identity) = &request.workspace_identity {
            params.insert("workspaceIdentity".to_string(), json!(identity));
        }
        params
    }
}

impl DispatchPort for AutomationDispatch {
    fn dispatch(
        &self,
        request: PortDispatchRequest,
        on_terminal: Arc<dyn Fn(TerminalOutcome) + Send + Sync>,
    ) -> RuntimeFuture<'_, Result<(PortDispatchResult, Unsubscribe), String>> {
        Box::pin(async move {
            let selection = &request.model_selection;
            let thought_level = selection
                .options
                .as_ref()
                .and_then(|options| options.reasoning_level.clone());
            let mut model = json!({
                "providerId": selection.provider_id,
                "modelId": selection.model_id,
            });
            if let Some(level) = &thought_level {
                model["options"] = json!({ "reasoningLevel": level });
            }

            // targetTaskId 存在＝会话内创建的 automation，追加回绑定会话；
            // 否则新建 task（Node dispatchRun 同）。
            let task_id = if let Some(target_task_id) = &request.target_task_id {
                let mut params = Self::workspace_params(&request);
                params.insert("taskId".to_string(), json!(target_task_id));
                if let Some(level) = &thought_level {
                    params.insert("thoughtLevel".to_string(), json!(level));
                }
                self.call_json("resumeTask", Value::Object(params)).await?;
                self.call_json(
                    "setAutomationSessionConfig",
                    json!({
                        "taskId": target_task_id,
                        "modelSelection": {
                            "providerId": selection.provider_id,
                            "modelId": selection.model_id,
                        },
                        "thoughtLevel": thought_level,
                        "mode": request.mode,
                        "workspacePath": request.workspace_path,
                        "workspaceIdentity": request.workspace_identity,
                    }),
                )
                .await?;
                target_task_id.clone()
            } else {
                let mut params = Self::workspace_params(&request);
                params.insert("model".to_string(), model);
                if let Some(level) = &thought_level {
                    params.insert("thoughtLevel".to_string(), json!(level));
                }
                if let Some(mode) = &request.mode {
                    params.insert("mode".to_string(), json!(mode));
                }
                params.insert("automationId".to_string(), json!(request.automation_id));
                let created = self.call_json("createTask", Value::Object(params)).await?;
                created
                    .get("taskId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "createTask 响应缺少 taskId".to_string())?
                    .to_string()
            };

            // 终态订阅必须先于 sendPrompt 注册（快速完成的 turn 也不能漏回写）。
            let run_id = request.run_id.clone();
            let callback_task_id = task_id.clone();
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
                    let input_id = payload
                        .get("inputId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| run_id.clone());
                    on_terminal(TerminalOutcome {
                        task_id: callback_task_id.clone(),
                        input_id,
                        outcome: payload
                            .get("outcome")
                            .and_then(Value::as_str)
                            .unwrap_or("succeeded")
                            .to_string(),
                        error: None,
                    });
                }),
            );

            let mut send_params = Self::workspace_params(&request);
            send_params.insert("taskId".to_string(), json!(task_id));
            send_params.insert("content".to_string(), json!(request.prompt));
            send_params.insert("inputId".to_string(), json!(request.run_id));
            send_params.insert(
                "automationId".to_string(),
                json!(request.automation_id),
            );
            if let Err(error) = self
                .call_json("sendPrompt", Value::Object(send_params))
                .await
            {
                // prompt 未被接受：终态永远不会来，就地退订避免泄漏。
                subscription();
                return Err(error);
            }

            let unsubscribe: Unsubscribe = subscription;
            Ok((
                PortDispatchResult {
                    task_id: task_id.clone(),
                    session_id: task_id,
                },
                unsubscribe,
            ))
        })
    }

    fn mark_task_unread(
        &self,
        task_id: &str,
        workspace_path: &str,
        workspace_identity: Option<&str>,
    ) {
        // 只有 tokio 上下文可用时才能异步投递；best-effort，失败不影响结算。
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let mode_task = Arc::clone(&self.mode_task);
        let params = json!({
            "taskId": task_id,
            "workspacePath": workspace_path,
            "workspaceIdentity": workspace_identity,
            "unread": true,
        });
        handle.spawn(async move {
            let _ = mode_task
                .call(
                    "automation".into(),
                    "setTaskUnread".into(),
                    WireValue::Json(params.to_string()),
                    CancellationToken::default(),
                )
                .await;
        });
    }
}

/// 创建宿主（不启动轮询；调用方决定 [`AutomationRuntime::start`] 的所有权时机）。
pub fn create_automation_host(
    options: AutomationHostOptions,
    model_selection: Arc<dyn ModelSelectionPort>,
) -> AutomationHost {
    let repo = Arc::new(AutomationRepo::new(options.db_path));
    let dispatch = Arc::new(AutomationDispatch {
        mode_task: options.mode_task,
        task_events: options.task_events,
    });
    let mut runtime_options =
        AutomationRuntimeOptions::new(Arc::clone(&repo), dispatch, model_selection);
    runtime_options.utc_offset_secs = options.utc_offset_secs;
    runtime_options.poll_interval_ms = options.poll_interval_ms;
    runtime_options.log = options.log;
    AutomationHost {
        runtime: AutomationRuntime::new(runtime_options),
        repo,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use automation_repo::ModelSelection;
    use automation_runtime::{ModelSelectionPort, ModelSelectionView};
    use std::sync::Mutex;

    use crate::mode_task::{ModeTaskChannel, TaskEventBus};
    use task_index_repo::{parse_mode_task_meta, SyncTaskMetaParams, TaskIndexRepo, TaskRef};

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "automation-host-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos() as u64)
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct FakeSelection(Option<ModelSelection>);

    impl ModelSelectionPort for FakeSelection {
        fn get_view(
            &self,
            _selection: Option<ModelSelection>,
        ) -> RuntimeFuture<'_, Result<ModelSelectionView, String>> {
            let selection = self.0.clone();
            Box::pin(async move {
                Ok(ModelSelectionView {
                    selection_issue: selection.is_none(),
                    effective_selection: selection.clone(),
                    preferred_selection: selection,
                })
            })
        }
    }

    fn selection() -> ModelSelection {
        ModelSelection {
            provider_id: "glm".to_string(),
            model_id: "glm-5".to_string(),
            options: None,
        }
    }

    fn task_channel(dir: &TempDir, bus: Arc<TaskEventBus>) -> (Arc<TaskIndexRepo>, Arc<ModeTaskChannel>) {
        let repo = Arc::new(TaskIndexRepo::new(dir.0.join("tasks-index.sqlite")));
        let channel = Arc::new(ModeTaskChannel::new(
            Arc::clone(&repo),
            None,
            None,
            bus,
        ));
        (repo, channel)
    }

    #[test]
    fn creates_host_with_injected_selection_port() {
        let dir = TempDir::new("create");
        let bus = Arc::new(TaskEventBus::new());
        let (_repo, channel) = task_channel(&dir, Arc::clone(&bus));
        let host = create_automation_host(
            AutomationHostOptions {
                db_path: dir.0.join("tasks-index.sqlite"),
                mode_task: channel,
                task_events: bus,
                utc_offset_secs: 8 * 3600,
                poll_interval_ms: 20_000,
                log: Arc::new(|_level, _message| {}),
            },
            Arc::new(FakeSelection(Some(selection()))),
        );
        assert!(!host.runtime.is_disposed());
    }

    #[tokio::test]
    async fn mark_task_unread_sets_index_marker_and_emits_event() {
        let dir = TempDir::new("unread");
        let bus = Arc::new(TaskEventBus::new());
        let (repo, channel) = task_channel(&dir, Arc::clone(&bus));
        repo.sync_task_meta(SyncTaskMetaParams {
            meta: parse_mode_task_meta(&json!({
                "taskId": "task-1",
                "traceId": "trace-1",
                "title": "任务",
                "workspacePath": "D:/work/app",
                "createdAt": 1_000,
                "updatedAt": 1_000,
                "mode": "build",
                "provider": "glm",
            }))
            .expect("meta"),
            pinned: None,
            archived: None,
            deleted: None,
            title_overridden: None,
            searchable_text: None,
        })
        .expect("seed");

        let events = Arc::new(Mutex::new(Vec::<Value>::new()));
        let sink = Arc::clone(&events);
        let _sub = bus.subscribe_task_list(
            crate::mode_task::WorkspaceEventScope::default(),
            Arc::new(move |value| {
                if let WireValue::Json(text) = value {
                    if let Ok(payload) = serde_json::from_str(&text) {
                        sink.lock().expect("events").push(payload);
                    }
                }
            }),
        );

        let dispatch = AutomationDispatch {
            mode_task: channel,
            task_events: bus,
        };
        dispatch.mark_task_unread("task-1", "D:/work/app", None);
        // 异步投递：等索引落盘。
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            let meta = repo
                .get_task_meta(&TaskRef {
                    workspace_path: "D:/work/app".to_string(),
                    workspace_identity: None,
                    task_id: "task-1".to_string(),
                })
                .expect("meta")
                .expect("exists");
            if meta.unread_at.is_some() {
                assert!(events.lock().expect("events").iter().any(|event| {
                    event["reason"] == "task_meta_changed"
                }));
                return;
            }
        }
        panic!("mark_task_unread 未写入 unreadAt");
    }
}
