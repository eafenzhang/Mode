//! Account Provider Config 交付（对应 TS `syncAccountProviderConfigToClient`）。
//!
//! 只负责协议侧：读取注入的 `ProviderSource` 快照 → 按 client 串行 → revision 去重 →
//! 发送 `provider/updateAccountConfig` → 校验回执版本。真实的账号 Provider 事实源属于
//! model-provider 域；未注入源时与 TS 的 `if (!accountProviderConfigSource) return`
//! 一致，不发送任何请求。

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

use mode_protocol_client::ProtocolClient;

use crate::ControlError;

pub const PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD: &str = "provider/updateAccountConfig";

pub type ProviderReadFuture<'a> =
    Pin<Box<dyn Future<Output = Result<AccountProviderConfigSnapshot, String>> + Send + 'a>>;

/// 账号 Provider 的运行时快照（`providers` 已是待发送的 JSON 对象）。
#[derive(Clone)]
pub struct AccountProviderConfigSnapshot {
    pub revision: String,
    pub based_on_mode_builtin_revision: String,
    pub providers: BTreeMap<String, Value>,
    pub states: BTreeMap<String, Value>,
}

pub trait ProviderSource: Send + Sync {
    fn read(&self) -> ProviderReadFuture<'_>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSyncStatus {
    Received,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSyncOutcome {
    pub delivered_revision: String,
    pub provider_count: u64,
    pub status: ProviderSyncStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateAccountConfigWire {
    received_revision: String,
    provider_count: u64,
    status: ProviderSyncStatus,
}

/// 按 workspace key 串行并记录已交付 revision；client 与 workspace 一一对应。
#[derive(Default, Clone)]
pub struct ProviderConfigSync {
    delivered: Arc<Mutex<BTreeMap<String, String>>>,
    locks: Arc<Mutex<BTreeMap<String, Arc<AsyncMutex<()>>>>>,
}

impl ProviderConfigSync {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn sync(
        &self,
        workspace_key: &str,
        client: &ProtocolClient,
        source: &dyn ProviderSource,
    ) -> Result<ProviderSyncOutcome, ControlError> {
        let gate = {
            let mut locks = self.locks.lock().expect("provider sync locks poisoned");
            Arc::clone(
                locks
                    .entry(workspace_key.to_string())
                    .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
            )
        };
        let _guard = gate.lock().await;

        let snapshot = source.read().await.map_err(ControlError::ProviderSync)?;
        if self
            .delivered
            .lock()
            .expect("provider sync state poisoned")
            .get(workspace_key)
            .is_some_and(|revision| revision == &snapshot.revision)
        {
            return Ok(ProviderSyncOutcome {
                delivered_revision: snapshot.revision,
                provider_count: snapshot.providers.len() as u64,
                status: ProviderSyncStatus::Unchanged,
            });
        }

        let providers: serde_json::Map<String, Value> = snapshot
            .providers
            .iter()
            .map(|(id, value)| (id.clone(), value.clone()))
            .collect();
        let states: serde_json::Map<String, Value> = snapshot
            .states
            .iter()
            .map(|(id, value)| (id.clone(), value.clone()))
            .collect();
        let response = client
            .request(
                PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD,
                Some(json!({
                    "revision": snapshot.revision,
                    "basedOnModeBuiltinRevision": snapshot.based_on_mode_builtin_revision,
                    "providers": providers,
                    "states": states,
                })),
            )
            .await?;
        let wire: UpdateAccountConfigWire = serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!(
                "invalid provider/updateAccountConfig result: {error}"
            ))
        })?;
        if wire.received_revision != snapshot.revision {
            return Err(ControlError::ProviderSync(
                "Account Config 接收回执版本与交付版本不一致".into(),
            ));
        }
        self.delivered
            .lock()
            .expect("provider sync state poisoned")
            .insert(workspace_key.to_string(), snapshot.revision.clone());
        Ok(ProviderSyncOutcome {
            delivered_revision: snapshot.revision,
            provider_count: wire.provider_count,
            status: wire.status,
        })
    }
}

/// 空账号快照（TS `EMPTY_ACCOUNT_PROVIDER_CONFIG_SNAPSHOT` 同值）。
pub fn empty_account_provider_config_snapshot() -> AccountProviderConfigSnapshot {
    AccountProviderConfigSnapshot {
        revision: "empty-account-config-v1".to_string(),
        based_on_mode_builtin_revision: "uninitialized".to_string(),
        providers: BTreeMap::new(),
        states: BTreeMap::new(),
    }
}

