//! Automation 调度运行时（Rust）：认领 → 派发 → 结算。
//!
//! 真源：`packages/services/src/session/automationRuntime.ts` 与
//! `docs/specs/automation-runtime.md`。调度状态机的唯一写入方仍是
//! `AutomationRepo`；本模块只按 repo 契约调用。派发（建/恢复 task、模型选择解析、
//! Bot 回推、sendPrompt、真实 turn 终态）由注入的 [`DispatchPort`] 承担，因此本 crate
//! 不依赖 Agent 会话运行时，也便于在所有权翻面前默认关闭。

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use automation_repo::{
    Automation, AutomationRepo, AutomationRun, ClaimedManualAutomationRun, DispatchFailureKind,
    EnsureRunClaimedParams, MarkDispatchFailedOptions, MarkDispatchedOptions,
    MarkManualRunDispatchedParams, MarkRunDispatchParams, ModelSelection, SkipAndRescheduleParams,
    UpsertRunClaimedParams,
};
use automation_schedule::{compute_automation_next_run_at, is_one_shot_automation, ScheduleRule};
use tokio::task::JoinHandle;

pub const DEFAULT_POLL_INTERVAL_MS: u64 = 20_000;
pub const DEFAULT_MISFIRE_GRACE_MS: i64 = 5 * 60_000;
pub const DEFAULT_CLAIM_HEARTBEAT_MS: u64 = 60_000;

pub type RuntimeFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
pub type Unsubscribe = Box<dyn FnOnce() + Send + 'static>;
pub type RuntimeLog = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// 一次派发的完整上下文；`model_selection` 已由 runtime 解析并固化。
#[derive(Clone, Debug)]
pub struct PortDispatchRequest {
    pub automation_id: String,
    pub run_id: String,
    pub prompt: String,
    pub target_task_id: Option<String>,
    pub mode: Option<String>,
    pub model_selection: ModelSelection,
    pub workspace_path: String,
    pub workspace_identity: Option<String>,
    pub workspace_key: String,
    pub trigger: String,
    pub scheduled_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct PortDispatchResult {
    pub task_id: String,
    pub session_id: String,
}

/// 真实 turn 终态；`input_id` 必须等于 `run_id`（与 TS `inputId === traceId` 一致）。
#[derive(Clone, Debug)]
pub struct TerminalOutcome {
    pub task_id: String,
    pub input_id: String,
    pub outcome: String,
    pub error: Option<String>,
}

/// 派发端口：承担 TS `dispatchRun` 与终态订阅注册（必须先于 sendPrompt）。
pub trait DispatchPort: Send + Sync {
    /// 创建/恢复 task、Bot 回推、注册终态回调并 `sendPrompt`；返回 task/session 与退订句柄。
    fn dispatch(
        &self,
        request: PortDispatchRequest,
        on_terminal: Arc<dyn Fn(TerminalOutcome) + Send + Sync>,
    ) -> RuntimeFuture<'_, Result<(PortDispatchResult, Unsubscribe), String>>;

