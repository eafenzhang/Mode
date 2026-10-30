//! mode-agent 控制面 facade：workspace ref、协议请求与结果严格解析。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    build_workspace_ref, AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, ControlError,
    GrantTrustResult, SpecFactory,
};
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-agent-control-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn read(&self) -> String {
        // fixture 是异步写文件；给一个很短的稳定窗口。
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                if text.contains("frame\t") {
                    return text;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::read_to_string(&self.0).unwrap_or_default()
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn registry(mode: &str, frame_log: &FrameLog, spawns: Arc<AtomicUsize>) -> AgentRuntimeRegistry {
    let mode = mode.to_string();
    let log = frame_log.0.to_string_lossy().into_owned();
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
    AgentRuntimeRegistry::new(factory)
}

fn digests() -> (String, String) {
    ("a".repeat(64), "b".repeat(64))
}

#[tokio::test]
async fn grant_sends_workspace_ref_and_parses_result() {
    let log = FrameLog::new("grant");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("", &log, Arc::clone(&spawns)));
    let control = AgentControlClient::new(registry);
    let target = agent_process::AgentWorkspaceTarget::new("D:/ws/a")
        .with_identity(" ssh://host/D:/ws/a ")
        .with_remote_session("s-1");
    let (bundle, declaration) = digests();

    let result = control
        .grant_workspace_hook_trust(&target, &bundle, &declaration)
        .await
        .expect("grant 必须成功");
    assert_eq!(
        result,
        GrantTrustResult {
            accepted: true,
            reason_code: None
        }
    );
    assert_eq!(spawns.load(Ordering::SeqCst), 1);

    let text = log.read();
    let frame = text
        .lines()
        .find(|line| line.starts_with("frame\t"))
        .expect("必须记录到协议帧");
    let mut parts = frame.splitn(3, '\t');
    assert_eq!(parts.next(), Some("frame"));
    assert_eq!(parts.next(), Some("workspace/hooks/trustGrant"));
    let params: Value = serde_json::from_str(parts.next().unwrap()).unwrap();
    assert_eq!(params["bundleDigest"], bundle);
    assert_eq!(params["hookDeclarationDigest"], declaration);
    assert_eq!(params["workspace"]["workspacePath"], "D:/ws/a");
    assert_eq!(params["workspace"]["workspaceIdentity"], " ssh://host/D:/ws/a ");
    assert_eq!(params["workspace"]["remoteSessionId"], "s-1");
    assert_eq!(
        params["workspace"]["workspaceKey"],
        "ssh://host/D:/ws/a",
        "workspaceKey 必须 trim identity"
    );
}

#[tokio::test]
async fn invalid_digest_fails_without_spawning() {
    let log = FrameLog::new("digest");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("", &log, Arc::clone(&spawns)));
    let control = AgentControlClient::new(registry);
    let target = agent_process::AgentWorkspaceTarget::new("D:/ws/a");
    let error = control
        .grant_workspace_hook_trust(&target, "short", &"b".repeat(64))
        .await
        .unwrap_err();
    assert!(matches!(error, ControlError::InvalidDigest { .. }));
    assert_eq!(spawns.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn server_error_message_is_preserved() {
    let log = FrameLog::new("server-error");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("error-grant", &log, Arc::clone(&spawns)));
    let control = AgentControlClient::new(registry);
    let target = agent_process::AgentWorkspaceTarget::new("D:/ws/a");
    let (bundle, declaration) = digests();
    let error = control
        .grant_workspace_hook_trust(&target, &bundle, &declaration)
        .await
        .unwrap_err();
    assert!(matches!(error, ControlError::Client(_)));
    assert_eq!(error.to_string(), "boom");
}

#[tokio::test]
async fn malformed_result_is_rejected() {
    let log = FrameLog::new("bad-result");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("bad-grant", &log, Arc::clone(&spawns)));
    let control = AgentControlClient::new(registry);
    let target = agent_process::AgentWorkspaceTarget::new("D:/ws/a");
    let (bundle, declaration) = digests();
    let error = control
        .grant_workspace_hook_trust(&target, &bundle, &declaration)
        .await
        .unwrap_err();
    assert!(matches!(error, ControlError::InvalidResult(_)));
}

#[test]
fn workspace_ref_omits_absent_optionals_and_trims_key() {
    let plain = build_workspace_ref(&agent_process::AgentWorkspaceTarget::new("/ws/a"));
    assert_eq!(plain["workspacePath"], "/ws/a");
    assert_eq!(plain["workspaceKey"], "/ws/a");
    assert!(plain.get("workspaceIdentity").is_none());
    assert!(plain.get("remoteSessionId").is_none());

    let whitespace = build_workspace_ref(
        &agent_process::AgentWorkspaceTarget::new("/ws/a").with_identity("   "),
    );
    assert_eq!(whitespace["workspaceIdentity"], "   ");
    assert_eq!(whitespace["workspaceKey"], "/ws/a");
}

#[tokio::test]
async fn registry_reuses_the_same_agent_between_grants() {
    let log = FrameLog::new("reuse");
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry("", &log, Arc::clone(&spawns)));
    let control = AgentControlClient::new(registry);
    let target = agent_process::AgentWorkspaceTarget::new("D:/ws/reuse");
    let (bundle, declaration) = digests();
    control
        .grant_workspace_hook_trust(&target, &bundle, &declaration)
        .await
        .unwrap();
    control
        .grant_workspace_hook_trust(&target, &bundle, &declaration)
        .await
        .unwrap();
    assert_eq!(
        spawns.load(Ordering::SeqCst),
        1,
        "同一 workspace 的 grant 必须复用 Agent"
    );
}
