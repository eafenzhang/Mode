//! Account Provider Config 交付：payload、revision 去重与回执校验。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent_process::{
    AccountProviderConfigSnapshot, AgentControlClient, AgentProcessSpec,
    AgentRuntimeRegistry, ControlError, ProviderReadFuture, ProviderSource, ProviderSyncStatus,
    SpecFactory,
};
use serde_json::{json, Value};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-provider-sync-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn frames(&self) -> Vec<String> {
        std::fs::read_to_string(&self.0)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("frame\t"))
            .map(str::to_string)
            .collect()
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct FakeSource {
    revision: Mutex<String>,
    based_on: String,
    providers: BTreeMap<String, Value>,
    states: BTreeMap<String, Value>,
}

impl FakeSource {
    fn new(revision: &str) -> Self {
        Self {
            revision: Mutex::new(revision.to_string()),
            based_on: "builtin-rev-1".into(),
            providers: BTreeMap::from([
                ("provider-a".to_string(), json!({ "access": { "entitled": true } })),
                ("provider-b".to_string(), json!({ "enabled": false })),
            ]),
            states: BTreeMap::from([("provider-a".to_string(), json!({ "connected": true }))]),
        }
    }

    fn set_revision(&self, revision: &str) {
        *self.revision.lock().unwrap() = revision.to_string();
    }
}

impl ProviderSource for FakeSource {
    fn read(&self) -> ProviderReadFuture<'_> {
        let revision = self.revision.lock().unwrap().clone();
        let based_on = self.based_on.clone();
        let providers = self.providers.clone();
        let states = self.states.clone();
        Box::pin(async move {
            Ok(AccountProviderConfigSnapshot {
                revision,
                based_on_mode_builtin_revision: based_on,
                providers,
                states,
            })
        })
    }
}

fn control(
    mode: &str,
    log: &FrameLog,
    spawns: Arc<AtomicUsize>,
) -> AgentControlClient {
    let mode = mode.to_string();
    let log_path = log.0.to_string_lossy().into_owned();
    let factory: SpecFactory = Arc::new(move |_target| {
        spawns.fetch_add(1, Ordering::SeqCst);
        let mut spec = AgentProcessSpec::new(FIXTURE);
        if !mode.is_empty() {
            spec.args.push(mode.clone());
        }
        spec.env
            .push(("AGENT_FIXTURE_FRAME_LOG".to_string(), log_path.clone()));
        Ok(spec)
    });
    AgentControlClient::new(Arc::new(AgentRuntimeRegistry::new(factory)))
}

#[tokio::test]
async fn syncs_payload_and_dedupes_by_revision() {
    let log = FrameLog::new("dedupe");
    let spawns = Arc::new(AtomicUsize::new(0));
    let control = control("", &log, Arc::clone(&spawns));
    let source = FakeSource::new("rev-1");
    let target = agent_process::AgentWorkspaceTarget::new("D:/ws/provider");

    let first = control
        .sync_account_provider_config(&target, Some(&source))
        .await
        .unwrap()
        .expect("有源必须返回结果");
    assert_eq!(first.delivered_revision, "rev-1");
    assert_eq!(first.provider_count, 2);
    assert_eq!(first.status, ProviderSyncStatus::Received);
    assert_eq!(spawns.load(Ordering::SeqCst), 1);

    let frames = log.frames();
    assert_eq!(frames.len(), 1, "只应发送一次");
    let mut parts = frames[0].splitn(3, '\t');
    assert_eq!(parts.next(), Some("frame"));
    assert_eq!(parts.next(), Some("provider/updateAccountConfig"));
    let params: Value = serde_json::from_str(parts.next().unwrap()).unwrap();
    assert_eq!(params["revision"], "rev-1");
    assert_eq!(params["basedOnModeBuiltinRevision"], "builtin-rev-1");
    assert_eq!(params["providers"]["provider-a"]["access"]["entitled"], true);
    assert_eq!(params["providers"]["provider-b"]["enabled"], false);
    assert_eq!(params["states"]["provider-a"]["connected"], true);

    let duplicate = control
        .sync_account_provider_config(&target, Some(&source))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(duplicate.status, ProviderSyncStatus::Unchanged);
    assert_eq!(log.frames().len(), 1, "同 revision 不得重复发送");

    source.set_revision("rev-2");
    let updated = control
        .sync_account_provider_config(&target, Some(&source))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.delivered_revision, "rev-2");
    assert_eq!(updated.status, ProviderSyncStatus::Received);
    assert_eq!(log.frames().len(), 2);
}

#[tokio::test]
async fn missing_source_is_a_noop() {
    let log = FrameLog::new("no-source");
    let spawns = Arc::new(AtomicUsize::new(0));
    let control = control("", &log, Arc::clone(&spawns));
    let target = agent_process::AgentWorkspaceTarget::new("D:/ws/no-source");
    let outcome = control
        .sync_account_provider_config(&target, None)
        .await
        .unwrap();
    assert!(outcome.is_none());
    assert_eq!(spawns.load(Ordering::SeqCst), 0, "无源不应拉起 Agent");
}

#[tokio::test]
async fn mismatched_receipt_fails_without_marking_delivered() {
    let log = FrameLog::new("mismatch");
    let spawns = Arc::new(AtomicUsize::new(0));
    let control = control("bad-provider-revision", &log, Arc::clone(&spawns));
    let source = FakeSource::new("rev-1");
    let target = agent_process::AgentWorkspaceTarget::new("D:/ws/mismatch");

    let error = control
        .sync_account_provider_config(&target, Some(&source))
        .await
        .unwrap_err();
    assert!(matches!(error, ControlError::ProviderSync(_)));
    assert_eq!(
        error.to_string(),
        "Account Config 接收回执版本与交付版本不一致"
    );
    // 未记录已交付，下一次仍会重试。
    let _ = control
        .sync_account_provider_config(&target, Some(&source))
        .await;
    assert_eq!(log.frames().len(), 2);
}
