//! 与 TS `OffPeakTaskService` 本地生命周期方法的对拍。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use model_selection::{ModelSelection, SelectionOptions};
use off_peak_repo::{
    CreateOptions, CreateParams, InvalidateOptions, MarkRunningOptions, MarkTerminalOptions,
    OffPeakTask, OffPeakTaskRepo,
};
use off_peak_service::{OffPeakTaskEnvironment, OffPeakTaskService, OffPeakUpdateTaskParams};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct Fixture {
    now: i64,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    seeds: Vec<Seed>,
    ops: Vec<Value>,
    expected: Vec<Value>,
    events: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Seed {
    id: String,
    title: String,
    prompt: String,
    model_selection: Value,
    run_state: String,
    conversation_id: Option<String>,
}

struct TempDb(PathBuf);

impl TempDb {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-off-peak-service-{label}-{}-{}.sqlite",
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

struct Environment {
    settle_repo: OffPeakTaskRepo,
    events: Arc<Mutex<Vec<Value>>>,
    now: i64,
}

impl OffPeakTaskEnvironment for Environment {
    fn resolve_model_selection(&self, selection: &ModelSelection) -> Option<ModelSelection> {
        if selection.model_id.starts_with("bad") {
            return None;
        }
        Some(ModelSelection {
            provider_id: "provider-1".to_string(),
            model_id: selection.model_id.clone(),
            options: selection.options.as_ref().map(|options| SelectionOptions {
                reasoning_level: options.reasoning_level.clone(),
            }),
        })
    }

    fn stop_running_task(
        &self,
        conversation_id: &str,
        workspace_path: &str,
        workspace_identity: Option<&str>,
    ) -> Result<(), String> {
        let mut event = serde_json::Map::new();
        event.insert("kind".to_string(), Value::String("stop".to_string()));
        event.insert(
            "conversationId".to_string(),
            Value::String(conversation_id.to_string()),
        );
        event.insert(
            "workspacePath".to_string(),
            Value::String(workspace_path.to_string()),
        );
        if let Some(identity) = workspace_identity {
            event.insert(
                "workspaceIdentity".to_string(),
                Value::String(identity.to_string()),
            );
        }
        self.events
            .lock()
            .expect("events lock")
            .push(Value::Object(event));
        Ok(())
    }

    fn settle_terminal(&self, task: &OffPeakTask) {
        if task.server_ticket_id.is_none() {
            let _ = self
                .settle_repo
                .mark_settled(&task.off_peak_task_id, self.now);
        }
    }

    fn on_tasks_changed(&self) {
        self.events
            .lock()
            .expect("events lock")
            .push(json!({ "kind": "changed" }));
    }
}

fn seed(repo: &OffPeakTaskRepo, now: i64, seed: &Seed) {
    let selection: ModelSelection =
        serde_json::from_value(seed.model_selection.clone()).expect("modelSelection");
    repo.create(
        &CreateParams {
            title: seed.title.clone(),
            prompt: seed.prompt.clone(),
            permission_mode: "edit".to_string(),
            model_selection: selection,
            workspace_path: "/ws/demo".to_string(),
            workspace_identity: None,
            bound_session_id: None,
        },
        &CreateOptions {
            now: Some(now),
            off_peak_task_id: Some(seed.id.clone()),
            ..CreateOptions::default()
        },
    )
    .expect("create seed");
    match seed.run_state.as_str() {
        "running" => {
            repo.mark_running(
                &seed.id,
                &MarkRunningOptions {
                    started_at: now,
                    conversation_id: seed
                        .conversation_id
                        .clone()
                        .or_else(|| Some("conv".to_string())),
                    session_id: None,
                    server_ticket_id: None,
                },
            )
            .expect("mark running");
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
            .expect("mark terminal");
        }
        "paused" => {
            repo.set_paused(&seed.id, true, &InvalidateOptions { now: Some(now) })
                .expect("pause");
        }
        _ => {}
    }
}

#[test]
fn matches_ts_off_peak_local_lifecycle() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("off-peak-service.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.cases {
        let db = TempDb::new(&case.name);
        let repo = OffPeakTaskRepo::with_clock(&db.0, move || fixture.now);
        let settle_repo = OffPeakTaskRepo::with_clock(&db.0, move || fixture.now);
        for seed_item in &case.seeds {
            seed(&repo, fixture.now, seed_item);
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let service = OffPeakTaskService::new(
            repo,
            Environment {
                settle_repo,
                events: Arc::clone(&events),
                now: fixture.now,
            },
            Arc::new(move || fixture.now),
        );

        assert_eq!(case.ops.len(), case.expected.len(), "{}", case.name);
        for (index, (op, expected)) in case.ops.iter().zip(case.expected.iter()).enumerate() {
            let kind = op["kind"].as_str().expect("kind");
            let id = op.get("id").and_then(Value::as_str);
            let actual = match kind {
                "list" => match service.list() {
                    Ok(tasks) => Value::Array(tasks),
                    Err(error) => json!({ "error": error }),
                },
                "get" => match service.get(id.expect("id")) {
                    Ok(task) => task.unwrap_or(Value::Null),
                    Err(error) => json!({ "error": error }),
                },
                "pause" => match service.pause_task(id.expect("id")) {
                    Ok(task) => task.unwrap_or(Value::Null),
                    Err(error) => json!({ "error": error }),
                },
                "deleteHistory" => match service.delete_history(id.expect("id")) {
                    Ok(task) => task.unwrap_or(Value::Null),
                    Err(error) => json!({ "error": error }),
                },
                "cancel" => match service.cancel_task(id.expect("id")) {
                    Ok(task) => task.unwrap_or(Value::Null),
                    Err(error) => json!({ "error": error }),
                },
                "delete" => match service.delete_task(id.expect("id")) {
                    Ok(()) => Value::Null,
                    Err(error) => json!({ "error": error }),
                },
                "update" => {
                    let params: OffPeakUpdateTaskParams =
                        serde_json::from_value(op["params"].clone()).expect("update params");
                    match service.update_task(id.expect("id"), &params) {
                        Ok(task) => task.unwrap_or(Value::Null),
                        Err(error) => json!({ "error": error }),
                    }
                }
                "validate" => {
                    let selection: ModelSelection =
                        serde_json::from_value(op["selection"].clone()).expect("selection");
                    Value::Bool(service.validate_dispatch_model_selection(&selection))
                }
                other => panic!("{}: 未知 op {other}", case.name),
            };
            assert_eq!(actual, *expected, "{} op {index} ({kind})", case.name);
        }

        let actual_events = events.lock().expect("events lock").clone();
        assert_eq!(actual_events, case.events, "{}: events", case.name);
    }
}
