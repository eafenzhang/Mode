//! `AccountProviderService` 编排语义：缓存、generation、superseded、LKG 与订阅。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use account_provider_overlay::AccountProviderResolveOutput;
use provider_config::{AccountProviderStates, JsonValue, ProviderConfigMap};
use provider_config_source::ProviderConfigSnapshot;
use provider_source_chain::{
    AccountProviderService, ConfigSourcePort, RefreshErrorEvent, ResolveRequest, ResolverPort,
    ServiceFuture, Unsubscribe,
};
use tokio::sync::Notify;
use tokio::time::{timeout, Duration};

type Listener = Arc<dyn Fn(&str) + Send + Sync>;

fn empty_output() -> AccountProviderResolveOutput {
    AccountProviderResolveOutput {
        providers: ProviderConfigMap::empty(),
        states: AccountProviderStates::default(),
    }
}

fn providers_with(provider_id: &str) -> ProviderConfigMap {
    let value = JsonValue::from_json_str(&format!(
        r#"[{{"providerId":"{provider_id}","config":{{}}}}]"#
    ))
    .unwrap();
    provider_config::parse_provider_config_map(&value, &[]).unwrap()
}

fn config_snapshot(revision: &str) -> ProviderConfigSnapshot {
    ProviderConfigSnapshot {
        revision: format!("[\"{revision}\"]"),
        mode_builtin_revision: revision.to_owned(),
        personal_revision: "personal".to_owned(),
        mode_builtin_providers: ProviderConfigMap::empty(),
        mode_builtin_provider_templates: Default::default(),
        personal_providers: ProviderConfigMap::empty(),
        mode_builtin_model_rules: Default::default(),
        personal_models: Default::default(),
        personal_provider_order: Vec::new(),
    }
}

struct FakeConfigSource {
    revision: Mutex<String>,
    reads: AtomicUsize,
    listeners: Arc<Mutex<Vec<Listener>>>,
}

