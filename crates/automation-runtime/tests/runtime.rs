//! AutomationRuntime 调度状态机：认领、misfire、结算、manual 恢复与直派。

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use automation_repo::{
    Automation, AutomationCreateOptions, AutomationCreateParams, AutomationRepo, AutomationRun,
    ModelSelection, RunNowOptions,
};
use automation_runtime::{
    AutomationRuntime, AutomationRuntimeOptions, DispatchPort, ModelSelectionPort,
    ModelSelectionView, PortDispatchRequest, PortDispatchResult, RuntimeFuture, TerminalOutcome,
    Unsubscribe,
};

const NOW: i64 = 1_700_000_000_000;
type TerminalCallback = Arc<dyn Fn(TerminalOutcome) + Send + Sync>;

fn temp_db(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "automation-runtime-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("tasks-index.sqlite")
}

fn sample_selection() -> ModelSelection {
    ModelSelection {
        provider_id: "p1".to_owned(),
        model_id: "m1".to_owned(),
        options: None,
    }
}

struct FakeSelection {
    calls: AtomicUsize,
    preferred: Option<ModelSelection>,
    issue: bool,
}

impl FakeSelection {
    fn new(preferred: Option<ModelSelection>) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            preferred,
            issue: false,
        })
    }
}

impl ModelSelectionPort for FakeSelection {
    fn get_view(
        &self,
        selection: Option<ModelSelection>,
    ) -> RuntimeFuture<'_, Result<ModelSelectionView, String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let preferred = self.preferred.clone();
        let effective = selection.or_else(|| preferred.clone());
        let issue = self.issue;
        Box::pin(async move {
            Ok(ModelSelectionView {
                selection_issue: issue,
                effective_selection: if issue { None } else { effective },
                preferred_selection: preferred,
            })
        })
    }
}

struct FakeDispatch {
    requests: Mutex<Vec<PortDispatchRequest>>,
    terminal: Mutex<Option<TerminalCallback>>,
    unread: Mutex<Vec<String>>,
    unsubscribed: Arc<AtomicUsize>,
    fail: bool,
}

impl FakeDispatch {
    fn new(fail: bool) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            terminal: Mutex::new(None),
            unread: Mutex::new(Vec::new()),
            unsubscribed: Arc::new(AtomicUsize::new(0)),
            fail,
        })
    }

    fn requests(&self) -> Vec<PortDispatchRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn invoke_terminal(&self, run_id: &str, outcome: &str) {
        let callback = self
            .terminal
            .lock()
            .unwrap()
            .clone()
            .expect("terminal callback");
        callback(TerminalOutcome {
            task_id: "task-1".to_owned(),
            input_id: run_id.to_owned(),
            outcome: outcome.to_owned(),
            error: None,
        });
    }
}

impl DispatchPort for FakeDispatch {
    fn dispatch(
        &self,
        request: PortDispatchRequest,
        on_terminal: TerminalCallback,
    ) -> RuntimeFuture<'_, Result<(PortDispatchResult, Unsubscribe), String>> {
        self.requests.lock().unwrap().push(request);
        *self.terminal.lock().unwrap() = Some(on_terminal);
        let fail = self.fail;
        let unsubscribed = Arc::clone(&self.unsubscribed);
        Box::pin(async move {
            if fail {
                return Err("sendPrompt rejected".to_owned());
            }
            Ok((
                PortDispatchResult {
                    task_id: "task-1".to_owned(),
                    session_id: "session-1".to_owned(),
                },
                Box::new(move || {
                    unsubscribed.fetch_add(1, Ordering::SeqCst);
                }) as Unsubscribe,
            ))
        })
    }

    fn mark_task_unread(
        &self,
        task_id: &str,
        _workspace_path: &str,
        _workspace_identity: Option<&str>,
    ) {
        self.unread.lock().unwrap().push(task_id.to_owned());
    }
}

