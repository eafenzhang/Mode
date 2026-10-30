//! v4 命令最小闭环：`resolveInteraction` 的信封、ACK 与拒绝映射。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    SpecFactory,
};
use agent_protocol::v4::{CommandAckStatus, ResolveInteractionAnswer};
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-v4-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn frame_params(&self, method: &str) -> Vec<Value> {
        let marker = format!("frame\t{method}\t");
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                let params: Vec<Value> = text
                    .lines()
                    .filter(|line| line.starts_with(&marker))
                    .filter_map(|line| line.splitn(3, '\t').nth(2)?.parse().ok())
                    .collect();
                if !params.is_empty() {
                    return params;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Vec::new()
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

fn answer() -> ResolveInteractionAnswer {
    ResolveInteractionAnswer {
        option_id: Some("allow_once".to_string()),
        ..ResolveInteractionAnswer::default()
    }
}

#[tokio::test]
async fn resolve_interaction_sends_envelope_and_returns_accepted_ack() {
    let log = FrameLog::new("ok");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);
    let target = AgentWorkspaceTarget::new("/ws/demo");

    let ack = client
        .resolve_interaction_v4(&target, "sess-1", "perm-1", answer())
        .await
        .unwrap();
    assert_eq!(ack.status, CommandAckStatus::Accepted);

    let frames = log.frame_params("v4/command");
    assert_eq!(frames.len(), 1);
    let params = &frames[0];
    assert_eq!(params["type"], "resolveInteraction");
    assert_eq!(params["sessionId"], "sess-1");
    assert_eq!(params["payload"]["interactionId"], "perm-1");
    assert_eq!(params["payload"]["answer"]["optionId"], "allow_once");
    assert!(
        params["commandId"].as_str().is_some_and(|id| !id.is_empty()),
        "commandId 必须非空"
    );
    assert!(
        params.get("baseRevision").is_none(),
        "resolveInteraction 不是 CAS 命令"
    );

    // 同一 client 的 clientId 稳定、commandId 每次不同。
    client
        .resolve_interaction_v4(&target, "sess-1", "perm-2", answer())
        .await
        .unwrap();
    let frames = log.frame_params("v4/command");
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["clientId"], frames[1]["clientId"]);
    assert_ne!(frames[0]["commandId"], frames[1]["commandId"]);
}

#[tokio::test]
async fn rejected_ack_maps_to_command_rejected() {
    let log = FrameLog::new("rejected");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("v4-command-rejected", &log, spawns);

    let error = client
        .resolve_interaction_v4(
            &AgentWorkspaceTarget::new("/ws/demo"),
            "sess-1",
            "perm-1",
            answer(),
        )
        .await
        .unwrap_err();
    match error {
        ControlError::CommandRejected(message) => {
            assert!(message.contains("resolveInteraction rejected"), "{message}");
            assert!(message.contains("fault.command.rejected"), "{message}");
            assert!(message.contains("denied"), "{message}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
