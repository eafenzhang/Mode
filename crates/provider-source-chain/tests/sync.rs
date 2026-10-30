//! 端到端：链路快照 → `provider/updateAccountConfig` 线格式与 revision 去重。

mod support;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget,
    ProviderSyncStatus, SpecFactory,
};
use provider_source_chain::AgentProviderSource;
use serde_json::Value;
use support::{Fixture, FakeOptions};

const FIXTURE: &str = env!("CARGO_BIN_EXE_chain-agent-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "provider-chain-sync-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn lines(&self) -> Vec<String> {
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                if text.contains("frame\tprovider/updateAccountConfig") {
                    return text.lines().map(str::to_string).collect();
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Vec::new()
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn registry(log: &FrameLog, spawns: Arc<AtomicUsize>) -> AgentRuntimeRegistry {
    let log_path = log.0.to_string_lossy().into_owned();
    let factory: SpecFactory = Arc::new(move |_target| {
        spawns.fetch_add(1, Ordering::SeqCst);
        let mut spec = AgentProcessSpec::new(FIXTURE);
        spec.env
            .push(("AGENT_CHAIN_FRAME_LOG".to_string(), log_path.clone()));
        Ok(spec)
    });
    AgentRuntimeRegistry::new(factory)
}

#[tokio::test]
async fn delivers_chain_snapshot_and_dedupes_revision() {
    let fixture = Fixture::new("sync");
    let log = FrameLog::new("sync");
    let spawns = Arc::new(AtomicUsize::new(0));
    let control = AgentControlClient::new(Arc::new(registry(&log, Arc::clone(&spawns))));
    let source = AgentProviderSource::new(
        fixture.bundled.clone(),
        None,
        fixture.personal.clone(),
        FakeOptions::ready(),
    );
    let target = AgentWorkspaceTarget::new("D:/ws/chain");

    let first = control
        .sync_account_provider_config(&target, Some(&source))
        .await
        .unwrap()
        .expect("注入源时应返回结果");
    assert_eq!(first.status, ProviderSyncStatus::Received);
    assert_eq!(first.provider_count, 2);

    let lines = log.lines();
    assert_eq!(lines.len(), 1, "unexpected frames: {lines:?}");
    let payload = lines[0]
        .splitn(3, '\t')
        .nth(2)
        .expect("frame 行应含参数");
    let params: Value = serde_json::from_str(payload).unwrap();
    assert_eq!(params["revision"], first.delivered_revision);
    assert!(params["revision"]
        .as_str()
        .unwrap()
        .starts_with("account:["));
    assert_eq!(params["providers"].as_object().unwrap().len(), 2);
    assert_eq!(
        params["providers"]["zai-start"]["access"]["entitled"],
        true
    );
    assert_eq!(
        params["states"]["zai-individual"]["availability"],
        "available"
    );

    let again = control
        .sync_account_provider_config(&target, Some(&source))
        .await
        .unwrap()
        .expect("注入源时应返回结果");
    assert_eq!(again.status, ProviderSyncStatus::Unchanged);
    assert_eq!(again.delivered_revision, first.delivered_revision);
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
    assert_eq!(log.lines().len(), 1, "revision 未变时不应重复发送");
}
