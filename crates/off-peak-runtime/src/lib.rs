//! 闲时任务调度运行时：`claim_due` → 派发会话 prompt → 终态结算。
//!
//! 规范见 `docs/specs/off-peak-runtime.md`。行为依据是本仓库已冻结的契约：
//! `docs/specs/off-peak-repo.md`（认领/状态机）、`docs/specs/off-peak-service.md`
//! （取号/轮询由调用方驱动）与 `docs/specs/session-write-group.md`
//! （`offPeakTaskId`/`offPeakRunType`）。取号与 settle outbox 仍由 Node Host 拥有；
//! 本 crate 只消费 `schedulable=1` 的任务，不写 ticket 状态。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use off_peak_repo::{
    InvalidateOptions, MarkRunningOptions, MarkTerminalOptions, OffPeakTask, OffPeakTaskRepo,
    ReleaseClaimOptions,
};
use tokio::task::JoinHandle;

pub type RuntimeFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type RuntimeLog = Arc<dyn Fn(&str, &str) + Send + Sync>;
pub type Unsubscribe = Box<dyn FnOnce() + Send>;
pub type TerminalCallback = Arc<dyn Fn(OffPeakTerminal) + Send + Sync>;

pub const DEFAULT_POLL_INTERVAL_MS: u64 = 20_000;
pub const DEFAULT_MAX_DISPATCH_ATTEMPTS: u32 = 3;

/// agent 适配层嵌入错误文本的稳定标记（`packages/shared/src/off-peak-types.ts`）。
/// 命中时按「重新取号 → 续跑」处理，不落 failed。
pub const TICKET_EXPIRED_MARKER: &str = "off-peak-ticket-expired";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffPeakRunType {
    Init,
    Resume,
}

impl OffPeakRunType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Init => "init",
            Self::Resume => "resume",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PortDispatchRequest {
    pub task: OffPeakTask,
    pub run_type: OffPeakRunType,
}

#[derive(Debug, Clone)]
pub struct PortDispatchResult {
    pub conversation_id: String,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OffPeakTerminal {
    pub task_id: String,
    pub input_id: String,
    /// `succeeded` | `failed`（同 `TaskEventBus::notify_terminal`）。
    pub outcome: String,
    /// 失败文本；生产端口从 task-index `lastError.message` 补齐。
    pub error: Option<String>,
}

/// 派发端口：建/恢复会话并 `sendPrompt`，终态经回调送回运行时。
pub trait OffPeakDispatchPort: Send + Sync {
    /// 必须先注册终态回调再发 prompt；返回退订句柄。
    fn dispatch(
        &self,
        request: PortDispatchRequest,
        on_terminal: TerminalCallback,
    ) -> RuntimeFuture<'_, Result<(PortDispatchResult, Unsubscribe), String>>;

    /// `setTaskUnread(true)` 的 best-effort 投影；失败不影响结算。
    fn mark_task_unread(&self, _task: &OffPeakTask) {}
}

pub struct OffPeakRuntimeOptions {
    pub repo: Arc<OffPeakTaskRepo>,
    pub dispatch: Arc<dyn OffPeakDispatchPort>,
    pub poll_interval_ms: u64,
    pub max_dispatch_attempts: u32,
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub log: RuntimeLog,
}

#[derive(Default)]
struct ActiveDispatch {
    unsubscribe: Option<Unsubscribe>,
    terminal: bool,
}

#[derive(Default)]
struct RuntimeState {
    started: bool,
    disposed: bool,
    active: HashMap<String, ActiveDispatch>,
    poll_task: Option<JoinHandle<()>>,
}

pub struct OffPeakRuntime {
    repo: Arc<OffPeakTaskRepo>,
    dispatch: Arc<dyn OffPeakDispatchPort>,
    poll_interval: Duration,
    max_dispatch_attempts: u32,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    log: RuntimeLog,
    state: Mutex<RuntimeState>,
    tick_gate: tokio::sync::Mutex<()>,
    tick_requested: AtomicBool,
}

impl OffPeakRuntime {
    pub fn new(options: OffPeakRuntimeOptions) -> Arc<Self> {
        Arc::new(Self {
            repo: options.repo,
            dispatch: options.dispatch,
            poll_interval: Duration::from_millis(options.poll_interval_ms.max(1)),
            max_dispatch_attempts: options.max_dispatch_attempts.max(1),
            clock: options.clock,
            log: options.log,
            state: Mutex::new(RuntimeState::default()),
            tick_gate: tokio::sync::Mutex::new(()),
            tick_requested: AtomicBool::new(false),
        })
    }

