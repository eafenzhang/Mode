//! 镜像服务：身份、两个发布入口的 origin 校验、读取。

use std::sync::{Arc, Mutex};

use device_mid::DeviceMidStore;

use crate::model::{bound_snapshot, validate_snapshot, Snapshot};
use crate::store::ConversationMirrorStore;

pub const MAX_ORIGIN_LABEL_LENGTH: usize = 64;
pub const FALLBACK_DEVICE_LABEL: &str = "Mode";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorIdentity {
    pub device_id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishResult {
    pub stored: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    pub record_count: usize,
}

impl PublishResult {
    fn rejected(reason: &'static str) -> Self {
        Self {
            stored: false,
            reason: Some(reason),
            record_count: 0,
        }
    }
}

pub struct ConversationMirrorService {
    store: ConversationMirrorStore,
    device_mid: DeviceMidStore,
    label: String,
    cached_identity: Mutex<Option<MirrorIdentity>>,
}

impl ConversationMirrorService {
    pub fn new(data_root: impl Into<std::path::PathBuf>) -> Arc<Self> {
        let data_root = data_root.into();
        Arc::new(Self {
            store: ConversationMirrorStore::new(&data_root),
            device_mid: DeviceMidStore::new(&data_root),
            label: resolve_device_label(),
            cached_identity: Mutex::new(None),
        })
    }

    pub fn get_mirror_identity(&self) -> Result<MirrorIdentity, String> {
        if let Some(cached) = self
            .cached_identity
            .lock()
            .map_err(|_| "identity cache poisoned".to_string())?
            .clone()
        {
            return Ok(cached);
        }
        let device_id = self
            .device_mid
            .ensure()
            .map_err(|error| format!("device identity unavailable: {error}"))?;
        let identity = MirrorIdentity {
            device_id,
            label: self.label.clone(),
        };
        *self
            .cached_identity
            .lock()
            .map_err(|_| "identity cache poisoned".to_string())? = Some(identity.clone());
        Ok(identity)
    }

    /// 对端发布通道：必须是**别人**的快照（origin 等于本机的属于回声）。
    pub fn publish_snapshot(&self, snapshot: &serde_json::Value) -> PublishResult {
        self.write_with_origin_check(snapshot, false)
    }

    /// 本机自快照通道：必须是**自己**的快照（对端不能用它伪造本机记录）。
    pub fn write_local_snapshot(&self, snapshot: &serde_json::Value) -> PublishResult {
        self.write_with_origin_check(snapshot, true)
    }

    pub fn read_workspace_snapshots(&self, workspace_path: &str) -> Vec<Snapshot> {
        self.store.read_workspace_snapshots(workspace_path)
    }

    fn write_with_origin_check(
        &self,
        snapshot: &serde_json::Value,
        expect_self: bool,
    ) -> PublishResult {
        let Ok(parsed) = validate_snapshot(snapshot) else {
            return PublishResult::rejected("invalid-record");
        };
        let Ok(identity) = self.get_mirror_identity() else {
            return PublishResult::rejected("invalid-record");
        };
        let is_self = parsed.origin_device_id == identity.device_id;
        if expect_self != is_self {
            return PublishResult::rejected(if expect_self {
                "foreign-origin"
            } else {
                "self-origin"
            });
        }
        let bounded = bound_snapshot(&parsed);
        if self.store.write_snapshot(&bounded).is_err() {
            return PublishResult::rejected("invalid-record");
        }
        PublishResult {
            stored: true,
            reason: Some("stored"),
            record_count: bounded.records.len(),
        }
    }
}

/// 展示名只用主机名（Windows `COMPUTERNAME`，其余 `HOSTNAME`），最多 64 个字符。
fn resolve_device_label() -> String {
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return FALLBACK_DEVICE_LABEL.to_string();
    }
    trimmed.chars().take(MAX_ORIGIN_LABEL_LENGTH).collect()
}
