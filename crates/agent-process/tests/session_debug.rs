//! `session/debug`：existing-only 语义与严格诊断快照。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget,
    ControlError, ListSessionsOptions, SpecFactory,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-session-debug-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
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
async fn existing_only_requires_a_live_runtime() {
    let log = FrameLog::new("existing-only");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));
    let target = AgentWorkspaceTarget::new("/ws/demo");

    let error = client
        .read_session_debug(&target, "sess-1")
        .await
        .unwrap_err();
    assert!(matches!(error, ControlError::RuntimeUnavailable { .. }));
    assert_eq!(spawns.load(Ordering::SeqCst), 0, "诊断读取不得拉起 Agent");

    // 已有进程时正常读取。
    client
        .list_sessions(&target, ListSessionsOptions::default())
        .await
        .unwrap();
    let snapshot = client.read_session_debug(&target, "sess-1").await.unwrap();
    assert_eq!(snapshot.session_id, "sess-1");
    assert!(snapshot.rounds.is_empty());
    assert!(snapshot.network_entries.is_empty());
    assert!(snapshot.cache.is_none());
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unknown_debug_fields_are_rejected() {
    let log = FrameLog::new("bad-result");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-session-debug", &log, Arc::clone(&spawns));
    let target = AgentWorkspaceTarget::new("/ws/demo");

    client
        .list_sessions(&target, ListSessionsOptions::default())
        .await
        .unwrap();
    let error = client
        .read_session_debug(&target, "sess-1")
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid session/debug result"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