type AccountSourceListener = Arc<dyn Fn(&str) + Send + Sync>;
type AccountSourceListeners = Arc<Mutex<Vec<AccountSourceListener>>>;

/// 进程外围适配器更新的账号 Provider Overlay（TS `MutableAccountProviderConfigSource`）。
///
/// 只保存账号状态投影出的第三层 Overlay：Token、API Key、Header 与账号身份不得写入。
/// `replace` 以 revision 做幂等门禁（同 revision 返回 false 且不通知监听者）。
pub struct MutableAccountProviderConfigSource {
    snapshot: Mutex<AccountProviderConfigSnapshot>,
    listeners: AccountSourceListeners,
}

impl Default for MutableAccountProviderConfigSource {
    fn default() -> Self {
        Self::new()
    }
}

impl MutableAccountProviderConfigSource {
    pub fn new() -> Self {
        Self {
            snapshot: Mutex::new(empty_account_provider_config_snapshot()),
            listeners: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// TS `replace(snapshot, reason)`：revision 相同直接返回 false。
    pub fn replace(&self, snapshot: AccountProviderConfigSnapshot, reason: &str) -> bool {
        {
            let mut current = self.snapshot.lock().expect("account snapshot poisoned");
            if snapshot.revision == current.revision {
                return false;
            }
            *current = snapshot;
        }
        let listeners = self
            .listeners
            .lock()
            .expect("account snapshot listeners poisoned")
            .clone();
        for listener in listeners {
            listener(reason);
        }
        true
    }

    pub fn on_did_change(&self, listener: AccountSourceListener) -> Box<dyn Fn() + Send + Sync> {
        self.listeners
            .lock()
            .expect("account snapshot listeners poisoned")
            .push(Arc::clone(&listener));
        let listeners = Arc::clone(&self.listeners);
        Box::new(move || {
            let mut guard = listeners
                .lock()
                .expect("account snapshot listeners poisoned");
            guard.retain(|candidate| !Arc::ptr_eq(candidate, &listener));
        })
    }
}

impl ProviderSource for MutableAccountProviderConfigSource {
    fn read(&self) -> ProviderReadFuture<'_> {
        let snapshot = self
            .snapshot
            .lock()
            .expect("account snapshot poisoned")
            .clone();
        Box::pin(async move { Ok(snapshot) })
    }
}

#[cfg(test)]
mod mutable_source_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn snapshot(revision: &str) -> AccountProviderConfigSnapshot {
        AccountProviderConfigSnapshot {
            revision: revision.to_string(),
            based_on_mode_builtin_revision: "builtin-1".to_string(),
            providers: BTreeMap::new(),
            states: BTreeMap::new(),
        }
    }

    #[test]
    fn initial_read_is_empty_account_snapshot() {
        let source = MutableAccountProviderConfigSource::new();
        let snapshot = futures_lite_block(source.read());
        assert_eq!(snapshot.revision, "empty-account-config-v1");
        assert_eq!(snapshot.based_on_mode_builtin_revision, "uninitialized");
        assert!(snapshot.providers.is_empty());
    }

    fn futures_lite_block(future: ProviderReadFuture<'_>) -> AccountProviderConfigSnapshot {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(future)
            .expect("read")
    }

    #[test]
    fn replace_is_revision_gated_and_notifies() {
        let source = MutableAccountProviderConfigSource::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let reasons = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let calls_for_listener = Arc::clone(&calls);
        let reasons_for_listener = Arc::clone(&reasons);
        let _unsubscribe = source.on_did_change(Arc::new(move |reason| {
            calls_for_listener.fetch_add(1, Ordering::SeqCst);
            reasons_for_listener
                .lock()
                .unwrap()
                .push(reason.to_string());
        }));

        assert!(source.replace(snapshot("rev-2"), "refresh"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(reasons.lock().unwrap().as_slice(), ["refresh"]);

        // 同 revision：返回 false 且不再通知。
        assert!(!source.replace(snapshot("rev-2"), "duplicate"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn unsubscribe_stops_notifications() {
        let source = MutableAccountProviderConfigSource::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_listener = Arc::clone(&calls);
        let unsubscribe = source.on_did_change(Arc::new(move |_| {
            calls_for_listener.fetch_add(1, Ordering::SeqCst);
        }));
        unsubscribe();
        assert!(source.replace(snapshot("rev-2"), "refresh"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
