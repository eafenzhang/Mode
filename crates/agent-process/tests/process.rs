//! Agent 进程层：真实子进程上的协议往返、stderr、优雅关闭与强杀。

use std::time::{Duration, Instant};

use agent_process::{pid_is_alive, spawn, AgentProcessError, AgentProcessSpec};
use mode_protocol_client::ClientError;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

fn spec(mode: &str) -> AgentProcessSpec {
    let mut spec = AgentProcessSpec::new(FIXTURE);
    if !mode.is_empty() {
        spec.args.push(mode.to_string());
    }
    spec
}

#[tokio::test]
async fn round_trip_and_stderr_lines_over_real_stdio() {
    let spawned = spawn(spec("")).await.expect("fixture 必须能启动");
    let mut stderr = spawned.stderr.resubscribe();    let result = spawned
        .process
        .client()
        .request("ping", Some(serde_json::json!({ "n": 1 })))
        .await
        .expect("请求必须成功");
    assert_eq!(result["method"], "ping");

    let line = tokio::time::timeout(Duration::from_secs(5), stderr.recv())
        .await
        .expect("stderr 超时")
        .expect("stderr 通道");
    assert_eq!(line, "handled:ping");

    spawned.process.shutdown(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn graceful_shutdown_closes_stdin_and_reaps() {
    let spawned = spawn(spec("")).await.expect("fixture 必须能启动");
    let client = spawned.process.client().clone();
    let started = Instant::now();
    spawned.process.shutdown(Duration::from_secs(5)).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "优雅关闭不应走到强杀超时"
    );
    // 子进程已退出，客户端应看到 EOF。
    let deadline = Instant::now() + Duration::from_secs(3);
    while !client.is_closed() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(client.is_closed(), "子进程退出后客户端必须关闭");
    assert_eq!(
        client.request("after-close", None).await.unwrap_err(),
        ClientError::Closed
    );
}

#[tokio::test]
async fn stubborn_child_is_killed_after_grace() {
    let spawned = spawn(spec("stubborn")).await.expect("fixture 必须能启动");
    let pid = spawned.process.pid();
    assert!(pid != 0);
    let result = spawned.process.client().request("hello", None).await;
    assert_eq!(result.unwrap()["method"], "hello");

    let started = Instant::now();
    spawned
        .process
        .shutdown(Duration::from_millis(150))
        .await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "强杀必须在有界时间内完成"
    );

    let deadline = Instant::now() + Duration::from_secs(3);
    while pid_is_alive(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!pid_is_alive(pid), "强杀后进程必须消失（pid {pid}）");
}

#[tokio::test]
async fn child_exit_surfaces_as_closed() {
    let spawned = spawn(spec("exit-immediately"))
        .await
        .expect("fixture 必须能启动");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let error = spawned
        .process
        .client()
        .request_with_timeout("ping", None, Duration::from_secs(2))
        .await
        .unwrap_err();
    assert!(
        matches!(error, ClientError::Closed | ClientError::Transport(_)),
        "实际: {error:?}"
    );
    spawned.process.shutdown(Duration::from_millis(200)).await;
}

#[tokio::test]
async fn missing_command_reports_spawn_error() {
    let result = spawn(AgentProcessSpec::new(
        "mode-agent-process-fixture-does-not-exist",
    ))
    .await;
    let error = match result {
        Ok(_) => panic!("不存在的命令必须失败"),
        Err(error) => error,
    };
    match error {
        AgentProcessError::Spawn { command, .. } => {
            assert!(command.contains("does-not-exist"));
        }
    }
}
