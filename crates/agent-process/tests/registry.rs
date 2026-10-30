//! Agent 运行时注册表：同一 workspace 单进程、并发去重、失效重建、空闲回收。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentProcessError, AgentProcessSpec, AgentRuntimeRegistry, SpecFactory, WorkspaceTarget,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

fn registry(counter: Arc<AtomicUsize>) -> AgentRuntimeRegistry {
    registry_with_mode("", counter)
}

fn registry_with_mode(mode: &str, counter: Arc<AtomicUsize>) -> AgentRuntimeRegistry {
    let mode = mode.to_string();
    let factory: SpecFactory = Arc::new(move |_target| {
        counter.fetch_add(1, Ordering::SeqCst);
        let mut spec = AgentProcessSpec::new(FIXTURE);
        if !mode.is_empty() {
            spec.args.push(mode.clone());
        }
        Ok(spec)
    });
    AgentRuntimeRegistry::new(factory)
}

fn target(path: &str) -> WorkspaceTarget {
    WorkspaceTarget::new(path)
}

#[tokio::test]
async fn same_key_reuses_one_process() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = registry(Arc::clone(&spawns));
    let first = registry.get_or_spawn(&target("D:/ws/a")).await.unwrap();
    let second = registry.get_or_spawn(&target("D:/ws/a")).await.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
    assert_eq!(registry.len(), 1);
    registry.shutdown_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn identity_wins_over_path_for_the_key() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = registry(Arc::clone(&spawns));
    let local = target("D:/ws/a");
    let remote = target("D:/ws/a").with_identity("ssh://host/D:/ws/a");
    assert_eq!(local.key(), "D:/ws/a");
    assert_eq!(remote.key(), "ssh://host/D:/ws/a");

    let first = registry.get_or_spawn(&local).await.unwrap();
    let second = registry.get_or_spawn(&remote).await.unwrap();
    assert!(!Arc::ptr_eq(&first, &second), "不同 identity 必须是不同进程");
    assert_eq!(spawns.load(Ordering::SeqCst), 2);
    registry.shutdown_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn concurrent_get_or_spawn_only_spawns_once() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry(Arc::clone(&spawns)));
    let a = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move { registry.get_or_spawn(&target("D:/ws/c")).await }
    });
    let b = tokio::spawn({
        let registry = Arc::clone(&registry);
        async move { registry.get_or_spawn(&target("D:/ws/c")).await }
    });
    let first = a.await.unwrap().unwrap();
    let second = b.await.unwrap().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
    registry.shutdown_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn closed_transport_is_replaced_on_next_call() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = registry(Arc::clone(&spawns));
    let first = registry.get_or_spawn(&target("D:/ws/d")).await.unwrap();
    first.client().dispose();
    let second = registry.get_or_spawn(&target("D:/ws/d")).await.unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(spawns.load(Ordering::SeqCst), 2);
    registry.shutdown_all(Duration::from_secs(3)).await;
}

#[tokio::test]
async fn invalidate_and_reap_idle_remove_entries() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = registry(Arc::clone(&spawns));
    registry.get_or_spawn(&target("D:/ws/e")).await.unwrap();
    registry
        .invalidate("D:/ws/e", Duration::from_secs(3))
        .await;
    assert_eq!(registry.len(), 0);

    registry.get_or_spawn(&target("D:/ws/f")).await.unwrap();
    let reaped = registry
        .reap_idle(Duration::ZERO, Duration::from_secs(3))
        .await;
    assert_eq!(reaped, 1);
    assert_eq!(registry.len(), 0);
}

#[tokio::test]
async fn reap_idle_skips_process_while_request_is_in_flight() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = registry_with_mode("slow-reply", Arc::clone(&spawns));
    let process = registry.get_or_spawn(&target("D:/ws/i")).await.unwrap();

    let client = Arc::clone(&process);
    let request = tokio::spawn(async move { client.client().request("slow", None).await });
    for _ in 0..200 {
        if process.client().pending_count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(process.client().pending_count(), 1, "请求应在飞");

    // 在飞期间即使阈值为 0 也不能回收。
    assert_eq!(registry.reap_idle(Duration::ZERO, Duration::from_secs(3)).await, 0);
    assert_eq!(registry.len(), 1);

    request.await.unwrap().unwrap();
    assert_eq!(process.client().pending_count(), 0);
    // 请求结束后计时归零；阈值为 0 时下一轮回收。
    assert_eq!(registry.reap_idle(Duration::ZERO, Duration::from_secs(3)).await, 1);
    assert_eq!(registry.len(), 0);
}

#[tokio::test]
async fn idle_reaper_task_reclaims_unused_process() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(registry(Arc::clone(&spawns)));
    registry.get_or_spawn(&target("D:/ws/j")).await.unwrap();

    let reaper = registry.spawn_idle_reaper(
        Duration::from_millis(20),
        Duration::from_millis(10),
        Duration::from_secs(3),
    );
    for _ in 0..200 {
        if registry.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    reaper.abort();
    assert_eq!(registry.len(), 0, "空闲进程应被周期任务回收");
}

#[tokio::test]
async fn spawn_failure_is_not_cached() {
    let error_factory: SpecFactory = Arc::new(|_| {
        Err(AgentProcessError::Spawn {
            command: "nope".into(),
            message: "boom".into(),
        })
    });
    let registry = AgentRuntimeRegistry::new(error_factory);
    assert!(registry.get_or_spawn(&target("D:/ws/g")).await.is_err());
    assert_eq!(registry.len(), 0);
}

#[tokio::test]
async fn shutdown_all_closes_every_process() {
    let spawns = Arc::new(AtomicUsize::new(0));
    let registry = registry(Arc::clone(&spawns));
    let first = registry.get_or_spawn(&target("D:/ws/h1")).await.unwrap();
    let second = registry.get_or_spawn(&target("D:/ws/h2")).await.unwrap();
    registry.shutdown_all(Duration::from_secs(3)).await;
    assert_eq!(registry.len(), 0);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while (!first.client().is_closed() || !second.client().is_closed())
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(first.client().is_closed());
    assert!(second.client().is_closed());
}
