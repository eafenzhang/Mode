//! `session/subscribe` + `session/event`：replay 补号、实时过滤与去重。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, SpecFactory,
};
use agent_protocol::snapshot::DeliveryKind;
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-session-events-stream-{label}-{}-{}.log",
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
async fn subscribes_with_replay_and_filters_live_events() {
    let log = FrameLog::new("replay");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("session-events-stream", &log, spawns);

    let mut subscription = client
        .subscribe_session(
            &AgentWorkspaceTarget::new("/ws/demo"),
            "sess-1",
            DeliveryKind::DesktopContinuous,
            Some(0),
            false,
        )
        .await
        .unwrap();

    // replay：两条 0 序号事件被补成 1、2，且按 seq 排序。
    assert_eq!(subscription.replay.len(), 2);
    assert_eq!(subscription.replay[0].event_id, "ev-1");
    assert_eq!(subscription.replay[0].seq, 1);
    assert_eq!(subscription.replay[1].event_id, "ev-2");
    assert_eq!(subscription.replay[1].seq, 2);
    assert!(subscription.snapshot.is_none());

    // live：重复 ev-2 被丢弃、其它会话被过滤，只有 ev-3 到达并补 seq=3。
    let event = tokio::time::timeout(Duration::from_secs(2), subscription.recv())
        .await
        .expect("必须收到实时事件")
        .expect("事件流仍在");
    assert_eq!(event.event_id, "ev-3");
    assert_eq!(event.session_id, "sess-1");
    assert_eq!(event.seq, 3);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), subscription.recv())
            .await
            .is_err(),
        "不应再有事件（重复与其它会话都被过滤）"
    );

    let params = log.frame_params("session/subscribe").expect("必须记录到请求帧");
    assert_eq!(params["sessionId"], "sess-1");
    assert_eq!(params["deliveryKind"], "desktop-continuous");
    assert_eq!(params["afterSeq"], 0);
    assert_eq!(params["includeSnapshot"], false);
}
