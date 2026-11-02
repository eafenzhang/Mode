//! 取号/Continue/轮询/核销/3102 续跑的行为测试。
//!
//! TS 真源：`packages/services/src/session/offPeakTaskService.ts`。本片暂未补跨语言夹具，
//! 用真实临时 SQLite + 桩客户端验证状态机与调用顺序。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use model_selection::ModelSelection;
use off_peak_repo::{
    CreateOptions, CreateParams, InvalidateOptions, MarkRunningOptions, MarkTerminalOptions,
    OffPeakTaskRepo,
};
use off_peak_service::{
    OffPeakBatchStatusResult, OffPeakClientError, OffPeakServerClient,
    OffPeakTakeNumberAvailability, OffPeakTakeTicketResult, OffPeakTaskCreateParams,
    OffPeakTaskEnvironment, OffPeakTaskService, OffPeakTicketStatusEntry,
};
use serde_json::Value;

const NOW: i64 = 1_700_000_000_000;

struct TempDb(PathBuf);

impl TempDb {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-off-peak-remote-{label}-{}-{}.sqlite",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("sqlite-shm"));
        let _ = std::fs::remove_file(self.0.with_extension("sqlite-wal"));
    }
}

enum TakeScript {
    Ok(OffPeakTakeTicketResult),
    Err(OffPeakClientError),
}

#[derive(Default)]
struct FakeClient {
    take: Mutex<VecDeque<TakeScript>>,
    batch: Mutex<Option<Result<OffPeakBatchStatusResult, OffPeakClientError>>>,
    settle: Mutex<Vec<(String, Result<(), OffPeakClientError>)>>,
    availability: Mutex<Option<OffPeakTakeNumberAvailability>>,
    calls: Mutex<Vec<Value>>,
}

impl FakeClient {
    fn push_ticket(&self, ticket: OffPeakTakeTicketResult) {
        self.take.lock().unwrap().push_back(TakeScript::Ok(ticket));
    }
    fn push_ticket_error(&self, error: OffPeakClientError) {
        self.take.lock().unwrap().push_back(TakeScript::Err(error));
    }
    fn ticket(
        id: &str,
        state: &str,
        position: Option<i64>,
        next_poll: Option<i64>,
    ) -> OffPeakTakeTicketResult {
        OffPeakTakeTicketResult {
            ticket_id: id.to_string(),
            state: state.to_string(),
            position,
            next_poll_after_ms: next_poll,
            registered_at: NOW,
        }
    }
    fn batch(&self, batch: Result<OffPeakBatchStatusResult, OffPeakClientError>) {
        *self.batch.lock().unwrap() = Some(batch);
    }
    fn calls(&self) -> Vec<Value> {
        self.calls.lock().unwrap().clone()
    }
    fn take_count(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call["kind"] == "take")
            .count()
    }
}

impl OffPeakServerClient for FakeClient {
    fn get_take_number_availability(
        &self,
    ) -> Result<OffPeakTakeNumberAvailability, OffPeakClientError> {
        self.calls
            .lock()
            .unwrap()
            .push(serde_json::json!({ "kind": "availability" }));
        self.availability
            .lock()
            .unwrap()
            .clone()
            .ok_or(OffPeakClientError::Network)
    }
    fn take_ticket(&self, task_id: &str) -> Result<OffPeakTakeTicketResult, OffPeakClientError> {
        self.calls
            .lock()
            .unwrap()
            .push(serde_json::json!({ "kind": "take", "taskId": task_id }));
        match self.take.lock().unwrap().pop_front() {
            Some(TakeScript::Ok(ticket)) => Ok(ticket),
            Some(TakeScript::Err(error)) => Err(error),
            None => Err(OffPeakClientError::Network),
        }
    }
    fn batch_status(
        &self,
        ticket_ids: &[String],
    ) -> Result<OffPeakBatchStatusResult, OffPeakClientError> {
        self.calls.lock().unwrap().push(serde_json::json!({
            "kind": "batch",
            "ticketIds": ticket_ids,
        }));
        self.batch
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(Err(OffPeakClientError::Network))
    }
    fn settle(&self, ticket_id: &str) -> Result<(), OffPeakClientError> {
        self.calls.lock().unwrap().push(serde_json::json!({
            "kind": "settle",
            "ticketId": ticket_id,
        }));
        let mut entries = self.settle.lock().unwrap();
        if let Some(index) = entries.iter().position(|(id, _)| id == ticket_id) {
            return entries.remove(index).1;
        }
        Ok(())
    }
}

struct Env {
    events: Arc<Mutex<Vec<Value>>>,
}

