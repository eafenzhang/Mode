//! C4 T4a：`v4/command startSavedWorkflow`（嵌入 JS 引擎）与命令对账读面。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_core::ModelProvider;
use agent_protocol::workflow::{SavedWorkflowScope, WorkflowRunStatus, WorkflowsGetResult};
use agent_server::workflows::{self, WorkflowDirs};
use agent_server::{
    AgentServer, ProviderConfigHub, ProviderRequestContext, ProviderSource, ServerConfig,
};
use rusqlite::OpenFlags;
use serde_json::{json, Value};
use session_store::SessionStore;
use tokio::runtime::Runtime;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-agent-server-v4-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 本切片不发起模型请求：装配源显式失败即可。
struct UnusedSource;

impl ProviderSource for UnusedSource {
    fn resolve(
        &self,
        provider_id: &str,
        _model_id: &str,
        _context: Option<&ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        Err(format!("Provider 不存在: {provider_id}"))
    }

    fn cache_key(&self) -> String {
        "unused".to_string()
    }
}

fn open_server(root: &TempRoot) -> Arc<AgentServer> {
    let config = ServerConfig::from_data_base_dir(root.path());
    let server = Arc::new(AgentServer::with_parts(
        config,
        ProviderConfigHub::from_env(),
        Arc::new(UnusedSource),
    ));
    let runtime = Runtime::new().expect("tokio runtime");
    let mut sink: Vec<u8> = Vec::new();
    runtime
        .block_on(async { agent_server::run_stdio(&b""[..], &mut sink, Arc::clone(&server)).await })
        .expect("存储门必须完成");
    server
}

const SAVED: &str = "/* mode-workflow\ndescription: probe run\n*/\nreturn { ok: true, n: 2 };";

fn seed_workflow(root: &TempRoot, name: &str) -> PathBuf {
    let dirs = WorkflowDirs::new(
        root.path().join("ws").to_str().expect("ws"),
        &root.path().join("data"),
    );
    std::fs::create_dir_all(&dirs.project).expect("workflows dir");
    let path = dirs.project.join(format!("{name}.dwf.ts"));
    std::fs::write(&path, SAVED).expect("write workflow");
    path
}

// ── execute_script ──────────────────────────────────────────────────────────

#[test]
fn execute_script_returns_json_and_reports_errors() {
    match workflows::execute_script("return { ok: true, n: 2 };") {
        workflows::ScriptOutcome::Completed(json) => {
            let value: Value = serde_json::from_str(&json).expect("json");
            assert_eq!(value, json!({ "ok": true, "n": 2 }));
        }
        other => panic!("expected completed, got {other:?}"),
    }
    match workflows::execute_script("throw new Error('boom');") {
        workflows::ScriptOutcome::Errored(message) => {
            assert!(message.contains("boom"), "message: {message}");
        }
        other => panic!("expected errored, got {other:?}"),
    }
}

// ── start_saved_workflow：函数级（store + dirs）──────────────────────────────

