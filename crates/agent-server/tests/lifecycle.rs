//! 二进制生命周期：真实子进程拉起、一帧进一帧出、额外参数忽略并记录、非法启动参数
//! 与非 EOF 场景的退出码、EOF 干净退出。
//!
//! Task 7 起，二进制入口带启动存储门；C2 T7 起 fresh 根走迁移，门帧是
//! `checking ×3 → migrating ×22 → committing ×1 → ready ×1`（27 帧）。本文件的用例把
//! `MODE_DATA_BASE_DIR` 指向临时目录（**绝不**让测试进程触碰真实 `~/.mode`），读响应前
//! 先跳过门通知。

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use agent_server::{ErrorObject, ErrorResponseFrame, Frame, RequestId};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

const BIN: &str = env!("CARGO_BIN_EXE_agent-server");

fn temp_data_base_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "agent-server-lifecycle-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("建临时基目录");
    dir
}

fn spawn(args: &[&str], label: &str) -> (Child, PathBuf) {
    let base = temp_data_base_dir(label);
    let child = Command::new(BIN)
        .args(args)
        .env("MODE_DATA_BASE_DIR", &base)
        // 与对拍纪律一致：不继承真实 provider 配置路径。
        .env_remove("MODE_PERSONAL_PROVIDER_CONFIG_FILE")
        .env_remove("MODE_BUILTIN_PROVIDER_CONFIG_FILE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("agent-server 必须能启动");
    (child, base)
}

async fn next_frame(
    stdout: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
) -> Frame {
    let line = tokio::time::timeout(Duration::from_secs(5), stdout.next_line())
        .await
        .expect("帧读取超时")
        .expect("读取 stdout 必须成功")
        .expect("进程不得在应答前关闭 stdout");
    serde_json::from_str(&line).expect("出站必须是合法帧")
}

/// 存储门通知（`startup/storageState`，fresh 根 27 帧：checking ×3 → migrating ×22 →
/// committing → ready；失败路径以 failed 终止）。
async fn skip_storage_gate(
    stdout: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
) -> Vec<Value> {
    let mut states = Vec::new();
    loop {
        match next_frame(stdout).await {
            Frame::Notification(notification) => {
                assert_eq!(notification.method, "startup/storageState");
                states.push(notification.params.expect("门通知必须带 params"));
            }
            other => panic!("门通知之后才允许有其他帧，实际 {other:?}"),
        }
        let last = states.last().expect("有帧");
        let phase = last["phase"].as_str().expect("phase");
        if phase == "ready" || phase == "failed" {
            return states;
        }
    }
}

fn request_line() -> String {
    serde_json::to_string(&json!({
        "id": 1,
        "method": "session/unknown",
        "params": {}
    }))
    .expect("请求帧必须可序列化")
}

#[tokio::test]
async fn round_trip_then_eof_exits_successfully() {
    let (mut child, _base) = spawn(&["app-server", "--stdio"], "round-trip");
    let mut stdin = child.stdin.take().expect("stdin 必须被 pipe");
    let stdout = child.stdout.take().expect("stdout 必须被 pipe");
    let mut stdout = BufReader::new(stdout).lines();

    // 启动存储门：临时基目录是空根 → fresh-install 迁移 → checking ×3 →
    // migrating ×22 → committing ×1 → ready ×1（27 帧，C2 契约 §5.1）。
    let gate = skip_storage_gate(&mut stdout).await;
    assert_eq!(gate.len(), 27, "fresh 启动必须发 27 帧");
    let phases: Vec<&str> = gate
        .iter()
        .map(|state| state["phase"].as_str().expect("phase"))
        .collect();
    let mut expected_phases = vec!["checking", "checking", "checking"];
    expected_phases.extend(std::iter::repeat_n("migrating", 22));
    expected_phases.push("committing");
    expected_phases.push("ready");
    assert_eq!(phases, expected_phases);
    assert_eq!(gate[0]["schemaVersion"], json!(1));
    assert_eq!(gate[0]["databaseKind"], json!("session"));
    assert_eq!(gate[26]["migration"]["committedCount"], json!(22));

    let line = format!("{}\n", request_line());
    stdin
        .write_all(line.as_bytes())
        .await
        .expect("写请求必须成功");
    stdin.flush().await.expect("flush 必须成功");

    let response = next_frame(&mut stdout).await;
    assert_eq!(
        response,
        Frame::ErrorResponse(ErrorResponseFrame {
            id: RequestId::Number(1),
            error: ErrorObject {
                code: -32601,
                message: "Method not found".to_string(),
                data: None,
            },
        })
    );

    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("EOF 后进程必须在有界时间内退出")
        .expect("等待子进程必须成功");
    assert!(status.success(), "EOF 必须干净退出，实际状态：{status}");
}

#[tokio::test]
async fn extra_arguments_are_ignored_and_logged() {
    let (mut child, _base) = spawn(
        &["app-server", "--stdio", "--surface", "desktop"],
        "extra-args",
    );
    let mut stdin = child.stdin.take().expect("stdin 必须被 pipe");
    let stdout = child.stdout.take().expect("stdout 必须被 pipe");
    let stderr = child.stderr.take().expect("stderr 必须被 pipe");
    let mut stdout = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();

    let _gate = skip_storage_gate(&mut stdout).await;

    let line = format!("{}\n", request_line());
    stdin
        .write_all(line.as_bytes())
        .await
        .expect("写请求必须成功");
    stdin.flush().await.expect("flush 必须成功");

    let frame = next_frame(&mut stdout).await;
    assert!(matches!(
        frame,
        Frame::ErrorResponse(response) if response.error.code == -32601
    ));

    let log = tokio::time::timeout(Duration::from_secs(5), stderr_lines.next_line())
        .await
        .expect("stderr 日志超时")
        .expect("读取 stderr 必须成功")
        .expect("stderr 必须记录被忽略的参数");
    assert!(log.contains("ignoring arguments"), "实际日志：{log}");
    assert!(log.contains("--surface"), "实际日志：{log}");
    assert!(log.contains("desktop"), "实际日志：{log}");

    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("EOF 后进程必须在有界时间内退出")
        .expect("等待子进程必须成功");
    assert!(status.success(), "EOF 必须干净退出，实际状态：{status}");
}

#[tokio::test]
async fn unsupported_argv_exits_with_failure() {
    let cases: &[&[&str]] = &[&[], &["app-server"], &["--stdio"], &["serve"]];
    for args in cases {
        let (mut child, _base) = spawn(args, "bad-argv");
        let stderr = child.stderr.take().expect("stderr 必须被 pipe");
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("不受支持的启动参数必须在有界时间内退出")
            .expect("等待子进程必须成功");
        assert!(
            !status.success(),
            "不受支持的启动参数必须以非零码退出，args={args:?}，实际状态：{status}"
        );

        let mut log = String::new();
        BufReader::new(stderr)
            .read_to_string(&mut log)
            .await
            .expect("读取 stderr 必须成功");
        assert!(
            log.contains("expected `app-server --stdio`"),
            "args={args:?}，stderr 缺少明确错误：{log}"
        );
    }
}

#[tokio::test]
async fn invalid_json_terminates_with_failure() {
    let (mut child, _base) = spawn(&["app-server", "--stdio"], "invalid-json");
    let mut stdin = child.stdin.take().expect("stdin 必须被 pipe");
    stdin
        .write_all(b"not json\n")
        .await
        .expect("写非法帧必须成功");
    stdin.flush().await.expect("flush 必须成功");
    drop(stdin);

    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("非法 JSON 后进程必须在有界时间内退出")
        .expect("等待子进程必须成功");
    assert!(
        !status.success(),
        "非法 JSON 必须关闭连接并以失败退出，实际状态：{status}"
    );
}