impl OffPeakTaskEnvironment for Env {
    fn resolve_model_selection(&self, selection: &ModelSelection) -> Option<ModelSelection> {
        if selection.model_id.starts_with("bad") {
            return None;
        }
        Some(ModelSelection {
            provider_id: "provider-1".to_string(),
            model_id: selection.model_id.clone(),
            options: selection.options.clone(),
        })
    }
    fn on_tasks_changed(&self) {
        self.events
            .lock()
            .unwrap()
            .push(serde_json::json!({ "kind": "changed" }));
    }
    fn request_scheduler_wake(&self) {
        self.events
            .lock()
            .unwrap()
            .push(serde_json::json!({ "kind": "wake" }));
    }
    fn coding_plan_support(&self) -> Value {
        serde_json::json!({ "supported": true })
    }
    fn telemetry_provider_name(&self) -> Result<String, String> {
        Ok("provider".to_string())
    }
}

fn selection() -> ModelSelection {
    ModelSelection {
        provider_id: "provider-1".to_string(),
        model_id: "m1".to_string(),
        options: None,
    }
}

fn params() -> OffPeakTaskCreateParams {
    OffPeakTaskCreateParams {
        title: "T".to_string(),
        prompt: "P".to_string(),
        permission_mode: "edit".to_string(),
        model_selection: selection(),
        workspace_path: "/ws/demo".to_string(),
        workspace_identity: None,
        bound_session_id: None,
    }
}

fn seed(repo: &OffPeakTaskRepo, id: &str, state: &str, ticket: Option<&str>, bound: Option<&str>) {
    repo.create(
        &CreateParams {
            title: id.to_string(),
            prompt: "p".to_string(),
            permission_mode: "edit".to_string(),
            model_selection: selection(),
            workspace_path: "/ws/demo".to_string(),
            workspace_identity: None,
            bound_session_id: bound.map(str::to_string),
        },
        &CreateOptions {
            now: Some(NOW),
            off_peak_task_id: Some(id.to_string()),
            server_ticket_id: ticket.map(str::to_string),
            registered_at: ticket.map(|_| NOW),
            ..CreateOptions::default()
        },
    )
    .unwrap();
    match state {
        "running" => {
            repo.mark_running(
                id,
                &MarkRunningOptions {
                    started_at: NOW,
                    conversation_id: Some("conv-1".to_string()),
                    session_id: None,
                    server_ticket_id: None,
                },
            )
            .unwrap();
        }
        "completed" => {
            repo.mark_terminal(
                id,
                &MarkTerminalOptions {
                    status: "completed".to_string(),
                    ended_at: NOW,
                    failure_reason: None,
                    files_changed: None,
                    dispatch_error: None,
                },
            )
            .unwrap();
        }
        "paused" => {
            repo.set_paused(id, true, &InvalidateOptions { now: Some(NOW) })
                .unwrap();
        }
        _ => {}
    }
}

fn build(
    repo: OffPeakTaskRepo,
    client: Arc<FakeClient>,
    events: Arc<Mutex<Vec<Value>>>,
) -> OffPeakTaskService<Env> {
    OffPeakTaskService::new(repo, Env { events }, Arc::new(|| NOW))
        .with_server(client, Arc::new(|| "offpeak-new".to_string()))
}

fn status_entry(id: &str, state: &str, position: Option<i64>) -> OffPeakTicketStatusEntry {
    OffPeakTicketStatusEntry {
        ticket_id: id.to_string(),
        state: state.to_string(),
        position,
        active_deadline: None,
    }
}

