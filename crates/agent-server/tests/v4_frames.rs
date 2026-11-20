//! C4 T4c：v4 帧订阅、sessions-index、行投影 query 与 workflow run 查询族。
//!
//! 通过 stdio 全链路驱动：输入帧 → 响应 + `v4/conversation/frame` 通知，
//! 断言快照/增量的形状与 query 读面。

use agent_protocol::workflow::{SavedWorkflowScope, WorkflowsGetResult};
use agent_server::workflows::{self, WorkflowDirs};
use agent_server::{
    run_stdio, AgentServer, ProviderConfigHub, ProviderRequestContext, ProviderSource, ServerConfig,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::runtime::Runtime;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-agent-server-v4f-{label}-{}-{}",
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

struct UnusedSource;

impl ProviderSource for UnusedSource {
    fn resolve(
        &self,
        provider_id: &str,
        _model_id: &str,
        _context: Option<&ProviderRequestContext>,
    ) -> Result<Arc<dyn agent_core::ModelProvider>, String> {
        Err(format!("Provider 不存在: {provider_id}"))
    }

    fn cache_key(&self) -> String {
        "unused".to_string()
    }
}

fn request_line(id: u64, method: &str, params: serde_json::Value) -> String {
    format!(
        "{}\n",
        serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        })
    )
}

/// 跑一段 stdio 输入；模拟 host 应答反向请求，返回（按 id 的响应, 通知帧列表）。
fn drive(root: &TempRoot, input: String) -> (std::collections::BTreeMap<u64, Value>, Vec<Value>) {
    drive_batches(root, vec![input])
}

/// 落库探针：批次写完后反复发同一个只读请求，直到响应里的 `rows` 达到期望
/// 条数，再写入后续批次。`sendText` 的 ACK 只表示命令被接纳，与 turn 落库之间
/// 没有协议级屏障，固定 sleep 在慢机器上会读到部分状态。
struct RowsReady {
    probe: String,
    probe_id: u64,
    expected_rows: usize,
    followup: String,
}

/// 分批写输入（批次之间留时间让 worker 落库），再等 live 帧刷完。
fn drive_batches(
    root: &TempRoot,
    batches: Vec<String>,
) -> (std::collections::BTreeMap<u64, Value>, Vec<Value>) {
    drive_batches_with(root, batches, None)
}