fn runtime(
    repo: Arc<AutomationRepo>,
    dispatch: Arc<FakeDispatch>,
    selection: Arc<FakeSelection>,
    clock: Arc<AtomicI64>,
) -> Arc<AutomationRuntime> {
    let mut options = AutomationRuntimeOptions::new(repo, dispatch, selection);
    options.clock = Arc::new(move || clock.load(Ordering::SeqCst));
    AutomationRuntime::new(options)
}

fn create_automation(
    repo: &AutomationRepo,
    label: &str,
    recurring: bool,
    next_run_at: i64,
) -> Automation {
    repo.create(
        &AutomationCreateParams {
            title: format!("automation {label}"),
            cron_expr: "*/5 * * * *".to_owned(),
            prompt: "run the task".to_owned(),
            workspace_path: "D:/ws/automation".to_owned(),
            recurring,
            ..AutomationCreateParams::default()
        },
        &AutomationCreateOptions {
            next_run_at: Some(next_run_at),
            lifecycle_status: None,
        },
    )
    .expect("create automation")
}

#[tokio::test]
async fn due_automation_dispatches_and_advances_next_run() {
    let db = temp_db("due");
    let repo = Arc::new(AutomationRepo::new(&db));
    let automation = create_automation(&repo, "due", true, NOW);
    let dispatch = FakeDispatch::new(false);
    let selection_port = FakeSelection::new(Some(sample_selection()));
    let clock = Arc::new(AtomicI64::new(NOW));
    let runtime = runtime(
        Arc::clone(&repo),
        Arc::clone(&dispatch),
        selection_port,
        Arc::clone(&clock),
    );

    runtime.tick_at(NOW).await;

    let requests = dispatch.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].trigger, "schedule");
    assert_eq!(requests[0].model_selection, sample_selection());

    let fresh = repo.get(&automation.automation_id, None).unwrap().unwrap();
    assert_eq!(fresh.run_count, 1);
    assert_eq!(fresh.dispatch_status, "dispatched");
    assert!(fresh.next_run_at.unwrap() > NOW);

    let runs = repo.list_runs(&automation.automation_id, None).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].dispatch_status, "dispatched");
    assert_eq!(runs[0].session_id.as_deref(), Some("session-1"));
    assert_eq!(runs[0].model_selection, Some(sample_selection()));
    std::fs::remove_file(&db).ok();
}

#[tokio::test]
async fn missed_window_skips_and_finalizes_one_shot() {
    let db = temp_db("misfire");
    let repo = Arc::new(AutomationRepo::new(&db));
    let automation = create_automation(&repo, "misfire", false, NOW - 6 * 60_000);
    let dispatch = FakeDispatch::new(false);
    let selection_port = FakeSelection::new(Some(sample_selection()));
    let clock = Arc::new(AtomicI64::new(NOW));
    let runtime = runtime(
        Arc::clone(&repo),
        Arc::clone(&dispatch),
        selection_port,
        Arc::clone(&clock),
    );

    runtime.tick_at(NOW).await;

    assert!(dispatch.requests().is_empty(), "错过的窗口不能补跑");
    let fresh = repo.get(&automation.automation_id, None).unwrap().unwrap();
    assert_eq!(fresh.lifecycle_status, "completed");
    assert!(!fresh.enabled);
    assert!(fresh.next_run_at.is_none());
    let runs = repo.list_runs(&automation.automation_id, None).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].dispatch_status, "skipped");
    assert_eq!(
        runs[0].error.as_deref(),
        Some("computer_asleep_or_app_not_running")
    );
    std::fs::remove_file(&db).ok();
}

