//! `session/events`：游标参数、信封严格解析与缺省省略。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    SpecFactory,
};
use agent_protocol::session_event::SessionEventType;
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-session-events-{label}-{}-{}.log",
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
                    let payload = line.splitn(3, '\t').nth(2)?;
                    return serde_json::from_str(payload).ok();
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
async fn read_session_events_sends_cursors_and_parses_envelope() {
    let log = FrameLog::new("ok");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));

    let events = client
        .read_session_events(
            &AgentWorkspaceTarget::new("/ws/demo"),
            "sess-1",
            Some(10),
            Some(20),
        )
        .await
        .unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, SessionEventType::SessionUpdated);
    assert_eq!(events[0].payload, Some(serde_json::json!({})));
    assert_eq!(spawns.load(Ordering::SeqCst), 1);

    let params = log.frame_params("session/events").expect("必须记录到请求帧");
    assert_eq!(params["sessionId"], "sess-1");
    assert_eq!(params["afterSeq"], 10);
    assert_eq!(params["limit"], 20);
}

#[tokio::test]
async fn omitted_cursors_are_not_serialized() {
    let log = FrameLog::new("omitted");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));

    client
        .read_session_events(&AgentWorkspaceTarget::new("/ws/demo"), "sess-1", None, None)
        .await
        .unwrap();

    let params = log.frame_params("session/events").expect("必须记录到请求帧");
    assert_eq!(params["sessionId"], "sess-1");
    assert!(params.get("afterSeq").is_none(), "afterSeq 缺省时不能出现键");
    assert!(params.get("limit").is_none(), "limit 缺省时不能出现键");
}

#[tokio::test]
async fn unknown_event_fields_are_rejected() {
    let log = FrameLog::new("bad-result");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-session-events", &log, spawns);

    let error = client
        .read_session_events(&AgentWorkspaceTarget::new("/ws/demo"), "sess-1", None, None)
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid session/events result"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
