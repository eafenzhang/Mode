//! 与 TS `OffPeakTaskService` 远程路径（取号/Continue/轮询/核销/3102）的对拍。

use std::collections::{BTreeMap, VecDeque};
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
    OffPeakTaskEnvironment, OffPeakTaskService,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    now: i64,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    seeds: Vec<Seed>,
    client: ClientSpec,
    ops: Vec<Value>,
    expected: Vec<Value>,
    calls: Vec<Value>,
    events: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Seed {
    id: String,
    title: String,
    run_state: String,
    server_ticket_id: Option<String>,
    bound_session_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ClientSpec {
    #[serde(default)]
    take: Vec<Value>,
    #[serde(default)]
    batch: Vec<Value>,
    #[serde(default)]
    settle: BTreeMap<String, String>,
    availability: Option<Value>,
}

enum TakeScript {
    Ok(OffPeakTakeTicketResult),
    Err(OffPeakClientError),
}

struct TempDb(PathBuf);

impl TempDb {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-off-peak-remote-fixture-{label}-{}-{}.sqlite",
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

struct FakeClient {
    take: Mutex<VecDeque<TakeScript>>,
    batch: Mutex<VecDeque<Result<OffPeakBatchStatusResult, OffPeakClientError>>>,
    settle: BTreeMap<String, String>,
    availability: Option<OffPeakTakeNumberAvailability>,
    calls: Mutex<Vec<Value>>,
}

impl FakeClient {
    fn from_spec(spec: &ClientSpec) -> Self {
        let take = spec
            .take
            .iter()
            .map(|script| {
                if let Some(ok) = script.get("ok") {
                    TakeScript::Ok(serde_json::from_value(ok.clone()).expect("ticket"))
                } else {
                    TakeScript::Err(map_error(script["error"].as_str().unwrap_or("network")))
                }
            })
            .collect();
        let batch = spec
            .batch
            .iter()
            .map(|script| {
                if let Some(ok) = script.get("ok") {
                    Ok(serde_json::from_value(ok.clone()).expect("batch"))
                } else {
                    Err(map_error(script["error"].as_str().unwrap_or("network")))
                }
            })
            .collect();
        let availability = spec.availability.as_ref().and_then(|value| {
            value
                .get("ok")
                .and_then(|ok| serde_json::from_value(ok.clone()).ok())
        });
        Self {
            take: Mutex::new(take),
            batch: Mutex::new(batch),
            settle: spec.settle.clone(),
            availability,
            calls: Mutex::new(Vec::new()),
        }
    }
}

fn map_error(value: &str) -> OffPeakClientError {
    match value {
        "3101" | "3103" => OffPeakClientError::Server {
            http_status: 400,
            biz_code: value.parse().ok(),
        },
        "invalid" => OffPeakClientError::InvalidResponse,
        _ => OffPeakClientError::Network,
    }
}

impl OffPeakServerClient for FakeClient {
    fn get_take_number_availability(
        &self,
    ) -> Result<OffPeakTakeNumberAvailability, OffPeakClientError> {
        self.calls
            .lock()
            .unwrap()
            .push(json!({ "kind": "availability" }));
        self.availability.clone().ok_or(OffPeakClientError::Network)
    }
    fn take_ticket(&self, task_id: &str) -> Result<OffPeakTakeTicketResult, OffPeakClientError> {
        self.calls
            .lock()
            .unwrap()
            .push(json!({ "kind": "take", "taskId": task_id }));
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
        self.calls.lock().unwrap().push(json!({
            "kind": "batch",
            "ticketIds": ticket_ids,
        }));
        self.batch
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(OffPeakClientError::Network))
    }
    fn settle(&self, ticket_id: &str) -> Result<(), OffPeakClientError> {
        self.calls
            .lock()
            .unwrap()
            .push(json!({ "kind": "settle", "ticketId": ticket_id }));
        match self.settle.get(ticket_id).map(String::as_str) {
            Some("400") => Err(OffPeakClientError::Server {
                http_status: 400,
                biz_code: None,
            }),
            Some("500") => Err(OffPeakClientError::Server {
                http_status: 500,
                biz_code: None,
            }),
            Some("network") => Err(OffPeakClientError::Network),
            _ => Ok(()),
        }
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
            .push(json!({ "kind": "changed" }));
    }
    fn request_scheduler_wake(&self) {
        self.events.lock().unwrap().push(json!({ "kind": "wake" }));
    }
    fn coding_plan_support(&self) -> Value {
        json!({ "supported": true })
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

fn seed(repo: &OffPeakTaskRepo, now: i64, seed: &Seed) {
    repo.create(
        &CreateParams {
            title: seed.title.clone(),
            prompt: "p".to_string(),
            permission_mode: "edit".to_string(),
            model_selection: selection(),
            workspace_path: "/ws/demo".to_string(),
            workspace_identity: None,
            bound_session_id: seed.bound_session_id.clone(),
        },
        &CreateOptions {
            now: Some(now),
            off_peak_task_id: Some(seed.id.clone()),
            server_ticket_id: seed.server_ticket_id.clone(),
            registered_at: seed.server_ticket_id.as_ref().map(|_| now),
            ..CreateOptions::default()
        },
    )
    .unwrap();
    match seed.run_state.as_str() {
        "running" => {
            repo.mark_running(
                &seed.id,
                &MarkRunningOptions {
                    started_at: now,
                    conversation_id: Some("conv-1".to_string()),
                    session_id: None,
                    server_ticket_id: None,
                },
            )
            .unwrap();
        }
        "completed" => {
            repo.mark_terminal(
                &seed.id,
                &MarkTerminalOptions {
                    status: "completed".to_string(),
                    ended_at: now,
                    failure_reason: None,
                    files_changed: None,
                    dispatch_error: None,
                },
            )
            .unwrap();
        }
        "paused" => {
            repo.set_paused(&seed.id, true, &InvalidateOptions { now: Some(now) })
                .unwrap();
        }
        _ => {}
    }
}

fn normalize(value: &mut Value) {
    match value {
        Value::String(text) if text.starts_with("offpeak-") => *text = "<task-id>".to_string(),
        Value::Array(items) => {
            for item in items {
                normalize(item);
            }
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                normalize(child);
            }
        }
        _ => {}
    }
}

fn build_params(op: &Value) -> OffPeakTaskCreateParams {
    let overrides = op.get("params").and_then(Value::as_object);
    let get = |key: &str| overrides.and_then(|map| map.get(key));
    OffPeakTaskCreateParams {
        title: get("title")
            .and_then(Value::as_str)
            .unwrap_or("T")
            .to_string(),
        prompt: get("prompt")
            .and_then(Value::as_str)
            .unwrap_or("P")
            .to_string(),
        permission_mode: get("permissionMode")
            .and_then(Value::as_str)
            .unwrap_or("edit")
            .to_string(),
        model_selection: get("modelSelection")
            .cloned()
            .map(|value| serde_json::from_value(value).expect("selection"))
            .unwrap_or_else(selection),
        workspace_path: "/ws/demo".to_string(),
        workspace_identity: None,
        bound_session_id: get("boundSessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

#[test]
fn matches_ts_off_peak_remote_paths() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("off-peak-service-remote.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.cases {
        let db = TempDb::new(&case.name);
        let now = fixture.now;
        let repo = OffPeakTaskRepo::with_clock(&db.0, move || now);
        for seed_item in &case.seeds {
            seed(&repo, now, seed_item);
        }
        let client = Arc::new(FakeClient::from_spec(&case.client));
        let server: Arc<dyn OffPeakServerClient> = client.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let service = OffPeakTaskService::new(
            repo,
            Env {
                events: Arc::clone(&events),
            },
            Arc::new(move || now),
        )
        .with_server(server, Arc::new(|| "offpeak-new".to_string()));

        assert_eq!(case.ops.len(), case.expected.len(), "{}", case.name);
        for (index, (op, expected)) in case.ops.iter().zip(case.expected.iter()).enumerate() {
            let kind = op["kind"].as_str().expect("kind");
            let mut actual = match kind {
                "create" => {
                    let result = service.create_task(&build_params(op)).unwrap();
                    serde_json::to_value(result).expect("序列化 create")
                }
                "continue" => service
                    .continue_task(op["id"].as_str().expect("id"))
                    .unwrap()
                    .unwrap_or(Value::Null),
                "sync" => {
                    let _ = service.run_sync_cycle();
                    Value::Null
                }
                "handleExpired" => {
                    service
                        .handle_ticket_expired_during_run(op["id"].as_str().expect("id"))
                        .unwrap();
                    Value::Null
                }
                "availability" => service.get_take_number_availability().unwrap(),
                "support" => service.get_coding_plan_support(),
                "list" => Value::Array(service.list().unwrap()),
                other => panic!("{}: 未知 op {other}", case.name),
            };
            normalize(&mut actual);
            assert_eq!(actual, *expected, "{} op {index} ({kind})", case.name);
        }

        let mut calls = client.calls.lock().unwrap().clone();
        for call in &mut calls {
            normalize(call);
        }
        assert_eq!(calls, case.calls, "{}: calls", case.name);
        let actual_events = events.lock().unwrap().clone();
        assert_eq!(actual_events, case.events, "{}: events", case.name);
    }
}