#[test]
fn start_saved_workflow_persists_run_rows_and_events() {
    let root = TempRoot::new("start-fn");
    seed_workflow(&root, "probe-run");
    let data_root = root.path().join("data");
    let db_path = data_root.join("cli").join("db").join("db.sqlite");
    let store = SessionStore::open(&db_path).expect("open store");
    let dirs = WorkflowDirs::new(root.path().join("ws").to_str().expect("ws"), &data_root);

    let started = workflows::start_saved_workflow(
        &store,
        &dirs,
        &root.path().join("ws").to_string_lossy(),
        "sess_parent",
        "probe-run",
        Some(SavedWorkflowScope::Project),
    )
    .expect("start");
    assert!(started.run_id.starts_with("dwfrun-"));
    assert!(!started.tool_call_id.is_empty());

    // run 行已结算。
    let connection =
        rusqlite::Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("read db");
    let runs = workflows::runs(
        &data_root,
        &root.path().join("ws").to_string_lossy(),
        None,
        10,
    );
    assert_eq!(runs.runs.len(), 1);
    assert_eq!(runs.runs[0].status, WorkflowRunStatus::Completed);
    let result_json: String = connection
        .query_row(
            "select result_json from dwf_run where id = ?1",
            [started.run_id.clone()],
            |row| row.get(0),
        )
        .expect("result_json");
    let result: Value = serde_json::from_str(&result_json).expect("json");
    assert_eq!(result, json!({ "ok": true, "n": 2 }));

    // 事件序列 run-started → run-launched → run-settled。
    let mut statement = connection
        .prepare("select sequence, type, payload_json from dwf_event where run_id = ?1 order by sequence")
        .expect("prepare events");
    let events: Vec<(i64, String, String)> = statement
        .query_map([started.run_id.clone()], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("query events")
        .collect::<Result<_, _>>()
        .expect("rows");
    let sequence: Vec<i64> = events.iter().map(|(sequence, _, _)| *sequence).collect();
    assert_eq!(sequence, vec![0, 1, 2]);
    assert_eq!(events[0].1, "run-started");
    assert_eq!(events[1].1, "run-launched");
    assert_eq!(events[2].1, "run-settled");
    let settled: Value = serde_json::from_str(&events[2].2).expect("payload");
    assert_eq!(
        settled,
        json!({ "type": "run-settled", "status": "completed" })
    );
}

#[test]
fn start_saved_workflow_reports_script_failure() {
    let root = TempRoot::new("start-error");
    let data_root = root.path().join("data");
    let dirs = WorkflowDirs::new(root.path().join("ws").to_str().expect("ws"), &data_root);
    std::fs::create_dir_all(&dirs.project).expect("workflows dir");
    std::fs::write(
        dirs.project.join("boom.dwf.ts"),
        "/* mode-workflow\ndescription: boom\n*/\nthrow new Error('workflow boom');",
    )
    .expect("write workflow");
    let db_path = data_root.join("cli").join("db").join("db.sqlite");
    let store = SessionStore::open(&db_path).expect("open store");

    let started = workflows::start_saved_workflow(
        &store,
        &dirs,
        &root.path().join("ws").to_string_lossy(),
        "sess_parent",
        "boom",
        None,
    )
    .expect("start");
    let runs = workflows::runs(
        &data_root,
        &root.path().join("ws").to_string_lossy(),
        None,
        10,
    );
    assert_eq!(runs.runs.len(), 1);
    assert_eq!(runs.runs[0].status, WorkflowRunStatus::Errored);
    let _ = started;
}

// ── v4/command：ACK 与对账 ──────────────────────────────────────────────────

fn envelope(command_id: &str, session_id: &str, command_type: &str, payload: Value) -> Value {
    json!({
        "commandId": command_id,
        "clientId": "probe-c4",
        "sessionId": session_id,
        "type": command_type,
        "payload": payload,
        "issuedAt": 1_700_000_000_000i64,
    })
}

#[test]
fn start_saved_workflow_rejects_missing_with_failed_ack() {
    let root = TempRoot::new("dispatch-missing");
    let server = open_server(&root);
    let ack = server
        .dispatch(
            "v4/command",
            envelope(
                "cmd-c4-wf-missing",
                "sess_v4",
                "startSavedWorkflow",
                json!({ "name": "missing-wf", "scope": "project" }),
            ),
        )
        .expect("ack");
    assert_eq!(ack["commandId"], json!("cmd-c4-wf-missing"));
    assert_eq!(ack["status"], json!("failed"));
    assert_eq!(
        ack["reasonCode"],
        json!("fault.command.savedWorkflowStartRejected.not_found")
    );
    assert_eq!(
        ack["message"],
        json!("No project workflow named 'missing-wf'.")
    );
    assert_eq!(ack["memoryEnabled"], json!(false));
    assert!(ack["revisionAtDecision"].is_number());
    assert_eq!(ack["result"], Value::Null);

    // 对账读面：同 key 返回 ACK；未知 key 返回 "unknown"。
    let query = server
        .dispatch(
            "v4/commands/query",
            json!({
                "commands": [
                    { "sessionId": "sess_v4", "commandId": "cmd-c4-wf-missing" },
                    { "sessionId": "sess_v4", "commandId": "cmd-unknown" }
                ]
            }),
        )
        .expect("query");
    let results = query["results"].as_array().expect("results");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["result"]["status"], json!("failed"));
    assert_eq!(results[1]["result"], json!("unknown"));
}