    fn lock(&self) -> MutexGuard<'_, RuntimeState> {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn log(&self, level: &str, message: impl AsRef<str>) {
        (self.log)(level, message.as_ref());
    }

    pub fn is_disposed(&self) -> bool {
        self.lock().disposed
    }

    /// 启动轮询并立即跑一轮；幂等。启动时回收上次进程残留的 `running` 行。
    pub fn start(self: &Arc<Self>) {
        {
            let mut state = self.lock();
            if state.started || state.disposed {
                return;
            }
            state.started = true;
        }
        match self.repo.recover_interrupted((self.clock)()) {
            Ok(recovered) if recovered > 0 => {
                self.log(
                    "info",
                    format!("off-peak 启动回收中断任务 count={recovered}"),
                );
            }
            Ok(_) => {}
            Err(error) => self.log("warn", format!("off-peak 启动回收失败: {error}")),
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.log("warn", "off-peak runtime 无 Tokio 上下文，未启动轮询");
            return;
        };
        let runtime = Arc::clone(self);
        let interval = self.poll_interval;
        let task = handle.spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if runtime.is_disposed() {
                    break;
                }
                runtime.request_tick().await;
            }
        });
        self.lock().poll_task = Some(task);
        self.log(
            "info",
            format!(
                "off-peak runtime started pollIntervalMs={}",
                self.poll_interval.as_millis()
            ),
        );
    }

    /// 停止轮询、等待在途 tick、退订并释放仍持有的认领。
    pub async fn stop(self: &Arc<Self>) {
        {
            let mut state = self.lock();
            if state.disposed {
                return;
            }
            state.disposed = true;
            if let Some(task) = state.poll_task.take() {
                task.abort();
            }
        }
        let _guard = self.tick_gate.lock().await;
        let active = std::mem::take(&mut self.lock().active);
        for (task_id, mut entry) in active {
            if let Some(unsubscribe) = entry.unsubscribe.take() {
                unsubscribe();
            }
            // 已 mark_running 的行 claim 已清，调用无副作用；仍在派发中的行回队重试。
            if let Err(error) = self.repo.release_claim(
                &task_id,
                &ReleaseClaimOptions {
                    error: None,
                    now: None,
                },
            ) {
                self.log(
                    "warn",
                    format!("off-peak stop 释放认领失败 task={task_id}: {error}"),
                );
            }
        }
        self.log("info", "off-peak runtime stopped");
    }

    /// 请求立即触发一轮认领并等待完成。
    pub async fn wake(self: &Arc<Self>) {
        self.request_tick().await;
    }

    /// 测试/诊断入口：在指定时间戳跑一轮 claim→dispatch→settle。
    pub async fn tick_at(self: &Arc<Self>, now: i64) {
        let _guard = self.tick_gate.lock().await;
        self.tick_once(now).await;
    }

    async fn request_tick(self: &Arc<Self>) {
        if self.is_disposed() {
            return;
        }
        self.tick_requested.store(true, Ordering::SeqCst);
        let _guard = self.tick_gate.lock().await;
        let mut first = true;
        while first || self.tick_requested.swap(false, Ordering::SeqCst) {
            first = false;
            if self.is_disposed() {
                break;
            }
            let now = (self.clock)();
            self.tick_once(now).await;
        }
    }

    async fn tick_once(self: &Arc<Self>, now: i64) {
        let claimed = match self.repo.claim_due(now) {
            Ok(claimed) => claimed,
            Err(error) => {
                self.log("error", format!("off-peak tick claim failed: {error}"));
                return;
            }
        };
        for task in claimed {
            self.handle_claimed(task, now).await;
        }
    }

    async fn handle_claimed(self: &Arc<Self>, task: OffPeakTask, now: i64) {
        let task_id = task.off_peak_task_id.clone();
        let run_type = if task
            .conversation_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        {
            OffPeakRunType::Resume
        } else {
            OffPeakRunType::Init
        };
        let callback = self.terminal_callback(&task);
        self.lock().active.insert(
            task_id.clone(),
            ActiveDispatch {
                unsubscribe: None,
                terminal: false,
            },
        );
        let request = PortDispatchRequest { task, run_type };
        match self.dispatch.dispatch(request, callback).await {
            Ok((result, unsubscribe)) => self.finish_dispatch(&task_id, result, unsubscribe, now),
            Err(error) => self.fail_dispatch(&task_id, error),
        }
    }

    fn finish_dispatch(
        self: &Arc<Self>,
        task_id: &str,
        result: PortDispatchResult,
        unsubscribe: Unsubscribe,
        now: i64,
    ) {
        let mut unsubscribe = Some(unsubscribe);
        let still_active = {
            let mut state = self.lock();
            match state.active.get_mut(task_id) {
                Some(entry) => {
                    if entry.terminal {
                        false
                    } else {
                        entry.unsubscribe = unsubscribe.take();
                        true
                    }
                }
                None => false,
            }
        };
        if let Some(unsubscribe) = unsubscribe {
            // 终态已先于 dispatch 返回：立即退订，避免句柄泄漏。
            unsubscribe();
            return;
        }
        if !still_active {
            return;
        }
        let started = self.repo.mark_running(
            task_id,
            &MarkRunningOptions {
                started_at: now,
                conversation_id: Some(result.conversation_id),
                session_id: result.session_id,
                server_ticket_id: None,
            },
        );
        match started {
            Ok(Some(_)) => self.log("info", format!("off-peak dispatched task={task_id}")),
            Ok(None) => {
                // 派发期间任务已被取消/暂停：退订，不再结算。
                let unsubscribe = self
                    .lock()
                    .active
                    .remove(task_id)
                    .and_then(|mut entry| entry.unsubscribe.take());
                if let Some(unsubscribe) = unsubscribe {
                    unsubscribe();
                }
                self.log(
                    "warn",
                    format!("off-peak task 在派发期间已非 queued task={task_id}"),
                );
            }
            Err(error) => self.log(
                "error",
                format!("off-peak mark_running failed task={task_id}: {error}"),
            ),
        }
    }

    fn fail_dispatch(self: &Arc<Self>, task_id: &str, error: String) {
        let unsubscribe = self
            .lock()
            .active
            .remove(task_id)
            .and_then(|mut entry| entry.unsubscribe.take());
        if let Some(unsubscribe) = unsubscribe {
            unsubscribe();
        }
        if let Err(release_error) = self.repo.release_claim(
            task_id,
            &ReleaseClaimOptions {
                error: Some(error.clone()),
                now: None,
            },
        ) {
            self.log(
                "warn",
                format!("off-peak release claim failed task={task_id}: {release_error}"),
            );
        }
        let attempts = self.repo.attempt_count(task_id).unwrap_or(0);
        if attempts >= i64::from(self.max_dispatch_attempts) {
            if let Err(terminal_error) = self.repo.mark_terminal(
                task_id,
                &MarkTerminalOptions {
                    status: "failed".to_string(),
                    ended_at: (self.clock)(),
                    failure_reason: Some(error.clone()),
                    files_changed: None,
                    dispatch_error: Some(error.clone()),
                },
            ) {
                self.log(
                    "error",
                    format!("off-peak mark_terminal failed task={task_id}: {terminal_error}"),
                );
            }
        }
        self.log(
            "warn",
            format!("off-peak dispatch failed task={task_id}: {error}"),
        );
    }

    fn terminal_callback(self: &Arc<Self>, task: &OffPeakTask) -> TerminalCallback {
        let runtime = Arc::downgrade(self);
        let task_id = task.off_peak_task_id.clone();
        let snapshot = task.clone();
        Arc::new(move |outcome: OffPeakTerminal| {
            if outcome.input_id != task_id {
                return;
            }
            let Some(runtime) = runtime.upgrade() else {
                return;
            };
            runtime.handle_terminal(outcome, &snapshot);
        })
    }

    fn handle_terminal(&self, outcome: OffPeakTerminal, task: &OffPeakTask) {
        let unsubscribe = {
            let mut state = self.lock();
            match state.active.get_mut(&outcome.task_id) {
                Some(entry) => {
                    if entry.terminal {
                        return;
                    }
                    entry.terminal = true;
                    entry.unsubscribe.take()
                }
                None => {
                    if state.disposed {
                        return;
                    }
                    None
                }
            }
        };
        self.lock().active.remove(&outcome.task_id);
        if let Some(unsubscribe) = unsubscribe {
            unsubscribe();
        }
        let now = (self.clock)();
        let task_id = outcome.task_id.as_str();
        match outcome.outcome.as_str() {
            "succeeded" => {
                if let Err(error) = self.repo.mark_terminal(
                    task_id,
                    &MarkTerminalOptions {
                        status: "completed".to_string(),
                        ended_at: now,
                        failure_reason: None,
                        files_changed: None,
                        dispatch_error: None,
                    },
                ) {
                    self.log(
                        "error",
                        format!("off-peak mark completed failed task={task_id}: {error}"),
                    );
                }
            }
            "failed" => {
                let error = outcome.error.unwrap_or_default();
                if error.contains(TICKET_EXPIRED_MARKER) {
                    // 票失效：回队等 Node 轮询重取号后续跑（spec §4）。
                    if let Err(requeue_error) = self
                        .repo
                        .requeue_for_continuation(task_id, &InvalidateOptions { now: Some(now) })
                    {
                        self.log(
                            "error",
                            format!("off-peak requeue failed task={task_id}: {requeue_error}"),
                        );
                    }
                } else if let Err(mark_error) = self.repo.mark_terminal(
                    task_id,
                    &MarkTerminalOptions {
                        status: "failed".to_string(),
                        ended_at: now,
                        failure_reason: Some(error).filter(|value| !value.is_empty()),
                        files_changed: None,
                        dispatch_error: None,
                    },
                ) {
                    self.log(
                        "error",
                        format!("off-peak mark failed task={task_id}: {mark_error}"),
                    );
                }
            }
            other => self.log(
                "warn",
                format!("off-peak 未知终态 outcome={other} task={task_id}"),
            ),
        }
        // 派发时快照还没有 conversationId（mark_running 之后才有），完成通知必须用
        // 最新行，否则 setTaskUnread 拿不到 taskId。
        let latest = self.repo.get(task_id).ok().flatten();
        self.dispatch
            .mark_task_unread(latest.as_ref().unwrap_or(task));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;

    use off_peak_repo::{CreateOptions, CreateParams, ModelSelection, SelectionOptions};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "off-peak-runtime-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos() as u64)
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("dir");
            TempDir(dir)
        }

        fn db(&self) -> PathBuf {
            self.0.join("tasks-index.sqlite")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const NOW: i64 = 1_000_000;

    fn repo(dir: &TempDir) -> Arc<OffPeakTaskRepo> {
        Arc::new(OffPeakTaskRepo::with_clock(dir.db(), || NOW))
    }

    fn create_ready(repo: &OffPeakTaskRepo, id: &str) {
        repo.create(
            &CreateParams {
                title: "夜间整理".to_string(),
                prompt: "整理今天的笔记".to_string(),
                permission_mode: "yolo".to_string(),
                model_selection: ModelSelection {
                    provider_id: "zai".to_string(),
                    model_id: "glm-4.6".to_string(),
                    options: Some(SelectionOptions {
                        reasoning_level: Some("high".to_string()),
                    }),
                },
                workspace_path: "/work/a".to_string(),
                workspace_identity: None,
                bound_session_id: None,
            },
            &CreateOptions {
                now: Some(NOW),
                off_peak_task_id: Some(id.to_string()),
                schedulable: Some(true),
                ..CreateOptions::default()
            },
        )
        .expect("create");
    }

    /// 记录派发请求；终态由测试显式触发。
    struct FakeDispatch {
        requests: StdMutex<Vec<PortDispatchRequest>>,
        callbacks: StdMutex<HashMap<String, TerminalCallback>>,
        unsubscribed: Arc<StdMutex<Vec<String>>>,
        result: Result<PortDispatchResult, String>,
        /// 在 `dispatch` 返回前同步触发的终态（模拟快速完成的 turn）。
        early_terminal: Option<OffPeakTerminal>,
    }

    impl FakeDispatch {
        fn ok() -> Arc<Self> {
            Self::with_result(Ok(PortDispatchResult {
                conversation_id: "task-1".to_string(),
                session_id: Some("task-1".to_string()),
            }))
        }

        fn ok_with_early_terminal(outcome: OffPeakTerminal) -> Arc<Self> {
            let dispatch = Self::with_result(Ok(PortDispatchResult {
                conversation_id: "task-1".to_string(),
                session_id: Some("task-1".to_string()),
            }));
            Arc::new(Self {
                early_terminal: Some(outcome),
                ..Arc::try_unwrap(dispatch).ok().expect("unique dispatch")
            })
        }

        fn failing(error: &str) -> Arc<Self> {
            Self::with_result(Err(error.to_string()))
        }

        fn with_result(result: Result<PortDispatchResult, String>) -> Arc<Self> {
            Arc::new(Self {
                requests: StdMutex::new(Vec::new()),
                callbacks: StdMutex::new(HashMap::new()),
                unsubscribed: Arc::new(StdMutex::new(Vec::new())),
                result,
                early_terminal: None,
            })
        }

        fn emit(&self, task_id: &str, outcome: OffPeakTerminal) {
            let callback = self
                .callbacks
                .lock()
                .expect("callbacks")
                .get(task_id)
                .cloned()
                .expect("callback registered");
            callback(outcome);
        }

        fn unsubscribed_ids(&self) -> Vec<String> {
            self.unsubscribed.lock().expect("unsub").clone()
        }
    }

    impl OffPeakDispatchPort for FakeDispatch {
        fn dispatch(
            &self,
            request: PortDispatchRequest,
            on_terminal: TerminalCallback,
        ) -> RuntimeFuture<'_, Result<(PortDispatchResult, Unsubscribe), String>> {
            self.requests
                .lock()
                .expect("requests")
                .push(request.clone());
            let task_id = request.task.off_peak_task_id.clone();
            self.callbacks
                .lock()
                .expect("callbacks")
                .insert(task_id.clone(), Arc::clone(&on_terminal));
            if let Some(outcome) = &self.early_terminal {
                on_terminal(outcome.clone());
            }
            let result = self.result.clone();
            let tracked = task_id;
            let unsubscribed = Arc::clone(&self.unsubscribed);
            Box::pin(async move {
                let result = result?;
                let unsubscribe: Unsubscribe = Box::new(move || {
                    unsubscribed.lock().expect("unsub").push(tracked);
                });
                Ok((result, unsubscribe))
            })
        }

        fn mark_task_unread(&self, _task: &OffPeakTask) {}
    }

    fn runtime(
        repo: Arc<OffPeakTaskRepo>,
        dispatch: Arc<dyn OffPeakDispatchPort>,
    ) -> Arc<OffPeakRuntime> {
        OffPeakRuntime::new(OffPeakRuntimeOptions {
            repo,
            dispatch,
            poll_interval_ms: 20_000,
            max_dispatch_attempts: 2,
            clock: Arc::new(|| NOW),
            log: Arc::new(|_, _| {}),
        })
    }

    #[tokio::test]
    async fn claim_then_settle_success() {
        let dir = TempDir::new("success");
        let repo = repo(&dir);
        create_ready(&repo, "op-1");
        let dispatch = FakeDispatch::ok();
        let runtime = runtime(Arc::clone(&repo), dispatch.clone());

        runtime.tick_at(NOW).await;
        let requests = dispatch.requests.lock().expect("requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].run_type, OffPeakRunType::Init);
        assert_eq!(requests[0].task.off_peak_task_id, "op-1");
        drop(requests);

        let running = repo.get("op-1").expect("get").expect("row");
        assert_eq!(running.status, "running");
        assert_eq!(running.conversation_id.as_deref(), Some("task-1"));

        dispatch.emit(
            "op-1",
            OffPeakTerminal {
                task_id: "op-1".to_string(),
                input_id: "op-1".to_string(),
                outcome: "succeeded".to_string(),
                error: None,
            },
        );
        let settled = repo.get("op-1").expect("get").expect("row");
        assert_eq!(settled.status, "completed");
        assert_eq!(settled.ended_at, Some(NOW));
        assert_eq!(dispatch.unsubscribed_ids(), vec!["op-1".to_string()]);
    }

    #[tokio::test]
    async fn terminal_before_dispatch_returns_is_settled_once() {
        let dir = TempDir::new("early-terminal");
        let repo = repo(&dir);
        create_ready(&repo, "op-2");
        let dispatch = FakeDispatch::ok_with_early_terminal(OffPeakTerminal {
            task_id: "op-2".to_string(),
            input_id: "op-2".to_string(),
            outcome: "succeeded".to_string(),
            error: None,
        });
        let runtime = runtime(Arc::clone(&repo), dispatch.clone());

        runtime.tick_at(NOW).await;
        let settled = repo.get("op-2").expect("get").expect("row");
        assert_eq!(settled.status, "completed");
        // dispatch 返回后立即退订，句柄不泄漏。
        assert_eq!(dispatch.unsubscribed_ids(), vec!["op-2".to_string()]);
    }

    #[tokio::test]
    async fn resume_run_type_uses_conversation_id() {
        let dir = TempDir::new("resume");
        let repo = repo(&dir);
        create_ready(&repo, "op-3");
        repo.mark_running(
            "op-3",
            &MarkRunningOptions {
                started_at: NOW,
                conversation_id: Some("task-9".to_string()),
                session_id: Some("task-9".to_string()),
                server_ticket_id: None,
            },
        )
        .expect("mark running");
        repo.requeue_for_continuation("op-3", &InvalidateOptions { now: Some(NOW) })
            .expect("requeue")
            .expect("row");
        repo.update_scheduling_snapshot(
            "op-3",
            &off_peak_repo::SchedulingPatch {
                schedulable: Some(true),
                now: Some(NOW),
                ..off_peak_repo::SchedulingPatch::default()
            },
        )
        .expect("schedulable");

        let dispatch = FakeDispatch::ok();
        let runtime = runtime(Arc::clone(&repo), dispatch.clone());
        runtime.tick_at(NOW).await;
        let requests = dispatch.requests.lock().expect("requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].run_type, OffPeakRunType::Resume);
    }

    #[tokio::test]
    async fn dispatch_failure_retries_then_marks_failed() {
        let dir = TempDir::new("failure");
        let repo = repo(&dir);
        create_ready(&repo, "op-4");
        let dispatch = FakeDispatch::failing("agent unavailable");
        let runtime = runtime(Arc::clone(&repo), dispatch);

        runtime.tick_at(NOW).await;
        let after_first = repo.get("op-4").expect("get").expect("row");
        assert_eq!(after_first.status, "queued");
        assert_eq!(repo.attempt_count("op-4").expect("attempts"), 1);

        runtime.tick_at(NOW).await;
        let after_second = repo.get("op-4").expect("get").expect("row");
        assert_eq!(after_second.status, "failed");
        assert_eq!(
            after_second.failure_reason.as_deref(),
            Some("agent unavailable")
        );
    }

    #[tokio::test]
    async fn ticket_expired_failure_requeues_for_continuation() {
        let dir = TempDir::new("ticket-expired");
        let repo = repo(&dir);
        create_ready(&repo, "op-5");
        let dispatch = FakeDispatch::ok();
        let runtime = runtime(Arc::clone(&repo), dispatch.clone());

        runtime.tick_at(NOW).await;
        dispatch.emit(
            "op-5",
            OffPeakTerminal {
                task_id: "op-5".to_string(),
                input_id: "op-5".to_string(),
                outcome: "failed".to_string(),
                error: Some("... off-peak-ticket-expired ...".to_string()),
            },
        );
        let requeued = repo.get("op-5").expect("get").expect("row");
        assert_eq!(requeued.status, "queued");
        assert!(!requeued.schedulable, "重取号由 Node 轮询完成");
        assert_eq!(requeued.conversation_id.as_deref(), Some("task-1"));
        assert_eq!(requeued.started_at, Some(NOW));
    }

    #[tokio::test]
    async fn start_recovers_interrupted_running_rows() {
        let dir = TempDir::new("recover");
        let repo = repo(&dir);
        create_ready(&repo, "op-6");
        let runtime = runtime(Arc::clone(&repo), FakeDispatch::ok());
        runtime.tick_at(NOW).await;
        assert_eq!(
            repo.get("op-6").expect("get").expect("row").status,
            "running"
        );

        // 模拟宿主崩溃后重启：新的运行时在 start 时把 running 收回到 queued。
        let restarted = self::runtime(Arc::clone(&repo), FakeDispatch::ok());
        restarted.start();
        let recovered = repo.get("op-6").expect("get").expect("row");
        assert_eq!(recovered.status, "queued");
        restarted.stop().await;
    }

    #[tokio::test]
    async fn stop_unsubscribes_active_dispatch() {
        let dir = TempDir::new("stop");
        let repo = repo(&dir);
        create_ready(&repo, "op-7");
        let dispatch = FakeDispatch::ok();
        let runtime = runtime(Arc::clone(&repo), dispatch.clone());

        runtime.tick_at(NOW).await;
        assert_eq!(
            repo.get("op-7").expect("get").expect("row").status,
            "running"
        );
        runtime.stop().await;
        assert!(runtime.is_disposed());
        assert_eq!(dispatch.unsubscribed_ids(), vec!["op-7".to_string()]);
    }
}