fn drive_batches_with(
    root: &TempRoot,
    batches: Vec<String>,
    ready: Option<RowsReady>,
) -> (std::collections::BTreeMap<u64, Value>, Vec<Value>) {
    let config = ServerConfig::from_data_base_dir(root.path());
    let server = Arc::new(AgentServer::with_parts(
        config,
        ProviderConfigHub::from_env(),
        Arc::new(UnusedSource),
    ));
    let runtime = Runtime::new().expect("tokio runtime");
    let (client, server_io) = tokio::io::duplex(1 << 20);
    let (mut client_read, client_write) = tokio::io::split(client);
    let (read_half, write_half) = tokio::io::split(server_io);
    let server_task =
        runtime.spawn(async move { run_stdio(read_half, write_half, Arc::clone(&server)).await });
    let write = std::sync::Arc::new(tokio::sync::Mutex::new(client_write));
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let ready_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let probe_expect = ready
        .as_ref()
        .map(|ready| (ready.probe_id, ready.expected_rows));
    let writer_write = Arc::clone(&write);
    let writer_flag = Arc::clone(&ready_flag);
    let writer = runtime.spawn(async move {
        use tokio::io::AsyncWriteExt;
        for batch in &batches {
            let mut guard = writer_write.lock().await;
            guard.write_all(batch.as_bytes()).await.ok();
            drop(guard);
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
        if let Some(ready) = ready {
            // 最多探 5s；探针超时也继续写 followup，让断言暴露真实状态。
            for _ in 0..50 {
                let mut guard = writer_write.lock().await;
                guard.write_all(ready.probe.as_bytes()).await.ok();
                drop(guard);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if writer_flag.load(Ordering::SeqCst) {
                    break;
                }
            }
            let mut guard = writer_write.lock().await;
            guard.write_all(ready.followup.as_bytes()).await.ok();
            drop(guard);
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        }
        let _ = shutdown_tx.send(());
    });
    let reader_write = Arc::clone(&write);
    let reader_flag = Arc::clone(&ready_flag);
    let (responses, notifications) = runtime.block_on(async move {
        let mut responses = std::collections::BTreeMap::new();
        let mut notifications = Vec::new();
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let mut reader = tokio::io::BufReader::new(&mut client_read);
        let mut line = String::new();
        loop {
            line.clear();
            tokio::select! {
                _ = &mut shutdown_rx => break,
                read = tokio::time::timeout(
                    std::time::Duration::from_millis(1500),
                    reader.read_line(&mut line),
                ) => {
                    match read {
                        Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                        Ok(Ok(_)) => {}
                    }
                }
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let raw: Value = match serde_json::from_str(trimmed) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            // 反向请求（id 是 server-N 字符串）：按 host 语义回一个默认偏好。
            if raw.get("id").and_then(Value::as_str).is_some() {
                let reply = serde_json::json!({
                    "id": raw["id"].clone(),
                    "result": { "memoryEnabled": false },
                });
                let text = format!(
                    "{}{}",
                    reply,
                    "
"
                );
                let mut guard = reader_write.lock().await;
                let _ = guard.write_all(text.as_bytes()).await;
                continue;
            }
            if raw.get("method").is_some() && raw.get("id").is_none() {
                notifications.push(raw);
                continue;
            }
            let id = raw.get("id").and_then(Value::as_i64).unwrap_or_default();
            if let Some((probe_id, expected_rows)) = probe_expect {
                if id as u64 == probe_id
                    && raw
                        .get("result")
                        .and_then(|result| result.get("rows"))
                        .and_then(Value::as_array)
                        .is_some_and(|rows| rows.len() >= expected_rows)
                {
                    reader_flag.store(true, Ordering::SeqCst);
                }
            }
            if raw.get("error").is_some() {
                responses.insert(id as u64, json!({ "error": raw["error"].clone() }));
            } else {
                responses.insert(id as u64, raw.get("result").cloned().unwrap_or(Value::Null));
            }
        }
        (responses, notifications)
    });
    let _ = runtime.block_on(writer);
    drop(write);
    let server_result = runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), server_task).await
    });
    let _ = server_result;
    (responses, notifications)
}

fn v4_frames(notifications: &[Value]) -> Vec<&Value> {
    notifications
        .iter()
        .filter(|frame| {
            frame.get("method").and_then(Value::as_str) == Some("v4/conversation/frame")
        })
        .collect()
}

const WORKSPACE: &str = "C:/mode-v4f-ws";