#[test]
fn start_saved_workflow_accepts_and_executes_via_dispatch() {
    let root = TempRoot::new("dispatch-ok");
    seed_workflow(&root, "probe-run");
    let server = open_server(&root);
    // 无 runtime 时 workspace 回退 dataRoot（`<root>/.mode`）；按该口径种 workflow。
    let fallback_workspace = root.path().join(".mode");
    let dirs = WorkflowDirs::new(
        fallback_workspace.to_str().expect("ws"),
        &root.path().join(".mode"),
    );
    std::fs::create_dir_all(&dirs.project).expect("workflows dir");
    std::fs::write(dirs.project.join("probe-run.dwf.ts"), SAVED).expect("write workflow");

    let ack = server
        .dispatch(
            "v4/command",
            envelope(
                "cmd-c4-wf-ok",
                "sess_v4",
                "startSavedWorkflow",
                json!({ "name": "probe-run", "scope": "project" }),
            ),
        )
        .expect("ack");
    assert_eq!(ack["status"], json!("accepted"));
    assert_eq!(ack["result"]["type"], json!("startSavedWorkflow"));
    let run_id = ack["result"]["runId"].as_str().expect("runId").to_string();
    assert!(run_id.starts_with("dwfrun-"));

    // run 行落在 dataRoot 库里，且已结算 completed。
    let db_path = root.path().join(".mode/cli/db/db.sqlite");
    let store = SessionStore::open(&db_path).expect("open store");
    drop(store);
    let runs = workflows::runs(
        &root.path().join(".mode"),
        &fallback_workspace.to_string_lossy(),
        None,
        10,
    );
    assert_eq!(runs.runs.len(), 1);
    assert_eq!(runs.runs[0].run_id, run_id);
    assert_eq!(runs.runs[0].status, WorkflowRunStatus::Completed);
    assert_eq!(runs.runs[0].name.as_deref(), Some("probe-run"));
}

#[test]
fn workflow_run_events_query_starts_at_sequence_zero() {
    let root = TempRoot::new("dispatch-events");
    let fallback_workspace = root.path().join(".mode");
    let dirs = WorkflowDirs::new(
        fallback_workspace.to_str().expect("ws"),
        &root.path().join(".mode"),
    );
    std::fs::create_dir_all(&dirs.project).expect("workflows dir");
    std::fs::write(dirs.project.join("probe-run.dwf.ts"), SAVED).expect("write workflow");
    let server = open_server(&root);

    let ack = server
        .dispatch(
            "v4/command",
            envelope(
                "cmd-c4-wf-events",
                "sess_v4",
                "startSavedWorkflow",
                json!({ "name": "probe-run", "scope": "project" }),
            ),
        )
        .expect("ack");
    let run_id = ack["result"]["runId"].as_str().expect("runId");

    // 缺省 afterSequence 必须包含 sequence 0 的 run-started（bug 回归）。
    let all = server
        .dispatch(
            "v4/conversation/workflowRunEvents",
            json!({ "sessionId": "sess_v4", "runId": run_id }),
        )
        .expect("events");
    let events = all["events"].as_array().expect("events");
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["sequence"], json!(0));
    assert_eq!(events[0]["type"], json!("run-started"));
    assert_eq!(events[2]["type"], json!("run-settled"));
    assert_eq!(all["hasMore"], json!(false));

    // 排他游标：afterSequence=1 只返回 2、3。
    let after = server
        .dispatch(
            "v4/conversation/workflowRunEvents",
            json!({ "sessionId": "sess_v4", "runId": run_id, "afterSequence": 1 }),
        )
        .expect("events after");
    let sequences: Vec<i64> = after["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| event["sequence"].as_i64().expect("sequence"))
        .collect();
    assert_eq!(sequences, vec![2]);
}

#[test]
fn unsupported_command_type_fails_per_command() {
    let root = TempRoot::new("dispatch-unsupported");
    let server = open_server(&root);
    let ack = server
        .dispatch(
            "v4/command",
            envelope(
                "cmd-c4-unsupported",
                "sess_v4",
                "resolveInteraction",
                json!({ "interactionId": "interaction-x", "answer": {} }),
            ),
        )
        .expect("ack");
    assert_eq!(ack["status"], json!("failed"));
    assert_eq!(ack["reasonCode"], json!("fault.command.unsupported"));
}

