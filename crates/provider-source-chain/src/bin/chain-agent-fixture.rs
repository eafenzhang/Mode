//! 最小 Mode agent 测试替身。
//!
//! 放在本包而不是复用 `agent-process` 的 fixture：集成测试只能拿到本包的
//! `CARGO_BIN_EXE_*`，跨包复用会引入 dev-dependency 环。

use std::io::{BufRead, Write};

use serde_json::{json, Value};

fn main() {
    let log_path = std::env::var("AGENT_CHAIN_FRAME_LOG").ok();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
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
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        if let Some(path) = &log_path {
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(file, "frame\t{method}\t{params}");
            }
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let reply = if method == "provider/updateAccountConfig" {
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
            json!({
                "id": id,
                "result": {
                    "receivedRevision": revision,
                    "providerCount": provider_count,
                    "status": "received"
                }
            })
        } else {
            json!({ "id": id, "result": { "method": method } })
        };
        if writeln!(stdout, "{reply}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
}
