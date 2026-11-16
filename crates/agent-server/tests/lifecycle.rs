//! 二进制生命周期：真实子进程拉起、一帧进一帧出、额外参数忽略并记录、EOF 干净退出。

use std::process::Stdio;
use std::time::Duration;

use agent_server::{Frame, RequestId};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

const BIN: &str = env!("CARGO_BIN_EXE_agent-server");

fn spawn(args: &[&str]) -> Child {
    Command::new(BIN)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("agent-server 必须能启动")
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
    let mut child = spawn(&["app-server", "--stdio"]);
    let mut stdin = child.stdin.take().expect("stdin 必须被 pipe");
    let stdout = child.stdout.take().expect("stdout 必须被 pipe");
    let mut lines = BufReader::new(stdout).lines();

    let line = format!("{}\n", request_line());
    stdin
        .write_all(line.as_bytes())
        .await
        .expect("写请求必须成功");
    stdin.flush().await.expect("flush 必须成功");

    let response = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("响应超时")
        .expect("读取 stdout 必须成功")
        .expect("进程不得在应答前关闭 stdout");
    let frame: Frame = serde_json::from_str(&response).expect("应答必须是合法帧");
    assert_eq!(
        frame,
        Frame::Error {
            id: RequestId::Number(1),
            code: -32601,
            message: "Method not found".to_string(),
            data: None,
        }
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
    let mut child = spawn(&["app-server", "--stdio", "--surface", "desktop"]);
    let mut stdin = child.stdin.take().expect("stdin 必须被 pipe");
    let stdout = child.stdout.take().expect("stdout 必须被 pipe");
    let stderr = child.stderr.take().expect("stderr 必须被 pipe");
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();

    let line = format!("{}\n", request_line());
    stdin
        .write_all(line.as_bytes())
        .await
        .expect("写请求必须成功");
    stdin.flush().await.expect("flush 必须成功");

    let response = tokio::time::timeout(Duration::from_secs(5), stdout_lines.next_line())
        .await
        .expect("响应超时")
        .expect("读取 stdout 必须成功")
        .expect("进程不得在应答前关闭 stdout");
    let frame: Frame = serde_json::from_str(&response).expect("应答必须是合法帧");
    assert!(matches!(frame, Frame::Error { code: -32601, .. }));

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
async fn invalid_json_terminates_with_failure() {
    let mut child = spawn(&["app-server", "--stdio"]);
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