#[test]
fn conversation_subscribe_emits_snapshot_then_deltas() {
    let root = TempRoot::new("subscribe");
    let create = request_line(
        1,
        "session/create",
        serde_json::json!({
            "sessionId": "sess_v4f",
            "workspace": { "workspacePath": WORKSPACE, "workspaceKey": WORKSPACE },
        }),
    );
    let subscribe = request_line(
        2,
        "v4/conversation/subscribe",
        serde_json::json!({
            "topic": "conversation/sess_v4f",
            "connectionId": "conn-v4f",
            "clientMode": "desktop-continuous",
        }),
    );
    let send = request_line(
        3,
        "v4/command",
        serde_json::json!({
            "commandId": "cmd-v4f-1",
            "clientId": "probe-c4",
            "sessionId": "sess_v4f",
            "type": "sendText",
            "payload": { "text": "turn one for the frame probe" },
            "issuedAt": 1,
        }),
    );
    let (responses, notifications) = drive_batches(&root, vec![create, subscribe, send]);

    // 订阅 ACK：snapshot 模式 + logEpoch + 行数 0。
    let ack = &responses[&2]["ack"];
    assert_eq!(ack["mode"], json!("snapshot"));
    assert!(ack["logEpoch"].as_str().expect("epoch").contains('-'));
    assert_eq!(ack["openTiming"]["snapshotRowCount"], json!(0));

    // 初始快照帧：21 键快照 + 空行窗口。
    let frames = v4_frames(&notifications);
    assert!(frames.len() >= 2, "至少要有快照帧 + 增量帧");
    let initial = &frames[0]["params"];
    assert_eq!(initial["kind"], json!("complete"));
    assert_eq!(initial["deliveryKind"], json!("initial"));
    assert_eq!(initial["wireVersion"], json!(3));
    let payload = &initial["frame"]["payload"];
    assert_eq!(payload["kind"], json!("snapshot"));
    let snapshot = &payload["snapshot"];
    for key in [
        "availability",
        "backgroundWorks",
        "config",
        "control",
        "goal",
        "inputRouting",
        "logEpoch",
        "meta",
        "modelTransition",
        "pendingCommands",
        "pendingInteractions",
        "plan",
        "protocolVersion",
        "queue",
        "revision",
        "rows",
        "seq",
        "sessionId",
        "subagents",
        "usage",
        "workspaceHookAdmission",
    ] {
        assert!(snapshot.get(key).is_some(), "snapshot 缺 {key}");
    }
    assert_eq!(snapshot["protocolVersion"], json!(1));
    assert_eq!(snapshot["sessionId"], json!("sess_v4f"));
    // 快照帧在任务里构建，可能与随后 sendText 竞态（running 也算合法）。
    assert!(
        snapshot["control"]["phase"] == json!("draft")
            || snapshot["control"]["phase"] == json!("running")
    );
    assert_eq!(snapshot["meta"]["titleSource"], json!("default"));
    assert!(snapshot["rows"]["window"].is_array());

    // sendText 的 ACK accepted。
    assert_eq!(responses[&3]["status"], json!("accepted"));

    // 增量帧：turnHeader/userInput 行 + state.updated（turn.failed 收口）。
    let delta_frames: Vec<&Value> = frames[1..]
        .iter()
        .copied()
        .filter(|frame| frame["params"]["frame"]["payload"]["kind"] == json!("deltas"))
        .collect();
    assert!(!delta_frames.is_empty(), "sendText 后必有增量帧");
    let mut row_kinds = Vec::new();
    let mut saw_state_patch = false;
    for frame in &delta_frames {
        for delta in frame["params"]["frame"]["payload"]["deltas"]
            .as_array()
            .expect("deltas")
        {
            match delta["op"].as_str().expect("op") {
                "row.appended" => {
                    row_kinds.push(delta["row"]["kind"].as_str().expect("row kind").to_string());
                }
                "state.updated" => saw_state_patch = true,
                _ => {}
            }
        }
    }
    assert!(row_kinds.contains(&"turnHeader".to_string()));
    assert!(row_kinds.contains(&"userInput".to_string()));
    assert!(saw_state_patch, "turn 终态必有 state.updated");
}

