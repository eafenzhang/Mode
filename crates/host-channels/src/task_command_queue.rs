//! 任务运行时命令队列（LT-16.2b）：host 内存队列 + 串行 drain。
//!
//! 真源：`packages/services/src/mode-agent/modeTaskServiceAdapter.ts` 的
//! `runtimeCommands` / `activePromptInputIds` / `runtimeCommandDrains` /
//! `enqueueTaskCommand` / `promoteTaskCommand` / `cancelTaskCommand` /
//! `drainRuntimeCommands`。语义：
//! - owner 门控（`assertCurrentOwnerRun`）：ownerRunId 缺省跳过；无 active run →
//!   `NO_ACTIVE_TASK_OWNER`；不匹配 → `STALE_TASK_OWNER_COMMAND`（旧 run 不能改队列）。
//! - drain：active run 进行中或已有 running 命令时跳过；取队首 accepted 的
//!   send_prompt → mark running → 锁外经注入的 send executor 发送；成功移除命令、
//!   失败 mark failed（留在队列，UI 可见）。
//! - promote：accepted 化并移到队首；cancel：not_found 容忍。
//!
//! 与 TS 的差异：TS 的 drain 链用 Promise 链串行；Rust 用 per-key tokio Mutex。
//! readyOutcome 事件（onDynamicTaskReady）与 active run 的登记/清除在 16.2c
//! 事件面接线。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use agent_protocol::session::SessionSendParams;
use agent_process::AgentWorkspaceTarget;
use rpc_host::ChannelError;
use serde::Serialize;

pub type SendFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'static>>;

/// TS `ModeSendPromptRuntimeCommand`。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCommand {
    pub command_id: String,
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_id: Option<String>,
    pub workspace_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
    pub workspace_key: String,
    pub status: RuntimeCommandStatus,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_label: Option<String>,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<serde_json::Map<String, serde_json::Value>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeCommandStatus {
    Accepted,
    Running,
    Failed,
}

/// TS `ModeEnqueueTaskCommandResult`。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueResult {
    pub accepted: bool,
    pub command: RuntimeCommand,
}

/// TS `ModeCancelTaskCommandResult`。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelResult {
    pub canceled: bool,
    pub command_id: String,
    pub reason: &'static str,
}

/// send executor：drain 消费 accepted 命令时的实际发送（生产装配 = control.send_prompt）。
pub type SendExecutor = Arc<
    dyn Fn(AgentWorkspaceTarget, SessionSendParams) -> SendFuture + Send + Sync,
>;

#[derive(Debug, Default)]
struct QueueState {
    commands: HashMap<String, Vec<RuntimeCommand>>,
    /// taskKey → 当前 owner run 的 inputId（TS activePromptInputIds）。
    active_runs: HashMap<String, String>,
}

/// 生产时钟：墙钟毫秒（TS Date.now() 口径）。
pub fn default_now_ms() -> Arc<dyn Fn() -> i64 + Send + Sync> {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0)
    })
}

