//! sidecar 监督器（M3 生态域 1 · Task 2 Step 3）。
//!
//! 用 `node -e` 起一个假 sidecar：正常应答、挂起不应答、收到 `crash` 直接退出。

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use plugin_host::protocol::Method;
use plugin_host::supervisor::{SidecarSupervisor, SupervisorConfig, SupervisorError};

const FAKE_SIDECAR: &str = r#"
const rl = require('node:readline').createInterface({ input: process.stdin });
rl.on('line', (line) => {
  const frame = JSON.parse(line);
  const tool = frame.params && frame.params.toolName;
  if (tool === 'hang') return;
  if (tool === 'crash') { setTimeout(() => process.exit(0), 20); return; }
  process.stdout.write(JSON.stringify({ id: frame.id, result: { content: 'ok:' + (tool || frame.method) } }) + '\n');
});
"#;

fn config() -> SupervisorConfig {
    SupervisorConfig {
        program: PathBuf::from("node"),
        args: vec!["-e".to_string(), FAKE_SIDECAR.to_string()],
        request_timeout: Duration::from_secs(2),
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
    }
}

fn content(value: Value) -> String {
    value["content"].as_str().expect("content").to_string()
}

#[test]
fn request_round_trips_through_the_sidecar() {
    let supervisor = SidecarSupervisor::start(config()).expect("启动 sidecar");
    let hello = supervisor
        .request(Method::Hello, json!({}))
        .expect("hello 应答");
    assert_eq!(content(hello), "ok:host/hello");

    let echoed = supervisor
        .request(
            Method::CallTool,
            json!({ "toolName": "echo", "input": { "text": "hi" } }),
        )
        .expect("callTool 应答");
    assert_eq!(content(echoed), "ok:echo");
}

#[test]
fn a_hanging_request_times_out_without_killing_the_process() {
    let supervisor = SidecarSupervisor::start(config()).expect("启动 sidecar");
    let error = supervisor
        .request_with_timeout(
            Method::CallTool,
            json!({ "toolName": "hang" }),
            Duration::from_millis(150),
        )
        .expect_err("应超时");
    assert_eq!(error, SupervisorError::Timeout);

    // 超时只影响那一条请求；进程仍然可用。
    let hello = supervisor
        .request(Method::Hello, json!({}))
        .expect("超时后仍可请求");
    assert_eq!(content(hello), "ok:host/hello");
}

#[test]
fn a_crashed_sidecar_is_restarted_on_the_next_request() {
    let supervisor = SidecarSupervisor::start(config()).expect("启动 sidecar");
    let crash = supervisor
        .request_with_timeout(
            Method::CallTool,
            json!({ "toolName": "crash" }),
            Duration::from_secs(1),
        )
        .expect_err("崩溃应失败");
    match crash {
        SupervisorError::Rpc(error) => assert!(error.message.contains("closed")),
        other => panic!("期望关闭错误，实际 {other:?}"),
    }

    let hello = supervisor
        .request(Method::Hello, json!({}))
        .expect("下一次请求应触发重启并成功");
    assert_eq!(content(hello), "ok:host/hello");
}
