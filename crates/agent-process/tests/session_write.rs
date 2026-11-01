//! 会话写路径：create / close / setModel / setThoughtLevel / setMode 的参数与结果。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    CreateSessionOptions, SpecFactory,
};
use agent_protocol::message::MessageModel;
use agent_protocol::session::SessionSendParams;
use agent_protocol::session::WorkspaceRef;
use agent_protocol::session_read::SessionMode;
use agent_protocol::session_write::{
    SessionCompactParams, SessionGoalAction, SessionGoalParams, SessionPersistence,
    SessionResumeParams,
};
use serde_json::{json, Value};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-session-write-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn frame_params_all(&self, method: &str) -> Vec<Value> {
        let marker = format!("frame	{method}	");
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
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Vec::new()
    }

    fn frame_params(&self, method: &str) -> Option<Value> {
        let marker = format!("frame\t{method}\t");
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                if let Some(line) = text.lines().find(|line| line.starts_with(&marker)) {
                    return line.splitn(3, '\t').nth(2)?.parse().ok();
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
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

fn model() -> MessageModel {
    MessageModel {
        provider_id: "zai".to_string(),
        model_id: "glm-4.6".to_string(),
        options: None,
    }
}

#[tokio::test]
async fn create_session_sends_workspace_and_flags() {
    let log = FrameLog::new("create");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);
    let target = AgentWorkspaceTarget::new("/ws/demo").with_identity(" ssh://host/ws/demo ");

    let snapshot = client
        .create_session(
            &target,
            CreateSessionOptions {
                session_id: Some("sess-new".to_string()),
                mode: Some(SessionMode::Plan),
                model: Some(model()),
                persistence: Some(SessionPersistence::Immediate),
                thought_level: Some("high".to_string()),
                title_generation_enabled: Some(false),
                tool_allowlist: Some(vec!["Read".to_string()]),
                tool_denylist: Some(vec!["Bash".to_string()]),
                off_peak_tool_enabled: true,
                dynamic_workflow_enabled: false,
                ..CreateSessionOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(snapshot.session.session_id, "sess-1");

    let params = log
        .frame_params("session/create")
        .expect("必须记录到请求帧");
    assert_eq!(params["sessionId"], "sess-new");
    assert_eq!(params["workspace"]["workspacePath"], "/ws/demo");
    assert_eq!(
        params["workspace"]["workspaceIdentity"],
        " ssh://host/ws/demo "
    );
    assert_eq!(params["workspace"]["workspaceKey"], "ssh://host/ws/demo");
    assert_eq!(params["mode"], "plan");
    assert_eq!(params["model"]["providerId"], "zai");
    assert_eq!(params["persistence"], "immediate");
    assert_eq!(params["thoughtLevel"], "high");
    assert_eq!(params["titleGenerationEnabled"], false);
    assert_eq!(params["toolAllowlist"], serde_json::json!(["Read"]));
    assert_eq!(params["toolDenylist"], serde_json::json!(["Bash"]));
    assert_eq!(params["offPeakToolEnabled"], true);
    assert!(
        params.get("dynamicWorkflowEnabled").is_none(),
        "灰度未命中时不能写字段"
    );
}

#[tokio::test]
async fn create_session_omits_every_unset_optional() {
    let log = FrameLog::new("create-defaults");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    client
        .create_session(
            &AgentWorkspaceTarget::new("/ws/demo"),
            CreateSessionOptions::default(),
        )
        .await
        .unwrap();

    let params = log
        .frame_params("session/create")
        .expect("必须记录到请求帧");
    assert_eq!(params["workspace"]["workspacePath"], "/ws/demo");
    for key in [
        "sessionId",
        "parentSessionId",
        "mode",
        "model",
        "persistence",
        "thoughtLevel",
        "titleGenerationEnabled",
        "mcpServers",
        "toolAllowlist",
        "toolDenylist",
        "importedHistory",
        "offPeakToolEnabled",
        "dynamicWorkflowEnabled",
    ] {
        assert!(params.get(key).is_none(), "{key} 缺省时不能出现键");
    }
}

#[tokio::test]
async fn malformed_create_result_is_rejected() {
    let log = FrameLog::new("bad-create");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-session-create", &log, spawns);
    let error = client
        .create_session(
            &AgentWorkspaceTarget::new("/ws/demo"),
            CreateSessionOptions::default(),
        )
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid session/create result"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn send_prompt_sends_full_params_and_returns_ack() {
    let log = FrameLog::new("send");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);
    let target = AgentWorkspaceTarget::new("/ws/demo");

    let params: SessionSendParams = serde_json::from_value(json!({
        "sessionId": "sess-1",
        "modelSelection": { "providerId": "zai", "modelId": "glm-4.6" },
        "modelExecution": {
            "selectionScope": "execution",
            "subagents": { "foregroundModel": "submission", "background": "deny" }
        },
        "content": "你好",
        "expectedRevision": 3,
        "offPeakTaskId": "task-1",
        "offPeakRunType": "resume",
        "botDeliveryTarget": {
            "provider": "feishu",
            "botId": "bot-1",
            "providerUserId": "user-1",
            "chatType": "private"
        },
        "toolDenylist": ["Bash"]
    }))
    .unwrap();

    let result = client.send_prompt(&target, params).await.unwrap();
    assert!(result.accepted);
    assert_eq!(result.session_id, "sess-1");
    assert_eq!(result.state_revision, 3);

    let sent = log.frame_params("session/send").expect("必须记录到请求帧");
    assert_eq!(sent["sessionId"], "sess-1");
    assert_eq!(sent["modelSelection"]["providerId"], "zai");
    assert_eq!(sent["modelExecution"]["selectionScope"], "execution");
    assert_eq!(sent["offPeakTaskId"], "task-1");
    assert_eq!(sent["offPeakRunType"], "resume");
    assert_eq!(sent["botDeliveryTarget"]["provider"], "feishu");
    assert_eq!(sent["toolDenylist"], json!(["Bash"]));
}

#[tokio::test]
async fn send_prompt_rejects_invalid_combinations_without_spawning() {
    let log = FrameLog::new("send-invalid");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));
    let target = AgentWorkspaceTarget::new("/ws/demo");

    let params: SessionSendParams = serde_json::from_value(json!({
        "sessionId": "sess-1",
        "modelExecution": { "selectionScope": "execution" },
        "content": "hi"
    }))
    .unwrap();
    let error = client.send_prompt(&target, params).await.unwrap_err();
    match error {
        ControlError::InvalidParams(message) => {
            assert_eq!(message, "modelExecution requires modelSelection");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(spawns.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn send_prompt_rejects_unaccepted_result() {
    let log = FrameLog::new("send-rejected");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("rejected-session-send", &log, spawns);
    let params = SessionSendParams {
        session_id: "sess-1".to_string(),
        model_selection: None,
        model_execution: None,
        input_id: None,
        query_id: None,
        content: "hi".to_string(),
        attachments: None,
        browser_ambient_context: None,
        expected_revision: None,
        expected_provider_revision: None,
        automation_id: None,
        off_peak_task_id: None,
        off_peak_run_type: None,
        bot_delivery_target: None,
        tool_denylist: None,
    };
    let error = client
        .send_prompt(&AgentWorkspaceTarget::new("/ws/demo"), params)
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(message.contains("accepted must be true"), "{message}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn set_model_sends_revision_and_persist_flag() {
    let log = FrameLog::new("set-model");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);
    let target = AgentWorkspaceTarget::new("/ws/demo");

    let snapshot = client
        .set_session_model(&target, "sess-1", &model(), Some(3), Some(false))
        .await
        .unwrap();
    assert_eq!(snapshot.session.session_id, "sess-1");

    let params = log
        .frame_params("session/setModel")
        .expect("必须记录到请求帧");
    assert_eq!(params["sessionId"], "sess-1");
    assert_eq!(params["model"]["providerId"], "zai");
    assert_eq!(params["expectedRevision"], 3);
    assert_eq!(params["persistAsWorkspaceLastUsed"], false);
}

#[tokio::test]
async fn thought_level_and_mode_omit_absent_options() {
    let log = FrameLog::new("thought-mode");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);
    let target = AgentWorkspaceTarget::new("/ws/demo");

    client
        .set_session_thought_level(&target, "sess-1", None, None, None)
        .await
        .unwrap();
    let params = log
        .frame_params("session/setThoughtLevel")
        .expect("必须记录到请求帧");
    assert_eq!(params["sessionId"], "sess-1");
    assert!(
        params.get("thoughtLevel").is_none(),
        "thoughtLevel 缺省时不能出现键"
    );
    assert!(params.get("expectedRevision").is_none());
    assert!(params.get("persistAsWorkspaceLastUsed").is_none());

    client
        .set_session_mode(&target, "sess-1", SessionMode::Plan, Some(7))
        .await
        .unwrap();
    let params = log
        .frame_params("session/setMode")
        .expect("必须记录到请求帧");
    assert_eq!(params["mode"], "plan");
    assert_eq!(params["expectedRevision"], 7);
}

#[tokio::test]
async fn close_sends_persistence_and_defaults_closed_to_true() {
    let log = FrameLog::new("close");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);
    let target = AgentWorkspaceTarget::new("/ws/demo");

    let closed = client
        .close_session(&target, "sess-1", Some(SessionPersistence::Deferred))
        .await
        .unwrap();
    assert!(closed);

    let params = log.frame_params("session/close").expect("必须记录到请求帧");
    assert_eq!(params["sessionId"], "sess-1");
    assert_eq!(params["expectedPersistence"], "deferred");
}

#[tokio::test]
async fn malformed_write_results_are_rejected() {
    let log = FrameLog::new("bad-setter");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-session-setter", &log, spawns);
    let target = AgentWorkspaceTarget::new("/ws/demo");
    let error = client
        .set_session_model(&target, "sess-1", &model(), None, None)
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid session/setModel result"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let log = FrameLog::new("bad-close");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-session-close", &log, spawns);
    let error = client
        .close_session(&target, "sess-1", None)
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => {
            assert!(
                message.contains("invalid session/close result"),
                "unexpected message: {message}"
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

fn compact_params() -> SessionCompactParams {
    SessionCompactParams {
        session_id: "sess-1".to_string(),
        input_id: None,
        instructions: Some("summarize".to_string()),
        expected_revision: None,
    }
}

fn goal_params() -> SessionGoalParams {
    SessionGoalParams {
        session_id: "sess-1".to_string(),
        input_id: None,
        action: SessionGoalAction::Set,
        objective: Some("ship it".to_string()),
        expected_revision: Some(3),
    }
}

#[tokio::test]
async fn compact_and_goal_send_only_present_fields_and_parse_results() {
    let log = FrameLog::new("compact-goal");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));

    let compact = client
        .compact_session(&AgentWorkspaceTarget::new("/ws/demo"), compact_params())
        .await
        .unwrap();
    assert_eq!(compact.response, "compacted");
    assert_eq!(compact.snapshot.session.session_id, "sess-1");
    assert_eq!(
        compact.compact.as_ref().map(|info| info.state),
        Some(agent_protocol::session_write::SessionCompactState::Accepted)
    );
    assert_eq!(
        log.frame_params("session/compact"),
        Some(json!({ "sessionId": "sess-1", "instructions": "summarize" }))
    );

    let goal = client
        .goal_session(&AgentWorkspaceTarget::new("/ws/demo"), goal_params())
        .await
        .unwrap();
    assert_eq!(goal.response, "goal");
    assert_eq!(goal.started_turn, Some(true));
    assert_eq!(
        log.frame_params("session/goal"),
        Some(json!({
            "sessionId": "sess-1",
            "action": "set",
            "objective": "ship it",
            "expectedRevision": 3
        }))
    );
}

#[tokio::test]
async fn compact_and_goal_reject_unknown_result_fields() {
    let log = FrameLog::new("compact-goal-bad");
    let spawns = Arc::new(AtomicUsize::new(0));

    for (mode, label) in [
        ("bad-session-compact", "compact"),
        ("bad-session-goal", "goal"),
    ] {
        let client = control(mode, &log, Arc::clone(&spawns));
        let error = if label == "compact" {
            match client
                .compact_session(&AgentWorkspaceTarget::new("/ws/demo"), compact_params())
                .await
            {
                Ok(_) => panic!("compact 未知字段必须拒绝"),
                Err(error) => error,
            }
        } else {
            match client
                .goal_session(&AgentWorkspaceTarget::new("/ws/demo"), goal_params())
                .await
            {
                Ok(_) => panic!("goal 未知字段必须拒绝"),
                Err(error) => error,
            }
        };
        assert!(matches!(error, ControlError::InvalidResult(_)), "{error:?}");
    }
}

fn resume_params() -> SessionResumeParams {
    SessionResumeParams {
        session_id: "sess-1".to_string(),
        workspace: Some(WorkspaceRef {
            workspace_path: "/ws/demo".to_string(),
            workspace_key: "/ws/demo".to_string(),
            workspace_identity: None,
            remote_session_id: None,
            extra: Default::default(),
        }),
        thought_level: Some("high".to_string()),
        mcp_servers: None,
        tool_allowlist: Some(vec!["Read".to_string()]),
        tool_denylist: None,
        off_peak_tool_enabled: Some(true),
        dynamic_workflow_enabled: None,
    }
}

#[tokio::test]
async fn resume_sends_full_params_and_parses_snapshot() {
    let log = FrameLog::new("resume");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));
    let snapshot = client
        .resume_session(&AgentWorkspaceTarget::new("/ws/demo"), resume_params())
        .await
        .unwrap();
    assert_eq!(snapshot.session.session_id, "sess-1");
    assert_eq!(
        log.frame_params("session/resume"),
        Some(json!({
            "sessionId": "sess-1",
            "workspace": { "workspacePath": "/ws/demo", "workspaceKey": "/ws/demo" },
            "thoughtLevel": "high",
            "toolAllowlist": ["Read"],
            "offPeakToolEnabled": true
        }))
    );
}

