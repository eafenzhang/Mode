//! `model_trajectory` 契约（真源 `packages/services/src/mode-agent/modelTrajectory.ts`）。

use std::path::PathBuf;

use serde_json::{json, Value};

use host_channels::model_trajectory::{read_model_trajectory_from_dirs, sanitize_session_segment};

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "model-trajectory-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn record(session_id: &str, request_id: &str, started_at: &str, extra: Value) -> Value {
    let mut base = json!({
        "type": "model_io",
        "sessionId": session_id,
        "requestId": request_id,
        "startedAt": started_at,
        "request": { "messages": [], "toolNames": [] },
    });
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    base
}

fn write_jsonl(dir: &TempDir, file_name: &str, records: &[Value]) -> PathBuf {
    let path = dir.0.join(file_name);
    let text = records
        .iter()
        .map(|record| record.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, format!("{text}\n")).expect("write jsonl");
    path
}

#[test]
fn maps_records_and_filters_session() {
    let dir = TempDir::new("map");
    let records = vec![
        record(
            "task-1",
            "r2",
            "2026-01-01T00:00:02.000Z",
            json!({
                "attempt": 2,
                "durationMs": 120,
                "turnId": "turn-1",
                "traceId": "trace-1",
                "querySource": "main_turn",
                "model": { "modelId": "glm-5", "providerId": "glm", "role": "main" },
                "request": {
                    "messages": [
                        { "role": "system", "content": "hello" },
                        {
                            "role": "user",
                            "content": [
                                { "type": "text", "text": "hi" },
                                { "type": "tool-call", "toolCallId": "t1", "toolName": "bash", "input": { "cmd": "ls" } }
                            ]
                        },
                        { "role": "tool", "tool_call_id": "t1", "name": "bash", "content": "{\"ok\":true}" },
                        { "role": "tool", "tool_call_id": "t2", "content": "boom", "is_error": true }
                    ],
                    "toolNames": ["bash"]
                },
                "response": {
                    "finishReason": "stop",
                    "text": "done",
                    "toolCalls": [{ "id": "r1", "name": "bash", "input": { "x": 1 } }],
                    "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 },
                    "responseId": "resp-1"
                }
            }),
        ),
        record("task-1", "r1", "2026-01-01T00:00:01.000Z", json!({})),
        record("task-2", "rx", "2026-01-01T00:00:00.000Z", json!({})),
    ];
    write_jsonl(&dir, "model-io-task-1.jsonl", &records);

    let result = read_model_trajectory_from_dirs("task-1", None, std::slice::from_ref(&dir.0));
    assert_eq!(result["taskId"], "task-1");
    assert_eq!(result["available"], true);
    assert_eq!(result["truncated"], false);
    assert_eq!(result["sourceFiles"].as_array().expect("files").len(), 1);
    let records = result["records"].as_array().expect("records");
    assert_eq!(records.len(), 2, "只保留 task-1 并按 startedAt 升序");
    assert_eq!(records[0]["requestId"], "r1");
    let record = &records[1];
    assert_eq!(record["requestId"], "r2");
    assert_eq!(record["attempt"], 2);
    assert_eq!(record["durationMs"], 120);
    assert_eq!(record["callSource"]["kind"], "main");
    assert_eq!(record["callSource"]["querySource"], "main_turn");
    assert_eq!(record["model"]["modelId"], "glm-5");
    assert_eq!(record["request"]["toolNames"], json!(["bash"]));
    let messages = record["request"]["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["parts"][0], json!({ "kind": "text", "text": "hello" }));
    assert_eq!(
        messages[1]["parts"][1],
        json!({
            "kind": "tool-call",
            "toolCallId": "t1",
            "toolName": "bash",
            "input": { "cmd": "ls" }
        })
    );
    // tool 字符串内容：可解析 JSON 原样输出，顶层 tool 关联字段保留。
    assert_eq!(
        messages[2]["parts"][0],
        json!({
            "kind": "tool-result",
            "toolCallId": "t1",
            "toolName": "bash",
            "output": { "ok": true }
        })
    );
    // isError 还原 error-text。
    assert_eq!(
        messages[3]["parts"][0],
        json!({
            "kind": "tool-result",
            "toolCallId": "t2",
            "output": { "type": "error-text", "value": "boom" }
        })
    );
    assert_eq!(record["response"]["finishReason"], "stop");
    assert_eq!(
        record["response"]["toolCalls"][0],
        json!({ "kind": "tool-call", "toolCallId": "r1", "toolName": "bash", "input": { "x": 1 } })
    );
    assert_eq!(
        record["response"]["usage"],
        json!({ "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 })
    );
}

#[test]
fn expands_delta_collections_and_resets_on_tail() {
    let dir = TempDir::new("delta");
    let records = vec![
        record(
            "task-1",
            "r1",
            "2026-01-01T00:00:01Z",
            json!({
                "request": {
                    "messages": [ { "role": "user", "content": "m0" }, { "role": "user", "content": "m1" } ],
                    "body": { "messages": [ { "role": "user", "content": "b0" } ] }
                }
            }),
        ),
        record(
            "task-1",
            "r2",
            "2026-01-01T00:00:02Z",
            json!({
                "request": {
                    "messagesKind": "delta",
                    "messageOffset": 1,
                    "messages": [ { "role": "user", "content": "m2" } ],
                    "sdkMessagesKind": "delta",
                    "sdkMessageOffset": 0,
                    "sdkMessages": [ { "role": "user", "content": "s0" } ],
                    "body": {
                        "bodyMessagesKind": "delta",
                        "bodyMessageOffset": 1,
                        "messages": [ { "role": "user", "content": "b1" } ]
                    }
                }
            }),
        ),
        record(
            "task-1",
            "r3",
            "2026-01-01T00:00:03Z",
            json!({
                "request": {
                    "messagesKind": "tail",
                    "messages": [ { "role": "user", "content": "t0" } ]
                }
            }),
        ),
    ];
    write_jsonl(&dir, "model-io-task-1.jsonl", &records);

    let result = read_model_trajectory_from_dirs("task-1", None, std::slice::from_ref(&dir.0));
    let records = result["records"].as_array().expect("records");
    let second = &records[1]["request"];
    // 输出只投影 request.messages（TS mapRecord 同）；delta 已按上一条展开。
    let messages = second["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(messages[0]["parts"][0]["text"], "m0");
    assert_eq!(messages[1]["parts"][0]["text"], "m2");

    // tail 是新起点：不拼接更早历史。
    let third = &records[2]["request"];
    assert_eq!(third["messages"].as_array().expect("tail").len(), 1);
}

#[test]
fn limit_truncates_keeping_latest() {
    let dir = TempDir::new("limit");
    let records = vec![
        record("task-1", "r1", "2026-01-01T00:00:01Z", json!({})),
        record("task-1", "r2", "2026-01-01T00:00:02Z", json!({})),
    ];
    write_jsonl(&dir, "model-io-task-1.jsonl", &records);

    let result = read_model_trajectory_from_dirs("task-1", Some(1.0), std::slice::from_ref(&dir.0));
    assert_eq!(result["truncated"], true);
    let records = result["records"].as_array().expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["requestId"], "r2");
}

#[test]
fn sanitized_file_name_and_missing_dirs() {
    assert_eq!(sanitize_session_segment("weird/id:1"), "weird-id-1");
    assert_eq!(sanitize_session_segment("///"), "");

    let dir = TempDir::new("sanitize");
    let records = vec![record("weird/id:1", "r1", "2026-01-01T00:00:01Z", json!({}))];
    write_jsonl(&dir, "model-io-weird-id-1.jsonl", &records);
    let result = read_model_trajectory_from_dirs("weird/id:1", None, std::slice::from_ref(&dir.0));
    assert_eq!(result["records"].as_array().expect("records").len(), 1);

    let missing = TempDir::new("missing");
    let result = read_model_trajectory_from_dirs(
        "task-1",
        None,
        &[missing.0.join("debug"), missing.0.join("rollout")],
    );
    assert_eq!(result["available"], true);
    assert!(result["records"].as_array().expect("records").is_empty());
    assert_eq!(result["truncated"], false);
    assert!(result["sourceFiles"].as_array().expect("files").is_empty());
}
