//! `session/subagents`：默认分页、可选游标与严格结果。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    SpecFactory,
};
use agent_protocol::session_read::RunningSubagentStatus;
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-session-subagents-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn frame_params(&self, method: &str) -> Option<Value> {
        let marker = format!("frame\t{method}\t");
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                if let Some(line) = text.lines().find(|line| line.starts_with(&marker)) {
                    return line.splitn(3, '\t').nth(2)?.parse().ok();
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn control(mode: &str, log: &FrameLog, spawns: Arc<AtomicUsize>) -> AgentControlClient {
    let mode = mode.to_string();
    let log = log.0.to_string_lossy().into_owned();
    let factory: SpecFactory = Arc::new(move |_target| {
        spawns.fetch_add(1, Ordering::SeqCst);
        let mut spec = AgentProcessSpec::new(FIXTURE);
        if !mode.is_empty() {
            spec.args.push(mode.clone());
        }
        spec.env
            .push(("AGENT_FIXTURE_FRAME_LOG".to_string(), log.clone()));
        Ok(spec)
    });
    AgentControlClient::new(Arc::new(AgentRuntimeRegistry::new(factory)))
}

#[tokio::test]
async fn defaults_ended_limit_and_parses_result() {
    let log = FrameLog::new("default-limit");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let result = client
        .list_session_subagents(&AgentWorkspaceTarget::new("/ws/demo"), "sess-1", None, None)
        .await
        .unwrap();

    assert_eq!(result.revision, 3);
    assert_eq!(result.child_session_ids, vec!["sess-2".to_string()]);
    assert_eq!(result.running.len(), 1);
    assert_eq!(result.running[0].status, RunningSubagentStatus::Running);
    assert_eq!(result.ended.total, 0);
    assert!(result.ended.items.is_empty());

    let params = log.frame_params("session/subagents").expect("必须记录到请求帧");
    assert_eq!(params["sessionId"], "sess-1");
    assert_eq!(params["endedLimit"], 20, "缺省分页必须是 20");
    assert!(
        params.get("endedCursor").is_none(),
        "endedCursor 缺省时不能出现键"
    );
}

#[tokio::test]
async fn explicit_cursor_and_limit_are_sent() {
    let log = FrameLog::new("explicit");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    client
        .list_session_subagents(
            &AgentWorkspaceTarget::new("/ws/demo"),
            "sess-1",
            Some("cursor-1"),
            Some(50),
        )
        .await
        .unwrap();

    let params = log.frame_params("session/subagents").expect("必须记录到请求帧");
    assert_eq!(params["endedCursor"], "cursor-1");
    assert_eq!(params["endedLimit"], 50);
}

#[tokio::test]
async fn unknown_result_fields_are_rejected() {
    let log = FrameLog::new("bad-result");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-session-subagents", &log, spawns);

    let error = client
        .list_session_subagents(&AgentWorkspaceTarget::new("/ws/demo"), "sess-1", None, None)
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid session/subagents result"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