#[test]
fn create_success_and_failure_categories() {
    let db = TempDb::new("create");
    let repo = OffPeakTaskRepo::with_clock(&db.0, || NOW);
    let client = Arc::new(FakeClient::default());
    client.push_ticket(FakeClient::ticket("tk-new", "queued", Some(2), Some(5000)));
    let events = Arc::new(Mutex::new(Vec::new()));
    let service = build(repo, Arc::clone(&client), Arc::clone(&events));
    let value = serde_json::to_value(service.create_task(&params()).unwrap()).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["ticketInitialState"], "queued");
    assert_eq!(value["queuePosition"], 2);
    assert_eq!(value["providerName"], "provider");
    let task = service
        .repo()
        .get(value["task"]["offPeakTaskId"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(task.server_ticket_id.as_deref(), Some("tk-new"));
    assert_eq!(task.next_poll_at, Some(NOW + 5000));
    assert_eq!(events.lock().unwrap().len(), 1);

    let cases = [
        (
            OffPeakClientError::Server {
                http_status: 400,
                biz_code: Some(3101),
            },
            "eligibility_3101",
            "3101",
        ),
        (
            OffPeakClientError::Server {
                http_status: 400,
                biz_code: Some(3103),
            },
            "quota_3103",
            "3103",
        ),
        (OffPeakClientError::Network, "network", ""),
        (OffPeakClientError::InvalidResponse, "invalid_response", ""),
    ];
    for (error, category, code) in cases {
        let db = TempDb::new("create-error");
        let client = Arc::new(FakeClient::default());
        client.push_ticket_error(error);
        let service = build(
            OffPeakTaskRepo::with_clock(&db.0, || NOW),
            client,
            Arc::new(Mutex::new(Vec::new())),
        );
        let value = serde_json::to_value(service.create_task(&params()).unwrap()).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["failureStage"], "ticket_request");
        assert_eq!(value["errorCategory"], category);
        assert_eq!(value["errorCode"], code);
    }

    let db = TempDb::new("validation");
    let client = Arc::new(FakeClient::default());
    let service = build(
        OffPeakTaskRepo::with_clock(&db.0, || NOW),
        Arc::clone(&client),
        Arc::new(Mutex::new(Vec::new())),
    );
    let mut invalid = params();
    invalid.title = "  ".to_string();
    let value = serde_json::to_value(service.create_task(&invalid).unwrap()).unwrap();
    assert_eq!(value["failureStage"], "client_validation");
    assert!(client.calls().is_empty());
}

#[test]
fn create_session_bound_rejects_before_ticket() {
    let db = TempDb::new("bound");
    let repo = OffPeakTaskRepo::with_clock(&db.0, || NOW);
    seed(&repo, "t1", "queued", Some("tk-1"), Some("s1"));
    let client = Arc::new(FakeClient::default());
    let service = build(repo, Arc::clone(&client), Arc::new(Mutex::new(Vec::new())));
    let mut request = params();
    request.bound_session_id = Some("s1".to_string());
    let value = serde_json::to_value(service.create_task(&request).unwrap()).unwrap();
    assert_eq!(value["errorCode"], "session_bound");
    assert!(client.calls().is_empty());
}

#[test]
fn continue_alive_expired_and_batch_error() {
    // ready 票活着：只查状态，不重取号。
    let db = TempDb::new("continue-alive");
    let repo = OffPeakTaskRepo::with_clock(&db.0, || NOW);
    seed(&repo, "t1", "paused", Some("tk-1"), None);
    let client = Arc::new(FakeClient::default());
    client.batch(Ok(OffPeakBatchStatusResult {
        next_poll_after_ms: None,
        tickets: vec![status_entry("tk-1", "ready", Some(1))],
    }));
    let service = build(repo, Arc::clone(&client), Arc::new(Mutex::new(Vec::new())));
    let task = service.continue_task("t1").unwrap().unwrap();
    assert_eq!(task["serverTicketId"], "tk-1");
    assert_eq!(task["schedulable"], true);
    assert_eq!(task["queuePosition"], 1);
    assert_eq!(client.take_count(), 0);

    // expired：同 task 重取号。
    let db = TempDb::new("continue-expired");
    let repo = OffPeakTaskRepo::with_clock(&db.0, || NOW);
    seed(&repo, "t1", "paused", Some("tk-1"), None);
    let client = Arc::new(FakeClient::default());
    client.batch(Ok(OffPeakBatchStatusResult {
        next_poll_after_ms: None,
        tickets: vec![status_entry("tk-1", "expired", None)],
    }));
    client.push_ticket(FakeClient::ticket("tk-2", "queued", None, None));
    let service = build(repo, Arc::clone(&client), Arc::new(Mutex::new(Vec::new())));
    let task = service.continue_task("t1").unwrap().unwrap();
    assert_eq!(task["serverTicketId"], "tk-2");
    assert_eq!(client.take_count(), 1);

    // 查询失败：按票活处理，不重取号。
    let db = TempDb::new("continue-error");
    let repo = OffPeakTaskRepo::with_clock(&db.0, || NOW);
    seed(&repo, "t1", "paused", Some("tk-1"), None);
    let client = Arc::new(FakeClient::default());
    client.batch(Err(OffPeakClientError::Network));
    let service = build(repo, Arc::clone(&client), Arc::new(Mutex::new(Vec::new())));
    let task = service.continue_task("t1").unwrap().unwrap();
    assert_eq!(task["serverTicketId"], "tk-1");
    assert_eq!(client.take_count(), 0);
}