impl FakeConfigSource {
    fn new(revision: &str) -> Arc<Self> {
        Arc::new(Self {
            revision: Mutex::new(revision.to_owned()),
            reads: AtomicUsize::new(0),
            listeners: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn set_revision(&self, revision: &str) {
        *self.revision.lock().unwrap() = revision.to_owned();
    }

    fn emit(&self, reason: &str) {
        let listeners = self.listeners.lock().unwrap().clone();
        for listener in listeners {
            listener(reason);
        }
    }
}

impl ConfigSourcePort for FakeConfigSource {
    fn read(&self) -> ServiceFuture<'_, Result<ProviderConfigSnapshot, String>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let revision = self.revision.lock().unwrap().clone();
        Box::pin(async move { Ok(config_snapshot(&revision)) })
    }

    fn subscribe(&self, listener: Listener) -> Unsubscribe {
        let listeners = Arc::clone(&self.listeners);
        listeners.lock().unwrap().push(Arc::clone(&listener));
        Box::new(move || {
            let mut listeners = listeners.lock().unwrap();
            if let Some(index) = listeners
                .iter()
                .position(|value| Arc::ptr_eq(value, &listener))
            {
                listeners.remove(index);
            }
        })
    }
}

type ResolveScript = Arc<
    dyn Fn(
            usize,
            ResolveRequest,
        ) -> ServiceFuture<'static, Result<AccountProviderResolveOutput, String>>
        + Send
        + Sync,
>;

struct ScriptedResolver {
    calls: AtomicUsize,
    script: ResolveScript,
}

impl ScriptedResolver {
    fn new(
        script: impl Fn(
                usize,
                ResolveRequest,
            ) -> ServiceFuture<'static, Result<AccountProviderResolveOutput, String>>
            + Send
            + Sync
            + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            script: Arc::new(script),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ResolverPort for ScriptedResolver {
    fn resolve(
        &self,
        request: ResolveRequest,
    ) -> ServiceFuture<'_, Result<AccountProviderResolveOutput, String>> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        (self.script)(index, request)
    }
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("等待条件超时");
}

#[tokio::test]
async fn reuses_cached_snapshot_and_first_read_is_single_round() {
    let config = FakeConfigSource::new("rev-1");
    let resolver = ScriptedResolver::new(|_, _| Box::pin(async { Ok(empty_output()) }));
    let service = AccountProviderService::new(config.clone(), resolver.clone());

    let first = service.read().await.unwrap();
    let second = service.read().await.unwrap();
    assert_eq!(first.revision, second.revision);
    assert_eq!(resolver.calls(), 1);
    // 一轮内两次读取（解析前/解析后复核）；缓存命中不再读配置。
    assert_eq!(config.reads(), 2);
    assert!(service.last_snapshot().is_some());
}

#[tokio::test]
async fn first_failure_publishes_fail_closed_snapshot() {
    let config = FakeConfigSource::new("rev-1");
    let resolver =
        ScriptedResolver::new(|_, _| Box::pin(async { Err("credential failed".to_owned()) }));
    let service = AccountProviderService::new(config.clone(), resolver.clone());

    let snapshot = service.read().await.unwrap();
    assert_eq!(snapshot.based_on_mode_builtin_revision, "rev-1");
    assert!(snapshot.providers.keys().is_empty());
    assert!(service.last_snapshot().is_some());
}

#[tokio::test]
async fn later_failure_keeps_last_known_good_and_reports_error() {
    let config = FakeConfigSource::new("rev-1");
    let resolver = ScriptedResolver::new(|index, _| {
        Box::pin(async move {
            if index == 0 {
                Ok(empty_output())
            } else {
                Err("account query failed".to_owned())
            }
        })
    });
    let service = AccountProviderService::new(config.clone(), resolver.clone());
    let events: Arc<Mutex<Vec<RefreshErrorEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let _unsubscribe = service.on_did_refresh_error(Arc::new(move |event| {
        sink.lock().unwrap().push(event.clone());
    }));

    let first = service.read().await.unwrap();
    let error = service.refresh("explicit").await.unwrap_err();
    assert_eq!(error, "account query failed");
    assert_eq!(
        service.last_snapshot().unwrap().revision,
        first.revision,
        "非首次失败必须保留 last-known-good"
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].error, "account query failed");
    assert_eq!(events[0].reasons, vec!["explicit".to_owned()]);
}

#[tokio::test]
async fn refresh_during_round_waits_for_next_round() {
    let config = FakeConfigSource::new("rev-1");
    let gate = Arc::new(Notify::new());
    let entered = Arc::new(Notify::new());
    let resolver = {
        let gate = Arc::clone(&gate);
        let entered = Arc::clone(&entered);
        ScriptedResolver::new(move |index, _| {
            let gate = Arc::clone(&gate);
            let entered = Arc::clone(&entered);
            Box::pin(async move {
                if index == 0 {
                    entered.notify_waiters();
                    gate.notified().await;
                }
                Ok(empty_output())
            })
        })
    };
    let service = AccountProviderService::new(config.clone(), resolver.clone());

    let reader = {
        let service = Arc::clone(&service);
        tokio::spawn(async move { service.read().await })
    };
    wait_until(|| resolver.calls() == 1).await;
    let refresh = {
        let service = Arc::clone(&service);
        tokio::spawn(async move { service.refresh("explicit").await })
    };
    // 让 refresh 完成登记（generation 提升），再放行第一轮。
    tokio::time::sleep(Duration::from_millis(20)).await;
    gate.notify_waiters();

    let first = reader.await.unwrap().unwrap();
    let second = refresh.await.unwrap().unwrap();
    assert_eq!(first.revision, second.revision);
    assert_eq!(resolver.calls(), 2, "开始后到达的请求必须再跑一轮");
}

#[tokio::test]
async fn config_change_during_resolution_supersedes_round() {
    let config = FakeConfigSource::new("rev-1");
    let gate = Arc::new(Notify::new());
    let entered = Arc::new(Notify::new());
    let resolver = {
        let gate = Arc::clone(&gate);
        let entered = Arc::clone(&entered);
        ScriptedResolver::new(move |index, _| {
            let gate = Arc::clone(&gate);
            let entered = Arc::clone(&entered);
            Box::pin(async move {
                if index == 0 {
                    entered.notify_waiters();
                    gate.notified().await;
                }
                Ok(empty_output())
            })
        })
    };
    let service = AccountProviderService::new(config.clone(), resolver.clone());

    let reader = {
        let service = Arc::clone(&service);
        tokio::spawn(async move { service.read().await })
    };
    wait_until(|| resolver.calls() == 1).await;
    config.set_revision("rev-2");
    gate.notify_waiters();

    let snapshot = reader.await.unwrap().unwrap();
    assert_eq!(snapshot.based_on_mode_builtin_revision, "rev-2");
    assert_eq!(resolver.calls(), 2, "revision 变化必须重跑一轮");
}

#[tokio::test]
async fn change_listener_emits_joined_reasons_only_when_revision_changes() {
    let config = FakeConfigSource::new("rev-1");
    let resolver = ScriptedResolver::new(|index, _| {
        Box::pin(async move {
            if index == 0 {
                Ok(empty_output())
            } else {
                Ok(AccountProviderResolveOutput {
                    providers: providers_with("p1"),
                    states: AccountProviderStates::default(),
                })
            }
        })
    });
    let service = AccountProviderService::new(config.clone(), resolver.clone());
    let reasons: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&reasons);
    let _unsubscribe = service.on_did_change(Arc::new(move |reason| {
        sink.lock().unwrap().push(reason.to_owned());
    }));

    service.read().await.unwrap();
    assert!(reasons.lock().unwrap().is_empty(), "首份快照不触发变更");

    service.refresh("explicit").await.unwrap();
    assert_eq!(reasons.lock().unwrap().as_slice(), ["explicit".to_owned()]);
}

#[tokio::test]
async fn config_source_notification_triggers_background_refresh() {
    let config = FakeConfigSource::new("rev-1");
    let resolver = ScriptedResolver::new(|_, _| Box::pin(async { Ok(empty_output()) }));
    let service = AccountProviderService::new(config.clone(), resolver.clone());

    service.read().await.unwrap();
    assert_eq!(resolver.calls(), 1);
    config.emit("poll-changed");
    wait_until(|| resolver.calls() == 2).await;
}

#[tokio::test]
async fn dispose_rejects_pending_refresh() {
    let config = FakeConfigSource::new("rev-1");
    let gate = Arc::new(Notify::new());
    let entered = Arc::new(Notify::new());
    let resolver = {
        let gate = Arc::clone(&gate);
        let entered = Arc::clone(&entered);
        ScriptedResolver::new(move |index, _| {
            let gate = Arc::clone(&gate);
            let entered = Arc::clone(&entered);
            Box::pin(async move {
                if index == 1 {
                    entered.notify_waiters();
                    gate.notified().await;
                }
                Ok(empty_output())
            })
        })
    };
    let service = AccountProviderService::new(config.clone(), resolver.clone());
    service.read().await.unwrap();
    let refresh = {
        let service = Arc::clone(&service);
        tokio::spawn(async move { service.refresh("explicit").await })
    };
    wait_until(|| resolver.calls() == 2).await;
    let _ = entered; // entered 由 resolver 触发，确保已进入阻塞轮。
    service.dispose();
    let result = refresh.await.unwrap();
    assert!(result.unwrap_err().contains("已 dispose"));
    gate.notify_waiters();
}
