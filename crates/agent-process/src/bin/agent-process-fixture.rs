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
        } else {
            json!({ "id": id, "result": { "method": method } })
        };
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
