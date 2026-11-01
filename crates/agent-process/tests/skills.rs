//! 技能引用 catalog：请求形状、结果解析与 disabled 拒绝。

use std::path::PathBuf;
use std::sync::Arc;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    SpecFactory,
};
use agent_protocol::skills::SkillReferenceAuthority;
use serde_json::{json, Value};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-skills-{label}-{}-{}.log",
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

fn control(mode: &str, log: &FrameLog) -> AgentControlClient {
    let mode = mode.to_string();
    let log = log.0.to_string_lossy().into_owned();
    let factory: SpecFactory = Arc::new(move |_target| {
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
async fn skill_reference_catalog_sends_session_id_and_parses_entries() {
    let log = FrameLog::new("ok");
    let client = control("", &log);
    let catalog = client
        .skill_reference_catalog(&AgentWorkspaceTarget::new("/ws/demo"), Some("sess-1"))
        .await
        .unwrap();
    assert_eq!(catalog.authority, SkillReferenceAuthority::Workspace);
    assert_eq!(catalog.skills.len(), 1);
    assert_eq!(catalog.skills[0].id, "demo");
    assert!(catalog.skills[0].enabled);
    assert_eq!(
        log.frame_params("skills/referenceCatalog"),
        Some(json!({
            "workspace": { "workspacePath": "/ws/demo", "workspaceKey": "/ws/demo" },
            "sessionId": "sess-1"
        }))
    );

    let without_session = client
        .skill_reference_catalog(&AgentWorkspaceTarget::new("/ws/demo"), None)
        .await
        .unwrap();
    assert_eq!(without_session.skills.len(), 1);
}

#[tokio::test]
async fn disabled_skill_entry_is_rejected() {
    let log = FrameLog::new("disabled");
    let client = control("disabled-skill-catalog", &log);
    let error = match client
        .skill_reference_catalog(&AgentWorkspaceTarget::new("/ws/demo"), None)
        .await
    {
        Ok(_) => panic!("disabled 技能必须拒绝"),
        Err(error) => error,
    };
    assert!(matches!(error, ControlError::InvalidResult(_)), "{error:?}");
}
