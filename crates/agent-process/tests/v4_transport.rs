//! v4 会话订阅：ACK、wire frame 过滤与分片外壳。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    SpecFactory,
};
use agent_protocol::v4_transport::{
    ConversationSubscribeParams, ConversationWireFrame, SubscribeMode, V4ClientMode,
};
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-v4-transport-{label}-{}-{}.log",
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

fn params() -> ConversationSubscribeParams {
    ConversationSubscribeParams {
        topic: "conversation/sess-1".to_string(),
        base: None,
        visibility: None,
        connection_id: "conn-1".to_string(),
        client_mode: V4ClientMode::DesktopContinuous,
        workspace: None,
        legacy_task_ids: None,
        resume_thought_level: None,
        workflow_run_deltas: Some(true),
    }
}

#[tokio::test]
async fn subscribes_and_filters_frames_by_subscription() {
    let log = FrameLog::new("ok");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let mut subscription = client
        .subscribe_conversation_v4(&AgentWorkspaceTarget::new("/ws/demo"), params())
        .await
        .unwrap();
    assert_eq!(subscription.ack.subscription_id, "sub-1");
    assert_eq!(subscription.ack.mode, SubscribeMode::Snapshot);
    assert_eq!(subscription.ack.log_epoch, "epoch-1");
    assert!(subscription.ack.open_timing.is_some());

    // 第一条：本订阅的 complete 帧。
    let first = tokio::time::timeout(Duration::from_secs(2), subscription.recv())
        .await
        .expect("必须收到首帧")
        .expect("流仍在");
    match first {
        ConversationWireFrame::Complete(frame) => {
            assert_eq!(frame.logical_frame_id, "f1");
            assert_eq!(frame.subscription_id, "sub-1");
            assert_eq!(frame.frame["payload"]["kind"], "snapshot");
        }
        other => panic!("unexpected frame: {other:?}"),
    }

    // 第二条：其它订阅的帧被过滤后，只应是本订阅的 fragment。
    let second = tokio::time::timeout(Duration::from_secs(2), subscription.recv())
        .await
        .expect("必须收到分片帧")
        .expect("流仍在");
    match second {
        ConversationWireFrame::Fragment(frame) => {
            assert_eq!(frame.logical_frame_id, "f2");
            assert_eq!(frame.subscription_id, "sub-1");
            assert_eq!(frame.checksum.value, "00000000");
        }
        other => panic!("unexpected frame: {other:?}"),
    }

    let sent = log
        .frame_params("v4/conversation/subscribe")
        .expect("必须记录到请求帧");
    assert_eq!(sent["topic"], "conversation/sess-1");
    assert_eq!(sent["connectionId"], "conn-1");
    assert_eq!(sent["clientMode"], "desktop-continuous");
    assert_eq!(sent["workflowRunDeltas"], true);
}

#[tokio::test]
async fn rejects_too_many_legacy_task_ids() {
    let log = FrameLog::new("legacy");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));
    let mut params = params();
    params.legacy_task_ids = Some((0..201).map(|index| format!("task-{index}")).collect());

    let error = match client
        .subscribe_conversation_v4(&AgentWorkspaceTarget::new("/ws/demo"), params)
        .await
    {
        Ok(_) => panic!("legacyTaskIds 超限时不应建立订阅"),
        Err(error) => error,
    };
    match error {
        ControlError::InvalidParams(message) => {
            assert!(message.contains("legacyTaskIds"), "{message}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(spawns.load(Ordering::SeqCst), 0, "参数非法不得拉起 Agent");
}
