//! plugin-sync 领域类型（`@mode/shared` plugin-sync 的投影）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginSyncComponentType {
    #[serde(rename = "skills")]
    Skills,
    #[serde(rename = "commands")]
    Commands,
    #[serde(rename = "hooks")]
    Hooks,
    #[serde(rename = "mcp")]
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSyncCandidate {
    pub id: String,
    pub name: String,
    pub plugin_id: String,
    pub directory_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub path: String,
    pub size_bytes: u64,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_override: Option<bool>,
    pub component_types: Vec<PluginSyncComponentType>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSyncCandidateListResult {
    pub candidates: Vec<PluginSyncCandidate>,
    pub max_archive_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginSyncRemoteStatusReason {
    #[serde(rename = "samePluginId")]
    SamePluginId,
    #[serde(rename = "targetExists")]
    TargetExists,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSyncRemoteStatus {
    pub plugin_id: String,
    pub directory_name: String,
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<PluginSyncRemoteStatusReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSyncRemoteStatusResult {
    pub statuses: Vec<PluginSyncRemoteStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSyncWriteAccessResult {
    pub ok: bool,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSyncArchivePlugin {
    pub id: String,
    pub name: String,
    pub plugin_id: String,
    pub directory_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSyncArchiveExportResult {
    pub archive: Vec<u8>,
    pub archive_bytes: u64,
    pub plugins: Vec<PluginSyncArchivePlugin>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginSyncImportStatus {
    #[serde(rename = "synced")]
    Synced,
    #[serde(rename = "skipped")]
    Skipped,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSyncImportResultItem {
    pub name: String,
    pub plugin_id: String,
    pub directory_name: String,
    pub status: PluginSyncImportStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSyncImportResult {
    pub results: Vec<PluginSyncImportResultItem>,
}