#[test]
fn rows_range_pages_the_projection() {
    let root = TempRoot::new("rows");
    let create = request_line(
        1,
        "session/create",
        serde_json::json!({
            "sessionId": "sess_rows",
            "workspace": { "workspacePath": WORKSPACE, "workspaceKey": WORKSPACE },
        }),
    );
    let mut sends = Vec::new();
    for (index, text) in ["turn one", "turn two", "turn three"].iter().enumerate() {
        sends.push(request_line(
            10 + index as u64,
            "v4/command",
            serde_json::json!({
                "commandId": format!("cmd-rows-{index}"),
                "clientId": "probe-c4",
                "sessionId": "sess_rows",
                "type": "sendText",
                "payload": { "text": text },
                "issuedAt": index as u64 + 1,
            }),
        ));
    }
    let mut queries = String::new();
    queries.push_str(&request_line(
        20,
        "v4/conversation/rowsRange",
        serde_json::json!({
            "sessionId": "sess_rows",
            "limit": 3,
        }),
    ));
    queries.push_str(&request_line(
        21,
        "v4/conversation/rowsRange",
        serde_json::json!({
            "sessionId": "sess_rows",
            "limit": 3,
            "beforeRowId": 4,
        }),
    ));
    // 三个无模型 turn 落库完成（探针看到 6 行）后再发分页查询。
    let ready = RowsReady {
        probe: request_line(
            30,
            "v4/conversation/rowsRange",
            serde_json::json!({ "sessionId": "sess_rows", "limit": 100 }),
        ),
        probe_id: 30,
        expected_rows: 6,
        followup: queries,
    };
    let mut batches = vec![create];
    batches.extend(sends);
    let (responses, _notifications) = drive_batches_with(&root, batches, Some(ready));
    let page_one = &responses[&20];
    assert_eq!(page_one["rows"].as_array().expect("rows").len(), 3);
    // 每轮 turnHeader+userInput（无模型 → 没有 assistantText 行）；尾部窗口从第 2 轮开始。
    assert_eq!(page_one["rows"][0]["kind"], json!("userInput"));
    assert_eq!(page_one["rows"][1]["kind"], json!("turnHeader"));
    assert_eq!(page_one["rows"][2]["kind"], json!("userInput"));
    assert_eq!(page_one["hasMore"], json!(true));
    assert!(page_one["atLogEpoch"]
        .as_str()
        .expect("epoch")
        .contains('-'));
    let row_ids: Vec<u64> = page_one["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["rowId"].as_u64().expect("rowId"))
        .collect();
    assert_eq!(row_ids, vec![4, 5, 6]);

    let page_two = &responses[&21];
    let ids_two: Vec<u64> = page_two["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["rowId"].as_u64().expect("rowId"))
        .collect();
    assert_eq!(ids_two, vec![1, 2, 3]);
    assert_eq!(page_two["hasMore"], json!(false));
}

#[test]
fn resync_and_unsubscribe_use_documented_params() {
    let root = TempRoot::new("resync");
    let create = request_line(
        1,
        "session/create",
        serde_json::json!({
            "sessionId": "sess_resync",
            "workspace": { "workspacePath": WORKSPACE, "workspaceKey": WORKSPACE },
        }),
    );
    let resync = request_line(
        2,
        "v4/conversation/resync",
        serde_json::json!({
            "topic": "conversation/sess_resync",
            "connectionId": "conn-resync",
            "subscriptionId": "sub-fixture",
            "base": { "logEpoch": "epoch-fixture", "seq": 3 },
        }),
    );
    let unsubscribe = request_line(
        3,
        "v4/conversation/unsubscribe",
        serde_json::json!({
            "topic": "conversation/sess_resync",
            "connectionId": "conn-resync",
            "subscriptionId": "sub-fixture",
        }),
    );
    let (responses, notifications) = drive(&root, format!("{create}{resync}{unsubscribe}"));
    let ack = &responses[&2]["ack"];
    assert_eq!(ack["subscriptionId"], json!("sub-fixture"));
    assert_eq!(ack["mode"], json!("resume"));
    assert_eq!(ack["logEpoch"], json!("epoch-fixture"));
    assert_eq!(responses[&3], serde_json::json!({}));
    // resync 必须补一帧 deliveryKind:"recovery"，否则 UI 等帧超时
    // （fault.subscription.recoveryFrameTimedOut）。
    let frames = v4_frames(&notifications);
    assert!(
        frames.iter().any(|frame| {
            frame["params"]["deliveryKind"] == json!("recovery")
                && frame["params"]["frame"]["payload"]["kind"] == json!("snapshot")
        }),
        "resync 未产生 recovery 快照帧：{frames:?}"
    );
}

#[test]
fn usage_and_workflow_queries_read_agent_db() {
    let root = TempRoot::new("usage");
    let data_root = root.path().join(".mode");
    // 先落一个 workflow（startSavedWorkflow 走它）。
    let dirs = WorkflowDirs::new(&data_root.display().to_string(), &data_root);
    std::fs::create_dir_all(&dirs.project).expect("workflows dir");
    std::fs::write(
        dirs.project.join("probe-run.dwf.ts"),
        "/* mode-workflow\ndescription: probe\n*/\nreturn { ok: true };",
    )
    .expect("write workflow");

    let mut input = String::new();
    input.push_str(&request_line(
        1,
        "session/create",
        serde_json::json!({
            "sessionId": "sess_usage",
            "workspace": { "workspacePath": &data_root.display().to_string(), "workspaceKey": &data_root.display().to_string() },
        }),
    ));
    input.push_str(&request_line(
        2,
        "v4/command",
        serde_json::json!({
            "commandId": "cmd-wf-run",
            "clientId": "probe-c4",
            "sessionId": "sess_usage",
            "type": "startSavedWorkflow",
            "payload": { "name": "probe-run", "scope": "project" },
            "issuedAt": 1,
        }),
    ));
    input.push_str(&request_line(
        3,
        "v4/conversation/usage",
        serde_json::json!({ "sessionId": "sess_usage" }),
    ));
    input.push_str(&request_line(
        4,
        "v4/conversation/workflowRuns",
        serde_json::json!({ "sessionId": "sess_usage" }),
    ));
    input.push_str(&request_line(
        5,
        "v4/conversation/fileChanges",
        serde_json::json!({
            "sessionId": "sess_usage",
            "target": { "rowId": 1, "entityId": "entity" },
            "baseRevision": 0,
            "baseLogEpoch": "",
        }),
    ));
    input.push_str(&request_line(
        6,
        "v4/conversation/backgroundBashOutput",
        serde_json::json!({ "sessionId": "sess_usage", "workId": "exec_none" }),
    ));
    let (responses, _notifications) = drive(&root, input);

    // startSavedWorkflow ACK accepted。
    assert_eq!(responses[&2]["status"], json!("accepted"));
    let run_id = responses[&2]["result"]["runId"]
        .as_str()
        .expect("runId")
        .to_string();

    // usage：turn 是配置失败（无模型），但 turn_usage 一定有行。
    let usage = &responses[&3];
    assert_eq!(usage["sessionId"], json!("sess_usage"));
    assert!(usage["totalTokens"].is_number());
    assert!(usage["modelRequestCount"].is_number());

    // workflowRuns 查询：返回刚启动的 run。
    let runs = responses[&4]["runs"].as_array().expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["runId"], json!(run_id));
    assert_eq!(runs[0]["status"], json!("completed"));
    assert_eq!(runs[0]["label"], json!("probe-run"));

    // fileChanges：baseLogEpoch 空串 ≠ 当前 epoch → stale 守卫（观察文案）。
    let file_changes = &responses[&5];
    assert_eq!(
        file_changes["error"]["message"],
        json!("proto.staleLogEpoch")
    );

    // backgroundBashOutput：runtime 读面未接 → unavailable。
    assert_eq!(responses[&6]["kind"], json!("unavailable"));
    assert_eq!(responses[&6]["workId"], json!("exec_none"));
}

#[test]
fn sessions_index_subscribe_emits_snapshot_frame() {
    let root = TempRoot::new("index");
    let mut input = String::new();
    input.push_str(&request_line(
        1,
        "session/create",
        serde_json::json!({
            "sessionId": "sess_idx",
            "workspace": { "workspacePath": WORKSPACE, "workspaceKey": WORKSPACE },
        }),
    ));
    input.push_str(&request_line(
        2,
        "v4/conversation/subscribe",
        serde_json::json!({
            "topic": "sessions-index/C:/mode-v4f-ws",
            "connectionId": "conn-idx",
            "clientMode": "desktop-continuous",
        }),
    ));
    let (responses, notifications) = drive(&root, input);
    assert_eq!(responses[&2]["ack"]["mode"], json!("snapshot"));
    let frames = v4_frames(&notifications);
    assert_eq!(frames.len(), 1, "sessions-index 只发初始快照帧");
    let payload = &frames[0]["params"]["frame"]["payload"];
    assert_eq!(payload["kind"], json!("snapshot"));
    let snapshot = &payload["snapshot"];
    assert_eq!(snapshot["protocolVersion"], json!(1));
    assert_eq!(snapshot["workspaceId"], json!("C:/mode-v4f-ws"));
    let sessions = snapshot["sessions"].as_array().expect("sessions");
    assert!(sessions
        .iter()
        .any(|session| session["sessionId"] == json!("sess_idx")));
}

#[test]
fn sessions_index_emits_live_upsert_for_new_session() {
    let root = TempRoot::new("index-live");
    let mut first = String::new();
    first.push_str(&request_line(
        1,
        "session/create",
        serde_json::json!({
            "sessionId": "sess_live_a",
            "workspace": { "workspacePath": WORKSPACE, "workspaceKey": WORKSPACE },
        }),
    ));
    first.push_str(&request_line(
        2,
        "v4/conversation/subscribe",
        serde_json::json!({
            "topic": "sessions-index/C:/mode-v4f-ws",
            "connectionId": "conn-idx-live",
            "clientMode": "desktop-continuous",
        }),
    ));
    let mut second = String::new();
    second.push_str(&request_line(
        3,
        "session/create",
        serde_json::json!({
            "sessionId": "sess_live_b",
            "workspace": { "workspacePath": WORKSPACE, "workspaceKey": WORKSPACE },
        }),
    ));
    let (responses, notifications) = drive_batches(&root, vec![first, second]);
    assert_eq!(responses[&2]["ack"]["mode"], serde_json::json!("snapshot"));
    let frames = v4_frames(&notifications);
    let deltas: Vec<_> = frames
        .iter()
        .filter(|frame| frame["params"]["frame"]["payload"]["kind"] == serde_json::json!("deltas"))
        .collect();
    assert_eq!(
        deltas.len(),
        1,
        "expected one live upsert, got {} frames",
        frames.len()
    );
    let delta = &deltas[0]["params"]["frame"]["payload"]["deltas"][0];
    assert_eq!(delta["op"], serde_json::json!("session.upserted"));
    assert_eq!(
        delta["session"]["sessionId"],
        serde_json::json!("sess_live_b")
    );
}

// 函数级回归：global 缺省定位（project → global）不被 T4 改动破坏。
#[test]
fn workflow_get_falls_back_to_global_scope() {
    let root = TempRoot::new("get-global");
    let dirs = WorkflowDirs::new(
        &root.path().join("ws").display().to_string(),
        &root.path().join("data"),
    );
    std::fs::create_dir_all(&dirs.global).expect("global dir");
    std::fs::write(
        dirs.global.join("probe-run.dwf.ts"),
        "/* mode-workflow\ndescription: g\n*/\n",
    )
    .expect("write");
    match workflows::get(&dirs, "probe-run", Some(SavedWorkflowScope::Global)) {
        WorkflowsGetResult::Ok(ok) => assert_eq!(ok.scope, SavedWorkflowScope::Global),
        other => panic!("expected global hit, got {other:?}"),
    }
}

#[test]
fn workspace_config_subscribe_emits_snapshot_frame() {
    let root = TempRoot::new("workspace-config");
    let subscribe = request_line(
        1,
        "v4/conversation/subscribe",
        serde_json::json!({
            "topic": "workspace-config/C:/mode-v4f-ws",
            "connectionId": "conn-wcfg",
            "clientMode": "desktop-continuous",
        }),
    );
    let (responses, notifications) = drive_batches(&root, vec![subscribe]);
    let ack = &responses[&1]["ack"];
    assert_eq!(ack["mode"], json!("snapshot"));
    assert!(ack["subscriptionId"]
        .as_str()
        .expect("subscription id")
        .starts_with("wcfg-"));

    let frame = &v4_frames(&notifications)[0]["params"];
    assert_eq!(frame["kind"], json!("complete"));
    assert_eq!(frame["deliveryKind"], json!("initial"));
    assert_eq!(frame["topic"], json!("workspace-config/C:/mode-v4f-ws"));
    let payload = &frame["frame"]["payload"];
    assert_eq!(payload["kind"], json!("snapshot"));
    assert_eq!(payload["snapshot"]["protocolVersion"], json!(1));
    assert_eq!(payload["snapshot"]["workspaceId"], json!("C:/mode-v4f-ws"));
    assert_eq!(payload["snapshot"]["config"]["configOptions"], json!([]));
    assert_eq!(payload["snapshot"]["config"]["slashCommands"], json!([]));
}
