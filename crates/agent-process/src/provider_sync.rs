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
#[derive(Default)]
pub struct ProviderConfigSync {
    delivered: Mutex<BTreeMap<String, String>>,
    locks: Mutex<BTreeMap<String, Arc<AsyncMutex<()>>>>,
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

        let snapshot = source
            .read()
            .await
            .map_err(ControlError::ProviderSync)?;
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
        let wire: UpdateAccountConfigWire = serde_json::from_value(response)
            .map_err(|error| ControlError::InvalidResult(error.to_string()))?;
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