#[tokio::test]
async fn resume_retries_once_without_unrecognized_optional_fields() {
    let log = FrameLog::new("resume-compat");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("old-session-resume", &log, Arc::clone(&spawns));
    client
        .resume_session(&AgentWorkspaceTarget::new("/ws/demo"), resume_params())
        .await
        .unwrap();
    let frames = log.frame_params_all("session/resume");
    assert_eq!(frames.len(), 2, "必须恰好重试一次");
    assert_eq!(frames[0]["toolAllowlist"], json!(["Read"]));
    assert_eq!(frames[0]["offPeakToolEnabled"], json!(true));
    assert!(frames[1].get("toolAllowlist").is_none());
    assert!(frames[1].get("offPeakToolEnabled").is_none());
    assert_eq!(frames[1]["sessionId"], json!("sess-1"));
}

#[tokio::test]
async fn resume_does_not_retry_for_non_optional_unrecognized_key() {
    let log = FrameLog::new("resume-bad");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("bad-session-resume", &log, spawns);
    let error = match client
        .resume_session(&AgentWorkspaceTarget::new("/ws/demo"), resume_params())
        .await
    {
        Ok(_) => panic!("非可选字段不得触发兼容重试"),
        Err(error) => error,
    };
    assert!(
        matches!(
            error,
            ControlError::Client(mode_protocol_client::ClientError::Server { code: -32602, .. })
        ),
        "{error:?}"
    );
    assert_eq!(log.frame_params_all("session/resume").len(), 1);
}