#[test]
fn sync_applies_status_backoff_and_settle_outbox() {
    let db = TempDb::new("sync");
    let repo = OffPeakTaskRepo::with_clock(&db.0, || NOW);
    seed(&repo, "t1", "queued", Some("tk-1"), None);
    seed(&repo, "t2", "queued", Some("tk-2"), None);
    let client = Arc::new(FakeClient::default());
    client.batch(Ok(OffPeakBatchStatusResult {
        next_poll_after_ms: Some(1000),
        tickets: vec![
            status_entry("tk-1", "ready", Some(1)),
            status_entry("tk-2", "expired", None),
        ],
    }));
    client.push_ticket(FakeClient::ticket("tk-3", "queued", None, None));
    let events = Arc::new(Mutex::new(Vec::new()));
    let service = build(repo, Arc::clone(&client), Arc::clone(&events));
    assert_eq!(service.run_sync_cycle(), 5000);
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|event| event["kind"] == "wake"));
    let t1 = service.repo().get("t1").unwrap().unwrap();
    assert!(t1.schedulable);
    assert_eq!(t1.next_poll_at, Some(NOW + 5000));
    assert_eq!(
        service
            .repo()
            .get("t2")
            .unwrap()
            .unwrap()
            .server_ticket_id
            .as_deref(),
        Some("tk-3")
    );

    // 轮询失败：首次退避 10s。
    let db = TempDb::new("sync-fail");
    let repo = OffPeakTaskRepo::with_clock(&db.0, || NOW);
    seed(&repo, "t1", "queued", Some("tk-1"), None);
    let client = Arc::new(FakeClient::default());
    client.batch(Err(OffPeakClientError::Network));
    let service = build(repo, client, Arc::new(Mutex::new(Vec::new())));
    assert_eq!(service.run_sync_cycle(), 10_000);

    // 核销：2xx 与 4xx 都标记 settled，网络失败保持未核销。
    let db = TempDb::new("settle");
    let repo = OffPeakTaskRepo::with_clock(&db.0, || NOW);
    seed(&repo, "t1", "completed", Some("tk-1"), None);
    seed(&repo, "t2", "completed", Some("tk-2"), None);
    seed(&repo, "t3", "completed", Some("tk-3"), None);
    let client = Arc::new(FakeClient::default());
    *client.settle.lock().unwrap() = vec![
        ("tk-1".to_string(), Ok(())),
        (
            "tk-2".to_string(),
            Err(OffPeakClientError::Server {
                http_status: 400,
                biz_code: None,
            }),
        ),
        ("tk-3".to_string(), Err(OffPeakClientError::Network)),
    ];
    let service = build(repo, client, Arc::new(Mutex::new(Vec::new())));
    let _ = service.run_sync_cycle();
    assert_eq!(
        service.repo().get("t1").unwrap().unwrap().settled_at,
        Some(NOW)
    );
    assert_eq!(
        service.repo().get("t2").unwrap().unwrap().settled_at,
        Some(NOW)
    );
    assert_eq!(service.repo().get("t3").unwrap().unwrap().settled_at, None);
}

#[test]
fn handle_expired_requeues_and_retakes() {
    let db = TempDb::new("expired");
    let repo = OffPeakTaskRepo::with_clock(&db.0, || NOW);
    seed(&repo, "t1", "running", Some("tk-1"), None);
    let client = Arc::new(FakeClient::default());
    client.push_ticket(FakeClient::ticket("tk-2", "queued", Some(0), None));
    let events = Arc::new(Mutex::new(Vec::new()));
    let service = build(repo, Arc::clone(&client), Arc::clone(&events));
    service.handle_ticket_expired_during_run("t1").unwrap();
    let task = service.repo().get("t1").unwrap().unwrap();
    assert_eq!(task.status, "queued");
    assert_eq!(task.server_ticket_id.as_deref(), Some("tk-2"));
    assert_eq!(client.take_count(), 1);
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|event| event["kind"] == "changed"));
}

#[test]
fn availability_and_coding_plan_support() {
    let db = TempDb::new("availability");
    let client = Arc::new(FakeClient::default());
    *client.availability.lock().unwrap() = Some(OffPeakTakeNumberAvailability {
        can_take_number: true,
        next_take_at: Some(NOW + 10_000),
    });
    let service = build(
        OffPeakTaskRepo::with_clock(&db.0, || NOW),
        client,
        Arc::new(Mutex::new(Vec::new())),
    );
    assert_eq!(
        service.get_take_number_availability().unwrap(),
        serde_json::json!({ "canTakeNumber": true, "nextTakeAt": NOW + 10_000 })
    );
    assert_eq!(
        service.get_coding_plan_support(),
        serde_json::json!({ "supported": true })
    );
}
