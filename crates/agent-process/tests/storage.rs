//! 存储启动门：ready 等待、失败传播、快照与进程代次。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentProcessSpec, AgentRuntimeRegistry, SpecFactory, StorageStartupPhase, WorkspaceTarget,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

fn registry(mode: &str, supports_storage: bool) -> (AgentRuntimeRegistry, Arc<AtomicUsize>) {
    let spawns = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&spawns);
    let mode = mode.to_string();
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

fn target(path: &str) -> WorkspaceTarget {
    WorkspaceTarget::new(path)
}

#[tokio::test]
async fn prepare_waits_for_ready_and_exposes_snapshot() {
    let (registry, _spawns) = registry("storage-ready", true);
    let target = target("D:/ws/storage-ready");
    assert!(registry.storage_startup_state(&target).is_none(), "未启动时无快照");

    registry.prepare_storage(&target).await.expect("ready 必须放行");

    let snapshot = registry
        .storage_startup_state(&target)
        .expect("prepare 后必须有快照");
    assert_eq!(snapshot.generation, 1);
    let state = snapshot.state.expect("必须收到状态");
    assert_eq!(state.phase, StorageStartupPhase::Ready);
    assert_eq!(state.sequence, 2);

    let receiver = registry
        .subscribe_storage_startup(&target)
        .expect("活进程必须可订阅");
    let current = receiver
        .borrow()
        .as_ref()
        .cloned()
        .expect("订阅应看到最新状态");
    assert_eq!(current.phase, StorageStartupPhase::Ready);

    registry.shutdown_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn failed_state_rejects_prepare_with_ts_message() {
    let (registry, _spawns) = registry("storage-failed", true);
    let target = target("D:/ws/storage-failed");
    let error = registry.prepare_storage(&target).await.unwrap_err();
    assert_eq!(error, "SQLite startup failed: sql_failed");
    registry.shutdown_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn non_storage_entry_does_not_wait() {
    let (registry, spawns) = registry("", false);
    let target = target("D:/ws/plain");
    registry.prepare_storage(&target).await.expect("非 storage 入口立即返回");
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
    let snapshot = registry.storage_startup_state(&target).expect("进程存在");
    assert!(snapshot.state.is_none(), "没有通知时状态为 None");
    registry.shutdown_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn generation_increments_after_restart() {
    let (registry, spawns) = registry("storage-ready", true);
    let target = target("D:/ws/gen");
    registry.prepare_storage(&target).await.unwrap();
    let first = registry.storage_startup_state(&target).unwrap().generation;
    registry.invalidate(&target.key(), Duration::from_secs(3)).await;
    assert!(registry.storage_startup_state(&target).is_none());
    registry.prepare_storage(&target).await.unwrap();
    let second = registry.storage_startup_state(&target).unwrap().generation;
    assert!(second > first, "重建必须产生新代次：{first} -> {second}");
    assert_eq!(spawns.load(Ordering::SeqCst), 2);
    registry.shutdown_all(Duration::from_secs(3)).await;
}
