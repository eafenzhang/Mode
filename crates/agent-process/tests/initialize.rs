//! `initialize`：可用性折叠与协议身份。

use std::sync::Arc;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, SpecFactory,
    MODE_PROTOCOL_NAME, MODE_PROTOCOL_VERSION,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

fn control_with(factory: SpecFactory) -> AgentControlClient {
    AgentControlClient::new(Arc::new(AgentRuntimeRegistry::new(factory)))
}

#[tokio::test]
async fn initialize_reports_available_with_protocol_identity() {
    let factory: SpecFactory = Arc::new(|_target| Ok(AgentProcessSpec::new(FIXTURE)));
    let client = control_with(factory);
    let result = client
        .initialize(&AgentWorkspaceTarget::new("/ws/init"))
        .await;
    assert!(result.available);
    assert_eq!(result.workspace_key, "/ws/init");
    assert_eq!(result.protocol_name, MODE_PROTOCOL_NAME);
    assert_eq!(result.protocol_version, MODE_PROTOCOL_VERSION);
    assert_eq!(result.transport_kind.as_deref(), Some("stdio"));
    assert!(result.reason.is_none());
}

#[tokio::test]
async fn initialize_folds_spawn_failure_into_unavailable_result() {
    let factory: SpecFactory =
        Arc::new(|_target| Ok(AgentProcessSpec::new("definitely-not-a-mode-agent-binary")));
    let client = control_with(factory);
    let result = client
        .initialize(&AgentWorkspaceTarget::new("/ws/missing"))
        .await;
    assert!(!result.available);
    assert_eq!(result.workspace_key, "/ws/missing");
    assert!(result.reason.is_some(), "失败必须带 reason");
    assert!(result.transport_kind.is_none());
}