// `get` 的 scope 语义回归：project 缺失时按 global 找。
#[test]
fn workflow_get_falls_back_to_global() {
    let root = TempRoot::new("get-global");
    let dirs = WorkflowDirs::new(
        root.path().join("ws").to_str().expect("ws"),
        &root.path().join("data"),
    );
    std::fs::create_dir_all(&dirs.global).expect("global dir");
    std::fs::write(dirs.global.join("probe-run.dwf.ts"), SAVED).expect("write workflow");
    match workflows::get(&dirs, "probe-run", None) {
        WorkflowsGetResult::Ok(ok) => assert_eq!(ok.scope, SavedWorkflowScope::Global),
        other => panic!("expected global hit, got {other:?}"),
    }
}

#[test]
fn create_session_command_returns_schema_compatible_ack() {
    let root = TempRoot::new("create-session");
    let server = open_server(&root);
    let workspace = root.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let workspace_id = workspace.to_str().expect("workspace path");
    let runtime = Runtime::new().expect("tokio runtime");
    let ack = runtime
        .block_on(async {
            server.dispatch(
            "v4/command",
            json!({
                "commandId": "cmd-create-1",
                "clientId": "probe-c4",
                "sessionId": null,
                "type": "createSession",
                "payload": {
                    "workspaceId": workspace_id,
                    "config": { "mode": "build", "provider": "fixture", "model": "fixture-model" },
                    "firstInput": {
                        "text": "first prompt",
                        "mode": "yolo",
                        "modelSelection": {
                            "providerId": "fixture",
                            "modelId": "fixture-model",
                            "options": { "reasoningLevel": "enabled" }
                        }
                    }
                },
                "issuedAt": 1_700_000_000_000i64,
            }),
        )
        })
        .expect("create ack");
    assert_eq!(ack["status"], json!("accepted"), "{ack}");
    assert_eq!(ack["result"]["type"], json!("createSession"));
    let session_id = ack["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    // 会话行真的落库（read 面可见）；同一 commandId 的 ACK 可对账。
    let read = runtime
        .block_on(async { server.dispatch("session/read", json!({ "sessionId": session_id })) })
        .expect("read");
    assert_eq!(read["session"]["sessionId"], json!(session_id));
    // firstInput 的选择必须落到 runtime（否则首发 turn 在模型请求前就失败）。
    assert_eq!(
        read["settings"]["model"]["current"],
        json!({
            "providerId": "fixture",
            "modelId": "fixture-model",
            "options": { "reasoningLevel": "enabled" }
        })
    );
    assert_eq!(read["settings"]["mode"]["current"], json!("yolo"));
    let query = runtime
        .block_on(async {
            server.dispatch(
                "v4/commands/query",
                json!({ "commands": [{ "sessionId": null, "commandId": "cmd-create-1" }] }),
            )
        })
        .expect("query");
    assert_eq!(query["results"][0]["result"]["status"], json!("accepted"));
}

#[test]
fn rename_and_delete_session_commands_round_trip() {
    let root = TempRoot::new("rename-delete");
    let server = open_server(&root);
    let workspace = root.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let workspace_id = workspace.to_str().expect("workspace path");
    let runtime = Runtime::new().expect("tokio runtime");
    let ack = runtime
        .block_on(async {
            server.dispatch(
                "v4/command",
                json!({
                    "commandId": "cmd-create-rn",
                    "clientId": "probe-c4",
                    "sessionId": null,
                    "type": "createSession",
                    "payload": { "workspaceId": workspace_id },
                    "issuedAt": 1_700_000_000_000i64,
                }),
            )
        })
        .expect("create");
    let session_id = ack["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    let rename = runtime
        .block_on(async {
            server.dispatch(
                "v4/command",
                json!({
                    "commandId": "cmd-rename-1",
                    "clientId": "probe-c4",
                    "sessionId": session_id,
                    "type": "renameSession",
                    "payload": { "title": "renamed by test" },
                    "issuedAt": 1_700_000_000_001i64,
                }),
            )
        })
        .expect("rename");
    assert_eq!(rename["status"], json!("accepted"), "{rename}");
    let read = runtime
        .block_on(async { server.dispatch("session/read", json!({ "sessionId": session_id })) })
        .expect("read");
    assert_eq!(read["session"]["title"], json!("renamed by test"));
    assert_eq!(read["session"]["titleSource"], json!("custom"));

    let delete = runtime
        .block_on(async {
            server.dispatch(
                "v4/command",
                json!({
                    "commandId": "cmd-delete-1",
                    "clientId": "probe-c4",
                    "sessionId": session_id,
                    "type": "deleteSession",
                    "payload": {},
                    "issuedAt": 1_700_000_000_002i64,
                }),
            )
        })
        .expect("delete");
    assert_eq!(delete["status"], json!("accepted"), "{delete}");
    let list = runtime
        .block_on(async {
            server.dispatch(
                "session/list",
                json!({ "includeArchived": false, "sessionIds": [session_id] }),
            )
        })
        .expect("list");
    assert_eq!(list["sessions"], json!([]));
}

#[test]
fn queue_commands_validate_cas_and_targets() {
    let root = TempRoot::new("queue-cmds");
    let server = open_server(&root);
    let workspace = root.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let workspace_id = workspace.to_str().expect("workspace path");
    let runtime = Runtime::new().expect("tokio runtime");
    let ack = runtime
        .block_on(async {
            server.dispatch(
                "v4/command",
                json!({
                    "commandId": "cmd-queue-create",
                    "clientId": "probe-c4",
                    "sessionId": null,
                    "type": "createSession",
                    "payload": { "workspaceId": workspace_id },
                    "issuedAt": 1_700_000_000_000i64,
                }),
            )
        })
        .expect("create");
    let session_id = ack["result"]["sessionId"]
        .as_str()
        .expect("sessionId")
        .to_string();

    // 缺 queueItemId → invalidPayload。
    let missing = runtime
        .block_on(async {
            server.dispatch(
                "v4/command",
                json!({
                    "commandId": "cmd-queue-missing",
                    "clientId": "probe-c4",
                    "sessionId": session_id,
                    "type": "deleteQueueItem",
                    "baseRevision": 0,
                    "payload": {},
                    "issuedAt": 1_700_000_000_001i64,
                }),
            )
        })
        .expect("ack");
    assert_eq!(missing["status"], json!("failed"));
    assert_eq!(missing["reasonCode"], json!("fault.command.invalidPayload"));

    // baseRevision 过期 → stale（UI 用 revisionAtDecision 重试）。
    let stale = runtime
        .block_on(async {
            server.dispatch(
                "v4/command",
                json!({
                    "commandId": "cmd-queue-stale",
                    "clientId": "probe-c4",
                    "sessionId": session_id,
                    "type": "editQueueItem",
                    "baseRevision": 999,
                    "payload": { "queueItemId": "in-x", "newText": "x" },
                    "issuedAt": 1_700_000_000_002i64,
                }),
            )
        })
        .expect("ack");
    assert_eq!(stale["status"], json!("stale"), "{stale}");
    assert!(stale["revisionAtDecision"].as_u64().is_some());

    // CAS 命中但目标不存在 → failed（文案带「队列项不存在」）。
    let missing_item = runtime
        .block_on(async {
            server.dispatch(
                "v4/command",
                json!({
                    "commandId": "cmd-queue-notfound",
                    "clientId": "probe-c4",
                    "sessionId": session_id,
                    "type": "deleteQueueItem",
                    "baseRevision": 0,
                    "payload": { "queueItemId": "in-x" },
                    "issuedAt": 1_700_000_000_003i64,
                }),
            )
        })
        .expect("ack");
    assert_eq!(missing_item["status"], json!("failed"), "{missing_item}");
    assert!(
        missing_item["message"]
            .as_str()
            .unwrap_or("")
            .contains("队列项不存在"),
        "{missing_item}"
    );

    // setFollowupMode / setAutoDrain：CAS 命中 → accepted。
    let followup = runtime
        .block_on(async {
            server.dispatch(
                "v4/command",
                json!({
                    "commandId": "cmd-followup",
                    "clientId": "probe-c4",
                    "sessionId": session_id,
                    "type": "setFollowupMode",
                    "baseRevision": 0,
                    "payload": { "mode": "guide" },
                    "issuedAt": 1_700_000_000_004i64,
                }),
            )
        })
        .expect("ack");
    assert_eq!(followup["status"], json!("accepted"), "{followup}");
    let auto_drain = runtime
        .block_on(async {
            server.dispatch(
                "v4/command",
                json!({
                    "commandId": "cmd-autodrain",
                    "clientId": "probe-c4",
                    "sessionId": session_id,
                    "type": "setAutoDrain",
                    "baseRevision": 1,
                    "payload": { "autoDrain": false },
                    "issuedAt": 1_700_000_000_005i64,
                }),
            )
        })
        .expect("ack");
    assert_eq!(auto_drain["status"], json!("accepted"), "{auto_drain}");
}