#[tokio::test]
async fn dispatch_failure_backs_off_without_losing_the_run() {
    let db = temp_db("failure");
    let repo = Arc::new(AutomationRepo::new(&db));
    let automation = create_automation(&repo, "failure", true, NOW);
    let dispatch = FakeDispatch::new(true);
    let selection_port = FakeSelection::new(Some(sample_selection()));
    let clock = Arc::new(AtomicI64::new(NOW));
    let runtime = runtime(
        Arc::clone(&repo),
        Arc::clone(&dispatch),
        selection_port,
        Arc::clone(&clock),
    );

    runtime.tick_at(NOW).await;

    let fresh = repo.get(&automation.automation_id, None).unwrap().unwrap();
    assert_eq!(fresh.dispatch_status, "failed_to_dispatch");
    assert_eq!(fresh.dispatch_attempts, 1);
    assert!(fresh.retry_at.unwrap() > NOW);
    let runs = repo.list_runs(&automation.automation_id, None).unwrap();
    assert_eq!(runs[0].dispatch_status, "failed_to_dispatch");
    assert_eq!(runs[0].error.as_deref(), Some("sendPrompt rejected"));
    std::fs::remove_file(&db).ok();
}

#[tokio::test]
async fn stale_manual_run_is_recovered_then_released_on_terminal() {
    let db = temp_db("manual");
    let repo = Arc::new(AutomationRepo::new(&db));
    let automation = create_automation(&repo, "manual", true, NOW);
    let dispatch = FakeDispatch::new(false);
    let selection_port = FakeSelection::new(Some(sample_selection()));
    let clock = Arc::new(AtomicI64::new(NOW));
    let runtime = runtime(
        Arc::clone(&repo),
        Arc::clone(&dispatch),
        selection_port,
        Arc::clone(&clock),
    );

    let claimed = repo
        .run_now(&automation.automation_id, &RunNowOptions { now: NOW }, None)
        .unwrap()
        .expect("首次立即运行应成功占锁");
    let run: AutomationRun = claimed.run;
    // 模拟派发进程崩溃：run 停留 claimed，等 claim_manual_runs 的 stale 窗口。
    let recover_at = NOW + automation_repo::CLAIM_STALE_MS + 1;
    clock.store(recover_at, Ordering::SeqCst);
    runtime.tick_at(recover_at).await;

    let requests = dispatch.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].trigger, "manual");
    assert_eq!(requests[0].run_id, run.run_id);
    let recovered = repo.get_run(&run.run_id).unwrap().unwrap();
    assert_eq!(recovered.dispatch_status, "dispatched");

    dispatch.invoke_terminal(&run.run_id, "completed");
    let settled = repo.get_run(&run.run_id).unwrap().unwrap();
    assert_eq!(settled.outcome.as_deref(), Some("completed"));
    assert_eq!(dispatch.unread.lock().unwrap().as_slice(), ["task-1"]);
    // 终态后 single-flight claim 释放：再次立即运行可以重新占锁。
    assert!(repo
        .run_now(
            &automation.automation_id,
            &RunNowOptions { now: recover_at },
            None,
        )
        .unwrap()
        .is_some());
    std::fs::remove_file(&db).ok();
}

#[tokio::test]
async fn direct_manual_dispatch_releases_claim_on_failure() {
    let db = temp_db("manual-failure");
    let repo = Arc::new(AutomationRepo::new(&db));
    let automation = create_automation(&repo, "manual-failure", true, NOW);
    let dispatch = FakeDispatch::new(true);
    let selection_port = FakeSelection::new(Some(sample_selection()));
    let clock = Arc::new(AtomicI64::new(NOW));
    let runtime = runtime(
        Arc::clone(&repo),
        Arc::clone(&dispatch),
        selection_port,
        Arc::clone(&clock),
    );

    let claimed = repo
        .run_now(&automation.automation_id, &RunNowOptions { now: NOW }, None)
        .unwrap()
        .expect("立即运行占锁");
    let error = runtime
        .dispatch_manual_run(&claimed.automation, &claimed.run)
        .await
        .unwrap_err();
    assert_eq!(error, "sendPrompt rejected");
    let failed = repo.get_run(&claimed.run.run_id).unwrap().unwrap();
    assert_eq!(failed.dispatch_status, "failed_to_dispatch");
    // 失败已释放 claim，UI 可再次触发。
    assert!(repo
        .run_now(
            &automation.automation_id,
            &RunNowOptions { now: NOW + 1 },
            None,
        )
        .unwrap()
        .is_some());
    std::fs::remove_file(&db).ok();
}
