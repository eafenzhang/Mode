//! 测试用 Agent 子进程：在 stdin/stdout 上说 Mode Protocol 的最小子集。
//!
//! 只由 `tests/process.rs` 通过 `CARGO_BIN_EXE_agent-process-fixture` 拉起。
//! 模式：
//! - 默认：逐行回 `{id, result:{method}}`，EOF 后退出；
//! - `exit-immediately`：立即退出（用于验证启动即关闭）；
//! - `stubborn`：EOF 后继续存活（用于验证强杀）。

use std::io::{BufRead, Write};
use std::time::Duration;

use serde_json::{json, Value};

fn write_notification(stdout: &mut std::io::Stdout, params: Value) {
    let message = json!({ "method": "startup/storageState", "params": params });
    let _ = writeln!(stdout, "{message}");
    let _ = stdout.flush();
}

/// `session/read` 与三个 setter 共用的最小合法快照。
fn minimal_snapshot() -> Value {
    json!({
        "protocol": { "name": "Mode Protocol", "version": 1 },
        "session": {
            "sessionId": "sess-1",
            "workspace": { "workspacePath": "/ws/demo", "workspaceKey": "/ws/demo" },
            "sessionKind": "interactive",
            "title": "fixture",
            "mode": "build",
            "status": "idle",
            "createdAt": 1,
            "updatedAt": 2
        },
        "settings": {
            "model": { "available": [] },
            "thoughtLevel": { "enabled": false, "available": [] },
            "mode": { "current": "build" }
        },
        "projection": {
            "sessionId": "sess-1",
            "status": "idle",
            "mode": "build",
            "turnCount": 0,
            "totalTokenCount": 0,
            "contextUsed": 0,
            "contextWindow": 128000,
            "pendingPermissions": [],
            "activeToolCalls": [],
            "backgroundJobs": []
        },
        "runtime": { "eventSeq": 0, "stateRevision": 0, "pendingRequestIds": [] },
        "messages": []
    })
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "exit-immediately" {
        return;
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    if mode == "storage-ready" {
        write_notification(
            &mut stdout,
            json!({
                "schemaVersion": 1, "attemptId": "attempt-1", "sequence": 1,
                "databaseId": "db-1", "databaseKind": "session",
                "phase": "checking", "elapsedMs": 1.0
            }),
        );
        write_notification(
            &mut stdout,
            json!({
                "schemaVersion": 1, "attemptId": "attempt-1", "sequence": 2,
                "databaseId": "db-1", "databaseKind": "session",
                "phase": "ready", "elapsedMs": 2.0
            }),
        );
    }
    if mode == "storage-failed" {
        write_notification(
            &mut stdout,
            json!({
                "schemaVersion": 1, "attemptId": "attempt-1", "sequence": 1,
                "databaseId": "db-1", "databaseKind": "session",
                "phase": "failed", "elapsedMs": 1.0, "errorCode": "sql_failed"
            }),
        );
    }
    for line in stdin.lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        eprintln!("handled:{method}");
        let frame_line = format!(
            "frame\t{method}\t{}",
            request.get("params").cloned().unwrap_or(Value::Null)
        );
        eprintln!("{frame_line}");
        if let Ok(log_path) = std::env::var("AGENT_FIXTURE_FRAME_LOG") {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
            {
                let _ = writeln!(file, "{frame_line}");
            }
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let reply = if method == "workspace/hooks/trustGrant" {
            match mode.as_str() {
                "error-grant" => json!({
                    "id": id,
                    "error": { "code": -32000, "message": "boom" }
                }),
                "bad-grant" => json!({ "id": id, "result": { "accepted": "yes" } }),
                _ => json!({ "id": id, "result": { "accepted": true } }),
            }
        } else if method == "provider/updateAccountConfig" {
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            let revision = params
                .get("revision")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let provider_count = params
                .get("providers")
                .and_then(Value::as_object)
                .map(|providers| providers.len())
                .unwrap_or(0);
            let received = if mode == "bad-provider-revision" {
                "different-revision".to_string()
            } else {
                revision
            };
            json!({
                "id": id,
                "result": {
                    "receivedRevision": received,
                    "providerCount": provider_count,
                    "status": "received"
                }
            })
        } else if method == "session/list" {
            let mut session = json!({
                "sessionId": "sess-1",
                "workspace": {
                    "workspacePath": "/ws/fixture",
                    "workspaceKey": "/ws/fixture"
                },
                "sessionKind": "interactive",
                "title": "fixture",
                "mode": "build",
                "status": "idle",
                "createdAt": 1,
                "updatedAt": 2
            });
            if mode == "bad-session-list" {
                session["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": { "sessions": [session] } })
        } else if method == "session/events" {
            let mut event = json!({
                "eventId": "evt-1",
                "sessionId": "sess-1",
                "seq": 11,
                "timestamp": 1,
                "type": "session.updated",
                "payload": {}
            });
            if mode == "bad-session-events" {
                event["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": { "events": [event] } })
        } else if method == "session/subagents" {
            let mut result = json!({
                "revision": 3,
                "childSessionIds": ["sess-2"],
                "running": [{
                    "childSessionId": "sess-2",
                    "subagentType": "explore",
                    "title": "运行中",
                    "status": "running"
                }],
                "ended": { "total": 0, "items": [] }
            });
            if mode == "bad-session-subagents" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "session/messages" {
            let mut message = json!({
                "info": {
                    "messageId": "msg-1",
                    "sessionId": "sess-1",
                    "role": "user",
                    "time": { "created": 1 },
                    "agent": "mode"
                },
                "parts": [{
                    "partId": "part-1",
                    "sessionId": "sess-1",
                    "messageId": "msg-1",
                    "type": "text",
                    "text": "hi"
                }]
            });
            if mode == "bad-session-messages" {
                message["info"]["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": { "messages": [message] } })
        } else if method == "session/create" {
            let mut snapshot = minimal_snapshot();
            if mode == "bad-session-create" {
                snapshot["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": snapshot })
        } else if method == "session/read" {
            let mut snapshot = minimal_snapshot();
            if mode == "bad-session-read" {
                snapshot["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": snapshot })
        } else if method == "session/setModel"
            || method == "session/setThoughtLevel"
            || method == "session/setMode"
        {
            let mut snapshot = minimal_snapshot();
            if mode == "bad-session-setter" {
                snapshot["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": snapshot })
        } else if method == "session/close" {
            let mut result = json!({ "closed": true });
            if mode == "bad-session-close" {
                result["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": result })
        } else if method == "session/debug" {
            let mut snapshot = json!({
                "sessionId": "sess-1",
                "rounds": [],
                "networkEntries": [],
                "cache": null
            });
            if mode == "bad-session-debug" {
                snapshot["unexpected"] = json!(true);
            }
            json!({ "id": id, "result": snapshot })
        } else {
            json!({ "id": id, "result": { "method": method } })
        };
        if mode == "slow-reply" {
            // 让请求保持「在飞」，用于空闲回收不得中途回收的用例。
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        if writeln!(stdout, "{reply}").is_err() {
            break;
        }
        if stdout.flush().is_err() {
            break;
        }
    }

    if mode == "stubborn" {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}
