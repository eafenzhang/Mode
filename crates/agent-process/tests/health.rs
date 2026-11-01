//! 健康快照（W4 S0）：只读、不拉起、不改状态。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget,
    RuntimeHealthState, SpecFactory, WorkspaceTarget,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

fn make_registry(mode: &str, supports_storage: bool) -> (AgentRuntimeRegistry, Arc<AtomicUsize>) {
    let mode = mode.to_string();
    let spawns = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&spawns);
    let factory: SpecFactory = Arc::new(move |_target| {
        counter.fetch_add(1, Ordering::SeqCst);
        let mut spec = AgentProcessSpec::new(FIXTURE);
        if !mode.is_empty() {
            spec.args.push(mode.clone());
        }
        spec.supports_storage_startup = supports_storage;
        Ok(spec)
    });
    (AgentRuntimeRegistry::new(factory), spawns)
}

fn control(mode: &str) -> AgentControlClient {
    let log = std::env::temp_dir()
        .join(format!(
            "mode-health-{mode}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .to_string_lossy()
        .into_owned();
    let mode = mode.to_string();
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
async fn health_is_absent_until_spawned_then_ready() {
    let client = control("");
    let target = AgentWorkspaceTarget::new("/ws/health");
    assert!(client.runtime_health(&target).is_none());

    client
        .read_workspace_presentation(&target)
        .await
        .expect("spawn + read");

    let health = client.runtime_health(&target).expect("健康快照");
    assert_eq!(health.state, RuntimeHealthState::Ready);
    assert_eq!(health.workspace_key, "/ws/health");
    assert!(health.runtime_identity.starts_with("/ws/health:"));
    assert!(health.process_id.is_some());
    assert!(health.last_error.is_none());
    assert!(
        health.storage_state.is_none(),
        "非 storage 入口无 gate 状态"
    );
}

#[tokio::test]
async fn health_waits_for_storage_gate_and_reports_failure() {
    let (registry, _spawns) = make_registry("storage-ready", true);
    let target = WorkspaceTarget::new("/ws/storage-ready");
    registry.get_or_spawn(&target).await.expect("spawn");
    let ready = loop {
        let health = registry.health(&target).expect("条目存在");
        if health.state == RuntimeHealthState::Ready {
            break health;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        ready.storage_state.as_ref().map(|state| state.phase),
        Some(agent_process::StorageStartupPhase::Ready)
    );

    let (registry, _spawns) = make_registry("storage-failed", true);
    let failed_target = WorkspaceTarget::new("/ws/storage-failed");
    registry
        .get_or_spawn(&failed_target)
        .await
        .expect("spawn 本身成功");
    let failed = loop {
        let health = registry.health(&failed_target).expect("条目存在");
        if health.state == RuntimeHealthState::Dead {
            break health;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(failed.last_error.is_some(), "storage 失败必须带 last_error");
}

#[tokio::test]
async fn health_reports_dead_when_transport_closes() {
    let (registry, _spawns) = make_registry("exit-immediately", false);
    let target = WorkspaceTarget::new("/ws/dead");
    let _ = registry.get_or_spawn(&target).await;
    let dead = loop {
        match registry.health(&target) {
            Some(health) if health.state == RuntimeHealthState::Dead => break health,
            _ => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    };
    assert!(dead.process_id.is_some());
}
