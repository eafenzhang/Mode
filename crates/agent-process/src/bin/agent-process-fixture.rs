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

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "exit-immediately" {
        return;
    }

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
        eprintln!("handled:{method}");
        let reply = json!({
            "id": request.get("id").cloned().unwrap_or(Value::Null),
            "result": { "method": method }
        });
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
