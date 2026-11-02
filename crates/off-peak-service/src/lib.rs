//! 闲时任务本地生命周期服务。
//!
//! 真源：`packages/services/src/session/offPeakTaskService.ts` 的本地管理方法。
//! 取号/轮询/核销 outbox 仍由 TS 服务负责；本 crate 覆盖
//! pause/deleteHistory/cancel/delete/update/list/get 与模型可用性投影。

use std::sync::Arc;

use model_selection::ModelSelection;
use off_peak_repo::{
    EditableFields, EditableModelSelection, InvalidateOptions, MarkTerminalOptions, OffPeakTask,
    OffPeakTaskRepo, TaskScope,
};
pub mod server;

pub use server::{
    OffPeakBatchStatusResult, OffPeakClientError, OffPeakServerClient,
    OffPeakTakeNumberAvailability, OffPeakTakeTicketResult, OffPeakTaskCreateFailure,
    OffPeakTaskCreateParams, OffPeakTaskCreateResult, OffPeakTaskCreateSuccess,
    OffPeakTicketStatusEntry,
};

use serde::Deserialize;
use serde_json::Value;

mod remote;

const TERMINAL_STATUSES: [&str; 3] = ["completed", "failed", "cancelled"];

pub fn is_off_peak_terminal_status(status: &str) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

/// 服务依赖：模型解析、取消 running 时的 loop 停止、终态核销与列表变更广播。
pub trait OffPeakTaskEnvironment {
    /// 用当前 Registry 解析选择；`None` 表示不可用。
    fn resolve_model_selection(&self, selection: &ModelSelection) -> Option<ModelSelection>;
    fn stop_running_task(
        &self,
        _conversation_id: &str,
        _workspace_path: &str,
        _workspace_identity: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }
    /// 终态核销（TS 的 settleOne）；本 crate 只触发，不维护 outbox。
    fn settle_terminal(&self, _task: &OffPeakTask) {}
    fn on_tasks_changed(&self) {}
    /// `getCodingPlanSupport` 的脱敏快照；缺省返回 null。
    fn coding_plan_support(&self) -> Value {
        Value::Null
    }
    /// 只返回安全 hostname；失败按空串处理。
    fn telemetry_provider_name(&self) -> Result<String, String> {
        Ok(String::new())
    }
    fn request_scheduler_wake(&self) {}
    fn on_dispose(&self) {}
}

/// `updateTask` 入参：`modelSelection: null` 明确拒绝编辑；缺省表示保留当前选择。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OffPeakUpdateTaskParams {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub model_selection: Option<Option<ModelSelection>>,
}

fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

pub(crate) const OFF_PEAK_SYNC_MIN_INTERVAL_MS: i64 = 5_000;
pub(crate) const OFF_PEAK_SYNC_MAX_INTERVAL_MS: i64 = 5 * 60_000;
pub(crate) const SYNC_FAILURE_BASE_MS: i64 = 10_000;

#[derive(Debug, Default)]
pub(crate) struct SyncState {
    pub(crate) running: bool,
    pub(crate) stopped: bool,
    pub(crate) consecutive_failures: i64,
}

pub struct OffPeakTaskService<E: OffPeakTaskEnvironment> {
    pub(crate) repo: OffPeakTaskRepo,
    pub(crate) environment: E,
    pub(crate) now: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub(crate) client: Option<Arc<dyn OffPeakServerClient>>,
    pub(crate) new_task_id: Arc<dyn Fn() -> String + Send + Sync>,
    pub(crate) sync_state: std::sync::Mutex<SyncState>,
}