    /// `setTaskUnread` 的 best-effort 投影；失败不影响结算。
    fn mark_task_unread(
        &self,
        task_id: &str,
        workspace_path: &str,
        workspace_identity: Option<&str>,
    );
}

#[derive(Clone, Debug, Default)]
pub struct ModelSelectionView {
    pub selection_issue: bool,
    pub effective_selection: Option<ModelSelection>,
    pub preferred_selection: Option<ModelSelection>,
}

/// 模型选择端口：对应 `IModelSelectionService.getView`。
pub trait ModelSelectionPort: Send + Sync {
    fn get_view(
        &self,
        selection: Option<ModelSelection>,
    ) -> RuntimeFuture<'_, Result<ModelSelectionView, String>>;
}

pub struct AutomationRuntimeOptions {
    pub repo: Arc<AutomationRepo>,
    pub dispatch: Arc<dyn DispatchPort>,
    pub model_selection: Arc<dyn ModelSelectionPort>,
    pub utc_offset_secs: i32,
    pub poll_interval_ms: u64,
    pub misfire_grace_ms: i64,
    pub claim_heartbeat_ms: u64,
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub log: RuntimeLog,
}

impl AutomationRuntimeOptions {
    pub fn new(
        repo: Arc<AutomationRepo>,
        dispatch: Arc<dyn DispatchPort>,
        model_selection: Arc<dyn ModelSelectionPort>,
    ) -> Self {
        Self {
            repo,
            dispatch,
            model_selection,
            utc_offset_secs: 0,
            poll_interval_ms: DEFAULT_POLL_INTERVAL_MS,
            misfire_grace_ms: DEFAULT_MISFIRE_GRACE_MS,
            claim_heartbeat_ms: DEFAULT_CLAIM_HEARTBEAT_MS,
            clock: Arc::new(now_millis),
            log: Arc::new(|_, _| {}),
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
struct RunRequest {
    automation_id: String,
    run_id: String,
    prompt: String,
    target_task_id: Option<String>,
    mode: Option<String>,
    workspace_path: String,
    workspace_identity: Option<String>,
    workspace_key: String,
    trigger: String,
    scheduled_at: Option<i64>,
}

struct ActiveRun {
    trigger: String,
    automation_id: String,
    workspace_key: String,
    heartbeat: Option<JoinHandle<()>>,
    unsubscribe: Option<Unsubscribe>,
    terminal: bool,
}

#[derive(Default)]
struct RuntimeState {
    started: bool,
    disposed: bool,
    active_runs: HashMap<String, ActiveRun>,
    schedule_claims: HashSet<String>,
    poll_task: Option<JoinHandle<()>>,
}

pub struct AutomationRuntime {
    repo: Arc<AutomationRepo>,
    dispatch: Arc<dyn DispatchPort>,
    model_selection: Arc<dyn ModelSelectionPort>,
    utc_offset_secs: i32,
    poll_interval: Duration,
    misfire_grace_ms: i64,
    claim_heartbeat: Duration,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    log: RuntimeLog,
    state: Mutex<RuntimeState>,
    tick_gate: tokio::sync::Mutex<()>,
    tick_requested: AtomicBool,
}

impl AutomationRuntime {
    pub fn new(options: AutomationRuntimeOptions) -> Arc<Self> {
        Arc::new(Self {
            repo: options.repo,
            dispatch: options.dispatch,
            model_selection: options.model_selection,
            utc_offset_secs: options.utc_offset_secs,
            poll_interval: Duration::from_millis(options.poll_interval_ms.max(1)),
            misfire_grace_ms: options.misfire_grace_ms,
            claim_heartbeat: Duration::from_millis(options.claim_heartbeat_ms.max(1)),
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

    /// 启动 20s 轮询并立即跑一轮；幂等。
    pub fn start(self: &Arc<Self>) {
        {
            let mut state = self.lock();
            if state.started || state.disposed {
                return;
            }
            state.started = true;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            self.log("warn", "automation runtime 无 Tokio 上下文，未启动轮询");
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
                "automation runtime started pollIntervalMs={}",
                self.poll_interval.as_millis()
            ),
        );
    }

    /// 停止轮询、等待在途 tick、释放本进程仍持有的认领。
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
        // 在途 tick 可能刚注册终态订阅；先等它收尾再统一释放。
        let _guard = self.tick_gate.lock().await;
        let (runs, schedule_claims) = {
            let mut state = self.lock();
            (
                std::mem::take(&mut state.active_runs),
                std::mem::take(&mut state.schedule_claims),
            )
        };
        for (_, mut entry) in runs {
            if let Some(heartbeat) = entry.heartbeat.take() {
                heartbeat.abort();
            }
            if let Some(unsubscribe) = entry.unsubscribe.take() {
                unsubscribe();
            }
            let released = if entry.trigger == "manual" {
                self.repo
                    .release_manual_claim(&entry.automation_id, &entry.workspace_key)
            } else {
                self.repo.release_claim(&entry.automation_id)
            };
            if let Err(error) = released {
                self.log("warn", format!("automation stop 释放认领失败: {error}"));
            }
        }
        for automation_id in schedule_claims {
            if let Err(error) = self.repo.release_claim(&automation_id) {
                self.log("warn", format!("automation stop 释放认领失败: {error}"));
            }
        }
        self.log("info", "automation runtime stopped");
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
                self.log("error", format!("automation tick failed: {error}"));
                return;
            }
        };
        for automation in claimed {
            self.handle_claimed_schedule(&automation, now).await;
        }
        let manual_runs = match self.repo.claim_manual_runs(now) {
            Ok(runs) => runs,
            Err(error) => {
                self.log("error", format!("automation tick failed: {error}"));
                return;
            }
        };
        for claimed in manual_runs {
            self.handle_claimed_manual_recovery(claimed).await;
        }
    }

    async fn handle_claimed_schedule(self: &Arc<Self>, automation: &Automation, now: i64) {
        let scheduled_at = automation
            .next_run_at
            .or(automation.retry_at)
            .unwrap_or(now);
        let run_id = format!("{}:{}", automation.automation_id, scheduled_at);
        let workspace_key = workspace_identity::workspace_key(
            &automation.workspace_path,
            automation.workspace_identity.as_deref(),
        );
        let is_retry = automation.dispatch_attempts > 0;
        let missed = !is_retry
            && automation
                .next_run_at
                .is_some_and(|next| next <= now - self.misfire_grace_ms);
        if missed {
            let finalize = is_one_shot_automation(automation.recurring, automation.max_runs);
            let next_run_at = if finalize {
                None
            } else {
                self.next_run_at(automation, now)
            };
            let result = self.repo.skip_and_reschedule(&SkipAndRescheduleParams {
                automation_id: automation.automation_id.clone(),
                run_id,
                workspace_key,
                scheduled_at: Some(scheduled_at),
                reason: "computer_asleep_or_app_not_running".to_owned(),
                next_run_at,
                finalize,
            });
            match result {
                Ok(()) => self.log(
                    "info",
                    format!(
                        "skip missed window automation={} scheduledAt={scheduled_at}{}",
                        automation.automation_id,
                        if finalize { " finalized=one-shot" } else { "" }
                    ),
                ),
                Err(error) => self.log(
                    "error",
                    format!(
                        "skip missed window 失败 automation={}: {error}",
                        automation.automation_id
                    ),
                ),
            }
            return;
        }

        if let Err(error) = self.repo.upsert_run_claimed(&UpsertRunClaimedParams {
            run_id: run_id.clone(),
            automation_id: automation.automation_id.clone(),
            workspace_key: workspace_key.clone(),
            scheduled_at: Some(scheduled_at),
            trigger: "schedule".to_owned(),
            model_selection: automation.model_selection.clone(),
        }) {
            self.log(
                "error",
                format!(
                    "认领定时任务 run 失败 automation={}: {error}",
                    automation.automation_id
                ),
            );
            let _ = self.repo.release_claim(&automation.automation_id);
            return;
        }
        self.lock()
            .schedule_claims
            .insert(automation.automation_id.clone());

        let request = RunRequest {
            automation_id: automation.automation_id.clone(),
            run_id: run_id.clone(),
            prompt: automation.prompt.clone(),
            target_task_id: automation.target_task_id.clone(),
            mode: automation.mode.clone(),
            workspace_path: automation.workspace_path.clone(),
            workspace_identity: automation.workspace_identity.clone(),
            workspace_key,
            trigger: "schedule".to_owned(),
            scheduled_at: Some(scheduled_at),
        };
        match self.dispatch_run(&request).await {
            Ok(result) => {
                self.settle_scheduled_dispatch(automation, &request, Ok(result))
                    .await;
                self.log(
                    "info",
                    format!(
                        "automation dispatched automation={} runId={run_id}",
                        automation.automation_id
                    ),
                );
            }
            Err(error) => {
                self.settle_scheduled_dispatch(automation, &request, Err(error.clone()))
                    .await;
                self.log(
                    "warn",
                    format!(
                        "automation dispatch failed automation={} runId={run_id}: {error}",
                        automation.automation_id
                    ),
                );
            }
        }
    }

    async fn handle_claimed_manual_recovery(self: &Arc<Self>, claimed: ClaimedManualAutomationRun) {
        let request = RunRequest {
            automation_id: claimed.automation.automation_id.clone(),
            run_id: claimed.run.run_id.clone(),
            prompt: claimed.automation.prompt.clone(),
            target_task_id: claimed.automation.target_task_id.clone(),
            mode: claimed.automation.mode.clone(),
            workspace_path: claimed.automation.workspace_path.clone(),
            workspace_identity: claimed.automation.workspace_identity.clone(),
            workspace_key: claimed.run.workspace_key.clone(),
            trigger: "manual".to_owned(),
            scheduled_at: claimed.run.scheduled_at,
        };
        match self.dispatch_run(&request).await {
            Ok(result) => {
                let persisted =
                    self.repo
                        .mark_manual_run_dispatched(&MarkManualRunDispatchedParams {
                            run_id: request.run_id.clone(),
                            session_id: Some(result.session_id),
                            dispatched_at: (self.clock)(),
                        });
                match persisted {
                    Ok(_) => self.log(
                        "info",
                        format!(
                            "manual run recovery dispatched runId={} taskId={}",
                            request.run_id, result.task_id
                        ),
                    ),
                    Err(error) => self.log(
                        "warn",
                        format!(
                            "回写 manual run 派发台账失败 runId={}: {error}",
                            request.run_id
                        ),
                    ),
                }
            }
            Err(error) => {
                self.log(
                    "warn",
                    format!(
                        "manual run recovery dispatch failed runId={}: {error}",
                        request.run_id
                    ),
                );
                self.settle_manual_dispatch_failure(&request, &error);
            }
        }
    }

    /// 「立即运行」直派：成功结算台账，失败释放 claim 并把错误交回 UI。
    pub async fn dispatch_manual_run(
        self: &Arc<Self>,
        automation: &Automation,
        run: &AutomationRun,
    ) -> Result<(), String> {
        if self.is_disposed() {
            return Err("Automation runtime is disposed.".to_owned());
        }
        let request = RunRequest {
            automation_id: automation.automation_id.clone(),
            run_id: run.run_id.clone(),
            prompt: automation.prompt.clone(),
            target_task_id: automation.target_task_id.clone(),
            mode: automation.mode.clone(),
            workspace_path: automation.workspace_path.clone(),
            workspace_identity: automation.workspace_identity.clone(),
            workspace_key: run.workspace_key.clone(),
            trigger: "manual".to_owned(),
            scheduled_at: run.scheduled_at,
        };
        let outcome = self.dispatch_run(&request).await;
        match &outcome {
            Ok(result) => {
                let persisted =
                    self.repo
                        .mark_manual_run_dispatched(&MarkManualRunDispatchedParams {
                            run_id: request.run_id.clone(),
                            session_id: Some(result.session_id.clone()),
                            dispatched_at: (self.clock)(),
                        });
                match persisted {
                    Ok(_) => self.log(
                        "info",
                        format!(
                            "manual automation dispatched automation={} runId={}",
                            request.automation_id, request.run_id
                        ),
                    ),
                    Err(error) => self.log(
                        "warn",
                        format!(
                            "回写 manual automation 派发台账失败 runId={}: {error}",
                            request.run_id
                        ),
                    ),
                }
            }
            Err(error) => {
                self.log(
                    "warn",
                    format!(
                        "manual automation dispatch failed automation={} runId={}: {error}",
                        request.automation_id, request.run_id
                    ),
                );
                self.settle_manual_dispatch_failure(&request, error);
            }
        }
        // 直派已结算或已失败；唤醒轮询补认领同 workspace 的到期 schedule 任务。
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let runtime = Arc::clone(self);
            handle.spawn(async move { runtime.request_tick().await });
        }
        outcome.map(|_| ())
    }

    fn settle_manual_dispatch_failure(&self, request: &RunRequest, error: &str) {
        if let Err(settle_error) = self.repo.mark_run_dispatch(&MarkRunDispatchParams {
            run_id: request.run_id.clone(),
            dispatch_status: "failed_to_dispatch".to_owned(),
            session_id: None,
            error: Some(error.to_owned()),
        }) {
            self.log(
                "error",
                format!(
                    "结算 manual run 派发失败 runId={}: {settle_error}",
                    request.run_id
                ),
            );
        }
        if let Err(settle_error) = self
            .repo
            .release_manual_claim(&request.automation_id, &request.workspace_key)
        {
            self.log(
                "error",
                format!(
                    "释放 manual automation claim 失败 automation={}: {settle_error}",
                    request.automation_id
                ),
            );
        }
    }

    async fn settle_scheduled_dispatch(
        self: &Arc<Self>,
        automation: &Automation,
        request: &RunRequest,
        outcome: Result<PortDispatchResult, String>,
    ) {
        self.lock().schedule_claims.remove(&request.automation_id);
        let now = (self.clock)();
        match outcome {
            Ok(result) => {
                if let Err(error) = self.repo.mark_run_dispatch(&MarkRunDispatchParams {
                    run_id: request.run_id.clone(),
                    dispatch_status: "dispatched".to_owned(),
                    session_id: Some(result.session_id),
                    error: None,
                }) {
                    self.log(
                        "error",
                        format!(
                            "结算定时任务派发失败 automation={}: {error}",
                            request.automation_id
                        ),
                    );
                }
                let fresh = self.repo.get(&request.automation_id, None).ok().flatten();
                let next_run_at = fresh
                    .as_ref()
                    .and_then(|fresh| self.next_run_at(fresh, now));
                if let Err(error) = self.repo.mark_dispatched(
                    &request.automation_id,
                    &MarkDispatchedOptions {
                        dispatched_at: now,
                        next_run_at,
                    },
                ) {
                    self.log(
                        "error",
                        format!(
                            "结算定时任务派发失败 automation={}: {error}",
                            request.automation_id
                        ),
                    );
                }
            }
            Err(error) => {
                if let Err(settle_error) = self.repo.mark_run_dispatch(&MarkRunDispatchParams {
                    run_id: request.run_id.clone(),
                    dispatch_status: "failed_to_dispatch".to_owned(),
                    session_id: None,
                    error: Some(error.clone()),
                }) {
                    self.log(
                        "error",
                        format!(
                            "结算定时任务派发失败 automation={}: {settle_error}",
                            request.automation_id
                        ),
                    );
                }
                let fresh = self.repo.get(&request.automation_id, None).ok().flatten();
                let next_run_at = fresh
                    .as_ref()
                    .and_then(|fresh| self.next_run_at(fresh, now));
                if let Err(settle_error) = self.repo.mark_dispatch_failed(
                    &request.automation_id,
                    &MarkDispatchFailedOptions {
                        failed_at: now,
                        error,
                        // 确定性配置错误重试不会自愈，但与 TS 一致统一按 transient 退避。
                        kind: DispatchFailureKind::Transient,
                        next_run_at,
                    },
                ) {
                    self.log(
                        "error",
                        format!(
                            "结算定时任务派发失败 automation={}: {settle_error}",
                            request.automation_id
                        ),
                    );
                }
                let _ = automation;
            }
        }
    }

    fn next_run_at(&self, automation: &Automation, from: i64) -> Option<i64> {
        let rule = automation
            .schedule_rule
            .as_ref()
            .and_then(|value| serde_json::from_value::<ScheduleRule>(value.clone()).ok());
        if automation.schedule_rule.is_some() && rule.is_none() {
            self.log(
                "warn",
                format!(
                    "automation scheduleRule 无法解析 automation={}",
                    automation.automation_id
                ),
            );
            return None;
        }
        compute_automation_next_run_at(
            &automation.cron_expr,
            rule.as_ref(),
            from,
            self.utc_offset_secs,
        )
    }

    async fn dispatch_run(
        self: &Arc<Self>,
        request: &RunRequest,
    ) -> Result<PortDispatchResult, String> {
        let selection = self.resolve_model_selection(request).await?;
        let port_request = PortDispatchRequest {
            automation_id: request.automation_id.clone(),
            run_id: request.run_id.clone(),
            prompt: request.prompt.clone(),
            target_task_id: request.target_task_id.clone(),
            mode: request.mode.clone(),
            model_selection: selection,
            workspace_path: request.workspace_path.clone(),
            workspace_identity: request.workspace_identity.clone(),
            workspace_key: request.workspace_key.clone(),
            trigger: request.trigger.clone(),
            scheduled_at: request.scheduled_at,
        };
        let callback = self.terminal_callback(request);
        {
            let mut state = self.lock();
            state.active_runs.insert(
                request.run_id.clone(),
                ActiveRun {
                    trigger: request.trigger.clone(),
                    automation_id: request.automation_id.clone(),
                    workspace_key: request.workspace_key.clone(),
                    heartbeat: None,
                    unsubscribe: None,
                    terminal: false,
                },
            );
        }
        let dispatched = self.dispatch.dispatch(port_request, callback).await;
        let (result, unsubscribe) = match dispatched {
            Ok(value) => value,
            Err(error) => {
                if let Some(mut entry) = self.lock().active_runs.remove(&request.run_id) {
                    if let Some(heartbeat) = entry.heartbeat.take() {
                        heartbeat.abort();
                    }
                    if let Some(unsubscribe) = entry.unsubscribe.take() {
                        unsubscribe();
                    }
                }
                return Err(error);
            }
        };
        let mut unsubscribe = Some(unsubscribe);
        let running = {
            let mut state = self.lock();
            match state.active_runs.get_mut(&request.run_id) {
                Some(entry) => {
                    entry.unsubscribe = unsubscribe.take();
                    if !entry.terminal && request.trigger == "manual" {
                        entry.heartbeat = Some(
                            self.spawn_heartbeat(&request.automation_id, &request.workspace_key),
                        );
                    }
                    !entry.terminal
                }
                None => false,
            }
        };
        if let Some(unsubscribe) = unsubscribe {
            // 终态回调已在本轮返回前清理：立即退订，避免句柄泄漏。
            unsubscribe();
        }
        if running {
            let ensure = EnsureRunClaimedParams {
                run_id: request.run_id.clone(),
                automation_id: request.automation_id.clone(),
                workspace_key: request.workspace_key.clone(),
                scheduled_at: request.scheduled_at,
                trigger: request.trigger.clone(),
            };
            if let Err(error) = self.repo.ensure_run_claimed(&ensure) {
                self.log(
                    "warn",
                    format!(
                        "回写定时任务 running 状态失败 runId={}: {error}",
                        request.run_id
                    ),
                );
            } else if let Err(error) = self.repo.mark_run_outcome(&request.run_id, "running", None)
            {
                self.log(
                    "warn",
                    format!(
                        "回写定时任务 running 状态失败 runId={}: {error}",
                        request.run_id
                    ),
                );
            }
        }
        Ok(result)
    }

    async fn resolve_model_selection(
        &self,
        request: &RunRequest,
    ) -> Result<ModelSelection, String> {
        let existing = self
            .repo
            .get_run(&request.run_id)
            .map_err(|error| error.to_string())?;
        if let Some(selection) = existing.and_then(|run| run.model_selection) {
            return Ok(selection);
        }
        let persisted = self
            .repo
            .get_model_selection_for_dispatch(&request.automation_id, &request.workspace_key)
            .map_err(|error| error.to_string())?;
        let chosen = match persisted {
            Some(selection) => {
                let view = self
                    .model_selection
                    .get_view(Some(selection))
                    .await
                    .map_err(|error| error.to_string())?;
                if view.selection_issue {
                    return Err("Automation 模型选择不可用，请重新选择模型与思考档位".to_owned());
                }
                view.effective_selection.ok_or_else(|| {
                    "Automation 模型选择不可用，请重新选择模型与思考档位".to_owned()
                })?
            }
            None => {
                let view = self
                    .model_selection
                    .get_view(None)
                    .await
                    .map_err(|error| error.to_string())?;
                view.preferred_selection
                    .ok_or_else(|| "Automation 无法从目标 Host 解析首选模型".to_owned())?
            }
        };
        self.repo
            .fix_run_model_selection(&request.run_id, &chosen)
            .map_err(|error| error.to_string())
    }

    fn terminal_callback(
        self: &Arc<Self>,
        request: &RunRequest,
    ) -> Arc<dyn Fn(TerminalOutcome) + Send + Sync> {
        let runtime = Arc::downgrade(self);
        let run_id = request.run_id.clone();
        let automation_id = request.automation_id.clone();
        let workspace_key = request.workspace_key.clone();
        let workspace_path = request.workspace_path.clone();
        let workspace_identity = request.workspace_identity.clone();
        let trigger = request.trigger.clone();
        let scheduled_at = request.scheduled_at;
        Arc::new(move |outcome: TerminalOutcome| {
            if outcome.input_id != run_id {
                return;
            }
            let Some(runtime) = runtime.upgrade() else {
                return;
            };
            runtime.handle_terminal(
                &run_id,
                &automation_id,
                &workspace_key,
                &workspace_path,
                workspace_identity.as_deref(),
                &trigger,
                scheduled_at,
                outcome,
            );
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_terminal(
        &self,
        run_id: &str,
        automation_id: &str,
        workspace_key: &str,
        workspace_path: &str,
        workspace_identity: Option<&str>,
        trigger: &str,
        scheduled_at: Option<i64>,
        outcome: TerminalOutcome,
    ) {
        {
            let mut state = self.lock();
            let Some(entry) = state.active_runs.get_mut(run_id) else {
                return;
            };
            if entry.terminal {
                return;
            }
            entry.terminal = true;
        }
        let ensure = EnsureRunClaimedParams {
            run_id: run_id.to_owned(),
            automation_id: automation_id.to_owned(),
            workspace_key: workspace_key.to_owned(),
            scheduled_at,
            trigger: trigger.to_owned(),
        };
        if let Err(error) = self.repo.ensure_run_claimed(&ensure) {
            self.log(
                "warn",
                format!("回写定时任务终态失败 runId={run_id}: {error}"),
            );
        } else if let Err(error) =
            self.repo
                .mark_run_outcome(run_id, &outcome.outcome, outcome.error.as_deref())
        {
            self.log(
                "warn",
                format!("回写定时任务终态失败 runId={run_id}: {error}"),
            );
        }
        if trigger == "manual" {
            if let Err(error) = self.repo.release_manual_claim(automation_id, workspace_key) {
                self.log(
                    "warn",
                    format!(
                        "释放 manual automation claim 失败 automation={automation_id}: {error}"
                    ),
                );
            }
        }
        self.dispatch
            .mark_task_unread(&outcome.task_id, workspace_path, workspace_identity);
        let entry = self.lock().active_runs.remove(run_id);
        if let Some(mut entry) = entry {
            if let Some(heartbeat) = entry.heartbeat.take() {
                heartbeat.abort();
            }
            if let Some(unsubscribe) = entry.unsubscribe.take() {
                unsubscribe();
            }
        }
    }

    fn spawn_heartbeat(
        self: &Arc<Self>,
        automation_id: &str,
        workspace_key: &str,
    ) -> JoinHandle<()> {
        let runtime = Arc::clone(self);
        let automation_id = automation_id.to_owned();
        let workspace_key = workspace_key.to_owned();
        let period = self.claim_heartbeat;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(period);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if runtime.is_disposed() {
                    break;
                }
                if let Err(error) = runtime
                    .repo
                    .touch_manual_claim(&automation_id, &workspace_key)
                {
                    runtime.log(
                        "warn",
                        format!(
                            "续租 manual automation claim 失败 automation={automation_id}: {error}"
                        ),
                    );
                }
            }
        })
    }
}
