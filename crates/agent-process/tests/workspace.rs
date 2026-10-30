//! workspace 只读面：首屏展示与 MCP 子进程。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    SpecFactory,
};
use agent_protocol::session_read::SessionMode;
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-workspace-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn frame_count(&self, method: &str) -> (usize, Option<Value>) {
        let marker = format!("frame\t{method}\t");
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                let params: Vec<Option<Value>> = text
                    .lines()
                    .filter(|line| line.starts_with(&marker))
                    .map(|line| line.splitn(3, '\t').nth(2).and_then(|p| serde_json::from_str(p).ok()))
                    .collect();
                if !params.is_empty() {
                    return (params.len(), params.into_iter().next().flatten());
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        (0, None)
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
async fn reads_workspace_presentation_with_trimmed_key() {
    let log = FrameLog::new("presentation");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);
    let target = AgentWorkspaceTarget::new("/ws/demo").with_identity(" ssh://host/ws/demo ");

    let presentation = client.read_workspace_presentation(&target).await.unwrap();
    assert_eq!(presentation.mode, SessionMode::Build);
    assert_eq!(presentation.slash_commands.len(), 1);
    assert_eq!(presentation.slash_commands[0].name, "/help");

    let (count, params) = log.frame_count("workspace/readPresentation");
    assert_eq!(count, 1);
    let params = params.expect("请求必须带 workspace");
    assert_eq!(params["workspace"]["workspacePath"], "/ws/demo");
    assert_eq!(params["workspace"]["workspaceIdentity"], " ssh://host/ws/demo ");
    assert_eq!(params["workspace"]["workspaceKey"], "ssh://host/ws/demo");
}

#[tokio::test]
async fn lists_child_processes_without_params() {
    let log = FrameLog::new("children");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let processes = client
        .list_child_processes(&AgentWorkspaceTarget::new("/ws/demo"))
        .await
        .unwrap();
    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].pid, 1234);
    assert_eq!(
        processes[0].mcp_source,
        agent_protocol::workspace::McpSource::Custom
    );

    let (count, params) = log.frame_count("process/childProcesses");
    assert_eq!(count, 1);
    assert!(
        params.is_none() || params == Some(Value::Null),
        "该请求不带 params"
    );
}

#[tokio::test]
async fn malformed_workspace_results_are_rejected() {
    let log = FrameLog::new("bad-presentation");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-workspace-presentation", &log, spawns);
    let error = client
        .read_workspace_presentation(&AgentWorkspaceTarget::new("/ws/demo"))
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid workspace/readPresentation result"),
                "{message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let log = FrameLog::new("bad-children");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-child-processes", &log, spawns);
    let error = client
        .list_child_processes(&AgentWorkspaceTarget::new("/ws/demo"))
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid process/childProcesses result"),
                "{message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