struct QueueInner {
    state: Mutex<QueueState>,
    send_executor: SendExecutor,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// per-task drain 串行锁（TS runtimeCommandDrains Promise 链的等价物）。
    drains: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

pub struct TaskCommandQueue {
    inner: Arc<QueueInner>,
}

fn task_key(workspace_path: &str, workspace_identity: Option<&str>, task_id: &str) -> String {
    format!(
        "{}\u{0000}{}",
        workspace_identity
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(workspace_path),
        task_id
    )
}

/// TS `assertCurrentOwnerRun` 的错误码。
pub const NO_ACTIVE_TASK_OWNER: &str = "NO_ACTIVE_TASK_OWNER";
pub const STALE_TASK_OWNER_COMMAND: &str = "STALE_TASK_OWNER_COMMAND";

fn owner_gate(
    state: &QueueState,
    key: &str,
    owner_run_id: Option<&str>,
) -> Result<(), ChannelError> {
    let Some(owner_run_id) = owner_run_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let Some(active_run_id) = state.active_runs.get(key) else {
        // 手机端 owner command 到达 host 时 task 可能已终态收口；
        // 不能再把旧 command 写入队列，否则桌面 shared host 会误发旧输入。
        return Err(ChannelError::error(format!(
            "No active task owner. (code: {NO_ACTIVE_TASK_OWNER})"
        )));
    };
    if active_run_id != owner_run_id {
        return Err(ChannelError::error(format!(
            "Stale task owner command. (code: {STALE_TASK_OWNER_COMMAND})"
        )));
    }
    Ok(())
}

impl TaskCommandQueue {
    pub fn new(send_executor: SendExecutor, now_ms: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        Self {
            inner: Arc::new(QueueInner {
                state: Mutex::new(QueueState::default()),
                send_executor,
                now_ms,
                drains: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// TS `enqueueTaskCommand`：owner 门控 → 队尾追加 accepted → 触发 drain。
    pub fn enqueue(
        &self,
        params: &QueueCommandParams,
        owner_run_id: Option<&str>,
        client_id: Option<&str>,
        client_label: Option<&str>,
    ) -> Result<EnqueueResult, ChannelError> {
        let key = task_key(&params.workspace_path, params.workspace_identity.as_deref(), &params.task_id);
        let now = (self.inner.now_ms)();
        let mut state = self.inner.state.lock().expect("task command queue poisoned");
        owner_gate(&state, &key, owner_run_id)?;
        let command = RuntimeCommand {
            command_id: params.command_id.clone(),
            task_id: params.task_id.clone(),
            trace_id: params.trace_id.clone(),
            query_id: params.query_id.clone(),
            workspace_path: params.workspace_path.clone(),
            workspace_identity: params.workspace_identity.clone(),
            workspace_key: key.clone(),
            status: RuntimeCommandStatus::Accepted,
            created_at: now,
            updated_at: now,
            content: params.content.clone(),
            client_id: client_id.map(str::to_string),
            client_label: client_label.map(str::to_string),
            // 手动 run / 手机 replayable 的 prompt 可能先进 host command queue；
            // 若这里丢 automationId，drain 时会按普通用户输入发送（TS 同因注释）。
            automation_id: params.automation_id.clone(),
            attachments: params.attachments.clone(),
            error: None,
        };
        state
            .commands
            .entry(key.clone())
            .or_default()
            .push(command.clone());
        drop(state);
        schedule_drain(&self.inner, &key, &params.workspace_path, params.workspace_identity.as_deref());
        Ok(EnqueueResult {
            accepted: true,
            command,
        })
    }

    /// TS `promoteTaskCommand`：owner 门控 → 找命令 → accepted 化并移到队首。
    pub fn promote(
        &self,
        params: &QueueCommandParams,
        command_id: &str,
        owner_run_id: Option<&str>,
    ) -> Result<EnqueueResult, ChannelError> {
        let key = task_key(&params.workspace_path, params.workspace_identity.as_deref(), &params.task_id);
        let now = (self.inner.now_ms)();
        let mut state = self.inner.state.lock().expect("task command queue poisoned");
        owner_gate(&state, &key, owner_run_id)?;
        let commands = state.commands.get_mut(&key).ok_or_else(|| {
            ChannelError::error("Task command not found. (code: OWNER_COMMAND_FAILED)")
        })?;
        let index = commands
            .iter()
            .position(|candidate| candidate.command_id == command_id)
            .ok_or_else(|| {
                ChannelError::error("Task command not found. (code: OWNER_COMMAND_FAILED)")
            })?;
        let mut command = commands.remove(index);
        if command.status != RuntimeCommandStatus::Running {
            command.status = RuntimeCommandStatus::Accepted;
            command.updated_at = now;
        }
        commands.insert(0, command.clone());
        drop(state);
        schedule_drain(&self.inner, &key, &params.workspace_path, params.workspace_identity.as_deref());
        Ok(EnqueueResult {
            accepted: true,
            command,
        })
    }

    /// TS `cancelTaskCommand`：owner 门控 → 移除；不存在容忍（canceled/not_found）。
    pub fn cancel(
        &self,
        params: &QueueCommandParams,
        command_id: &str,
        owner_run_id: Option<&str>,
    ) -> Result<CancelResult, ChannelError> {
        let key = task_key(&params.workspace_path, params.workspace_identity.as_deref(), &params.task_id);
        let mut state = self.inner.state.lock().expect("task command queue poisoned");
        owner_gate(&state, &key, owner_run_id)?;
        if let Some(commands) = state.commands.get_mut(&key) {
            let before = commands.len();
            commands.retain(|candidate| candidate.command_id != command_id);
            if commands.len() != before {
                return Ok(CancelResult {
                    canceled: true,
                    command_id: command_id.to_string(),
                    reason: "removed",
                });
            }
        }
        Ok(CancelResult {
            canceled: true,
            command_id: command_id.to_string(),
            reason: "not_found",
        })
    }

    /// drain 单次消费（TS `drainRuntimeCommands`）：委托 QueueInner。
    pub async fn drain_once(
        &self,
        key: &str,
        workspace_path: &str,
        workspace_identity: Option<&str>,
    ) {
        self.inner
            .drain_once(key, workspace_path, workspace_identity)
            .await;
    }

        pub fn snapshot_commands(&self, key: &str) -> Vec<RuntimeCommand> {
        self.inner.state
            .lock()
            .expect("task command queue poisoned")
            .commands
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    /// 测试/诊断：登记 active run（生产由 16.2c 的 prompt 生命周期事件写入）。
    pub fn set_active_run_for_test(&self, key: &str, input_id: &str) {
        self.inner.state
            .lock()
            .expect("task command queue poisoned")
            .active_runs
            .insert(key.to_string(), input_id.to_string());
    }

    /// 测试/诊断：清除 active run。
    pub fn clear_active_run_for_test(&self, key: &str) {
        self.inner.state
            .lock()
            .expect("task command queue poisoned")
            .active_runs
            .remove(key);
    }
}

impl QueueInner {
    /// 实际 drain 消费：active run / running 存在则跳过；
    /// 取队首 accepted send_prompt → running → 锁外发送 → 成功移除 / 失败标 failed。
    pub async fn drain_once(
        &self,
        key: &str,
        workspace_path: &str,
        workspace_identity: Option<&str>,
    ) {
        let target = AgentWorkspaceTarget {
            workspace_path: workspace_path.to_string(),
            workspace_identity: workspace_identity.map(str::to_string),
            remote_session_id: None,
        };
        // 锁内：门控检查 + 取队首 accepted + mark running。
        let command = {
            let mut state = self.state.lock().expect("task command queue poisoned");
            // active run 进行中：prompt 还没收口，不消费队列。
            if state.active_runs.contains_key(key) {
                return;
            }
            let Some(commands) = state.commands.get_mut(key) else {
                return;
            };
            if commands
                .iter()
                .any(|candidate| candidate.status == RuntimeCommandStatus::Running)
            {
                return;
            }
            let Some(index) = commands.iter().position(
                |candidate| candidate.status == RuntimeCommandStatus::Accepted,
            ) else {
                return;
            };
            let mut command = commands[index].clone();
            command.status = RuntimeCommandStatus::Running;
            command.updated_at = (self.now_ms)();
            commands[index] = command.clone();
            command
        };
        // 锁外发送（fail-soft：失败标 failed 留在队列，TS 同语义）。
        let send_params = SessionSendParams {
            session_id: command.task_id.clone(),
            model_selection: None,
            model_execution: None,
            input_id: Some(command.command_id.clone()),
            query_id: command.query_id.clone(),
            content: command.content.clone(),
            attachments: command.attachments.clone(),
            browser_ambient_context: None,
            expected_revision: None,
            expected_provider_revision: None,
            // drain 的命令在 enqueue 时盖过 automationId——丢失会让 CronCreate
            // 重新暴露（TS 同因注释）。
            automation_id: command.automation_id.clone(),
            off_peak_task_id: None,
            off_peak_run_type: None,
            bot_delivery_target: None,
            tool_denylist: None,
        };
        let outcome = (self.send_executor)(target, send_params).await;
        let mut state = self.state.lock().expect("task command queue poisoned");
        let Some(commands) = state.commands.get_mut(key) else {
            return;
        };
        match outcome {
            Ok(()) => {
                // 发送成功：从队列移除（session 输入已被 agent 接收）。
                commands.retain(|candidate| candidate.command_id != command.command_id);
            }
            Err(error) => {
                for candidate in commands.iter_mut() {
                    if candidate.command_id == command.command_id {
                        candidate.status = RuntimeCommandStatus::Failed;
                        candidate.updated_at = (self.now_ms)();
                        candidate.error = Some(error.clone());
                    }
                }
            }
        }
    }

}

/// TS `scheduleRuntimeCommandDrain`：per-key 串行（tokio Mutex），锁内 drain 一次。
/// spawn 捕获 `Arc<QueueInner>`（drain 任务 'static，队列生命周期独立于频道调用）。
fn schedule_drain(
    queue: &Arc<QueueInner>,
    key: &str,
    workspace_path: &str,
    workspace_identity: Option<&str>,
) {
    let inner = Arc::clone(queue);
    let lock = {
        let mut drains = inner.drains.lock().expect("drain map poisoned");
        Arc::clone(drains.entry(key.to_string()).or_default())
    };
    let key = key.to_string();
    let workspace_path = workspace_path.to_string();
    let workspace_identity = workspace_identity.map(str::to_string);
    tokio::spawn(async move {
        let _guard = lock.lock().await;
        // drain_once 在锁内串行执行（TS Promise 链等价）。
        inner
            .drain_once(&key, &workspace_path, workspace_identity.as_deref())
            .await;
    });
}

/// TS `ModeEnqueueTaskCommandParams` 的队列入参子集。
#[derive(Debug, Clone, Default)]
pub struct QueueCommandParams {
    pub command_id: String,
    pub task_id: String,
    pub trace_id: Option<String>,
    pub query_id: Option<String>,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub content: String,
    pub automation_id: Option<String>,
    pub attachments: Option<Vec<serde_json::Map<String, serde_json::Value>>>,
}