impl<E: OffPeakTaskEnvironment> OffPeakTaskService<E> {
    pub fn new(
        repo: OffPeakTaskRepo,
        environment: E,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            repo,
            environment,
            now,
            client: None,
            new_task_id: Arc::new(|| "offpeak-task".to_string()),
            sync_state: std::sync::Mutex::new(SyncState::default()),
        }
    }

    /// 注入服务端客户端与任务 ID 生成器（create/continue/同步需要）。
    pub fn with_server(
        mut self,
        client: Arc<dyn OffPeakServerClient>,
        new_task_id: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Self {
        self.client = Some(client);
        self.new_task_id = new_task_id;
        self
    }

    pub fn repo(&self) -> &OffPeakTaskRepo {
        &self.repo
    }

    pub(crate) fn now(&self) -> i64 {
        (self.now)()
    }

    pub(crate) fn emit_changed(&self) {
        self.environment.on_tasks_changed();
    }

    /// `validateDispatchModelSelection`：解析成功且 provider/model 与选择一致。
    pub fn validate_dispatch_model_selection(&self, selection: &ModelSelection) -> bool {
        self.environment
            .resolve_model_selection(selection)
            .is_some_and(|resolved| {
                resolved.provider_id == selection.provider_id
                    && resolved.model_id == selection.model_id
            })
    }

    pub(crate) fn project_task(&self, task: &OffPeakTask) -> Value {
        let mut value = serde_json::to_value(task).expect("OffPeakTask 序列化不会失败");
        let Some(object) = value.as_object_mut() else {
            return value;
        };
        let usable = match &task.model_selection {
            Some(selection) => {
                is_off_peak_terminal_status(&task.status)
                    || self.validate_dispatch_model_selection(selection)
            }
            None => false,
        };
        if let Some(selection) = &task.model_selection {
            if !usable {
                object.insert("schedulable".to_string(), Value::Bool(false));
                let mut issue = serde_json::Map::new();
                issue.insert(
                    "code".to_string(),
                    Value::String("repair-required".to_string()),
                );
                issue.insert(
                    "legacyModelId".to_string(),
                    Value::String(selection.model_id.clone()),
                );
                if let Some(reasoning_level) = selection
                    .options
                    .as_ref()
                    .and_then(|options| options.reasoning_level.clone())
                {
                    issue.insert(
                        "legacyReasoningLevel".to_string(),
                        Value::String(reasoning_level),
                    );
                }
                object.insert("modelSelectionIssue".to_string(), Value::Object(issue));
            }
        } else {
            object.insert("schedulable".to_string(), Value::Bool(false));
        }
        value
    }

    pub fn list(&self) -> Result<Vec<Value>, String> {
        let tasks = self
            .repo
            .list(&TaskScope::default())
            .map_err(|error| error.to_string())?;
        Ok(tasks.iter().map(|task| self.project_task(task)).collect())
    }

    pub fn get(&self, off_peak_task_id: &str) -> Result<Option<Value>, String> {
        let task = self
            .repo
            .get(off_peak_task_id)
            .map_err(|error| error.to_string())?;
        Ok(task.as_ref().map(|task| self.project_task(task)))
    }

    pub fn pause_task(&self, off_peak_task_id: &str) -> Result<Option<Value>, String> {
        let task = self
            .repo
            .set_paused(
                off_peak_task_id,
                true,
                &InvalidateOptions {
                    now: Some(self.now()),
                },
            )
            .map_err(|error| error.to_string())?;
        if task.is_some() {
            self.emit_changed();
        }
        Ok(task.map(|task| serde_json::to_value(task).expect("序列化")))
    }

    pub fn delete_history(&self, off_peak_task_id: &str) -> Result<Option<Value>, String> {
        let task = self
            .repo
            .mark_history_deleted(
                off_peak_task_id,
                &InvalidateOptions {
                    now: Some(self.now()),
                },
            )
            .map_err(|error| error.to_string())?;
        if task.is_some() {
            self.emit_changed();
        }
        Ok(task.map(|task| serde_json::to_value(task).expect("序列化")))
    }

    pub fn cancel_task(&self, off_peak_task_id: &str) -> Result<Option<Value>, String> {
        let Some(existing) = self
            .repo
            .get(off_peak_task_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        if is_off_peak_terminal_status(&existing.status) {
            return Ok(Some(serde_json::to_value(existing).expect("序列化")));
        }
        let cancelled = self
            .repo
            .mark_terminal(
                off_peak_task_id,
                &MarkTerminalOptions {
                    status: "cancelled".to_string(),
                    ended_at: self.now(),
                    failure_reason: None,
                    files_changed: None,
                    dispatch_error: None,
                },
            )
            .map_err(|error| error.to_string())?;
        let Some(cancelled) = cancelled else {
            return Ok(self
                .repo
                .get(off_peak_task_id)
                .map_err(|error| error.to_string())?
                .map(|task| serde_json::to_value(task).expect("序列化")));
        };
        if existing.status == "running" {
            if let Some(conversation_id) = existing.conversation_id.as_deref() {
                let _ = self.environment.stop_running_task(
                    conversation_id,
                    &existing.workspace_path,
                    existing.workspace_identity.as_deref(),
                );
            }
        }
        self.emit_changed();
        self.environment.settle_terminal(&cancelled);
        Ok(Some(serde_json::to_value(cancelled).expect("序列化")))
    }

    pub fn delete_task(&self, off_peak_task_id: &str) -> Result<(), String> {
        let Some(existing) = self
            .repo
            .get(off_peak_task_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        if !is_off_peak_terminal_status(&existing.status) {
            let _ = self.cancel_task(off_peak_task_id)?;
        }
        self.repo
            .delete(off_peak_task_id)
            .map_err(|error| error.to_string())?;
        self.emit_changed();
        Ok(())
    }

    pub fn update_task(
        &self,
        off_peak_task_id: &str,
        params: &OffPeakUpdateTaskParams,
    ) -> Result<Option<Value>, String> {
        let Some(existing) = self
            .repo
            .get(off_peak_task_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        if existing.status != "queued" && existing.status != "paused" {
            return Ok(None);
        }
        if matches!(params.model_selection, Some(None)) {
            return Ok(None);
        }
        let candidate = match &params.model_selection {
            Some(Some(selection)) => Some(selection.clone()),
            Some(None) => None,
            None => existing.model_selection.clone(),
        };
        let Some(candidate) = candidate else {
            return Ok(None);
        };
        let Some(resolved) = self.environment.resolve_model_selection(&candidate) else {
            return Ok(None);
        };
        let updated = self
            .repo
            .update_editable_fields(
                off_peak_task_id,
                &EditableFields {
                    title: params.title.clone(),
                    prompt: params.prompt.clone(),
                    permission_mode: params.permission_mode.clone(),
                    model_selection: Some(EditableModelSelection::Replace(resolved)),
                },
                &InvalidateOptions {
                    now: Some(self.now()),
                },
            )
            .map_err(|error| error.to_string())?;
        if updated.is_some() {
            self.emit_changed();
        }
        Ok(updated.map(|task| serde_json::to_value(task).expect("序列化")))
    }
}
