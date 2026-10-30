//! `session/list`：参数形状、严格结果解析与 existing-only 策略。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    ListSessionsOptions, RuntimePolicy, SpecFactory,
};
use agent_protocol::session_read::SessionKind;
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-session-list-{label}-{}-{}.log",
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

fn control(
    mode: &str,
    log: &FrameLog,
    spawns: Arc<AtomicUsize>,
) -> AgentControlClient {
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

fn target() -> AgentWorkspaceTarget {
    AgentWorkspaceTarget::new("/ws/demo").with_identity("  ssh://host/ws/demo  ")
}

#[tokio::test]
async fn list_sessions_sends_workspace_ref_and_parses_strictly() {
    let log = FrameLog::new("ok");
    let spawns = Arc::new(AtomicUsize::new(0));
    let control = control("", &log, Arc::clone(&spawns));

    let sessions = control
        .list_sessions(
            &target(),
            ListSessionsOptions {
                include_archived: true,
                limit: Some(10),
                session_ids: Some(vec!["sess-1".to_string()]),
                runtime_policy: RuntimePolicy::StartIfNeeded,
            },
        )
        .await
        .unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "sess-1");
    assert_eq!(sessions[0].session_kind, SessionKind::Interactive);
    assert_eq!(spawns.load(Ordering::SeqCst), 1);

    let text = log.read();
    let frame = text
        .lines()
        .find(|line| line.starts_with("frame\t"))
        .expect("必须记录到协议帧");
    let mut parts = frame.splitn(3, '\t');
    assert_eq!(parts.next(), Some("frame"));
    assert_eq!(parts.next(), Some("session/list"));
    let params: Value = serde_json::from_str(parts.next().unwrap()).unwrap();
    assert_eq!(params["workspace"]["workspacePath"], "/ws/demo");
    // buildWorkspaceRef 保留原始 identity（含空白），key 已 trim。
    assert_eq!(params["workspace"]["workspaceIdentity"], "  ssh://host/ws/demo  ");
    assert_eq!(params["workspace"]["workspaceKey"], "ssh://host/ws/demo");
    assert_eq!(params["includeArchived"], true);
    assert_eq!(params["limit"], 10);
    assert_eq!(params["sessionIds"], serde_json::json!(["sess-1"]));
}

#[tokio::test]
async fn omitted_options_are_not_serialized() {
    let log = FrameLog::new("omitted");
    let spawns = Arc::new(AtomicUsize::new(0));
    let control = control("", &log, Arc::clone(&spawns));

    control
        .list_sessions(&target(), ListSessionsOptions::default())
        .await
        .unwrap();

    let text = log.read();
    let frame = text
        .lines()
        .find(|line| line.starts_with("frame\t"))
        .expect("必须记录到协议帧");
    let params: Value = serde_json::from_str(frame.splitn(3, '\t').nth(2).unwrap()).unwrap();
    assert_eq!(params["includeArchived"], false);
    assert!(params.get("limit").is_none(), "limit 缺省时不能出现键");
    assert!(params.get("sessionIds").is_none(), "sessionIds 缺省时不能出现键");
}

#[tokio::test]
async fn existing_only_without_runtime_fails_without_spawning() {
    let log = FrameLog::new("existing-only");
    let spawns = Arc::new(AtomicUsize::new(0));
    let control = control("", &log, Arc::clone(&spawns));

    let error = control
        .list_sessions(
            &target(),
            ListSessionsOptions {
                runtime_policy: RuntimePolicy::ExistingOnly,
                ..ListSessionsOptions::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ControlError::RuntimeUnavailable { .. }));
    assert_eq!(error.to_string(), "Mode Agent runtime is not running.");
    assert_eq!(spawns.load(Ordering::SeqCst), 0, "existing-only 不得 spawn");

    // 已有进程时 existing-only 正常复用。
    control
        .list_sessions(&target(), ListSessionsOptions::default())
        .await
        .unwrap();
    control
        .list_sessions(
            &target(),
            ListSessionsOptions {
                runtime_policy: RuntimePolicy::ExistingOnly,
                ..ListSessionsOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unknown_result_fields_are_rejected() {
    let log = FrameLog::new("bad-result");
    let spawns = Arc::new(AtomicUsize::new(0));
    let control = control("bad-session-list", &log, spawns);

    let error = control
        .list_sessions(&target(), ListSessionsOptions::default())
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid session/list result"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
