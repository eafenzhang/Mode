//! 插件只读面：`plugins/list` 与 `plugins/describe` 的严格镜像。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的 `modePluginInfoSchema` /
//! `modePluginComponentGroupSchema` / `modePluginsDescribeResultSchema`。规范见
//! `docs/specs/session-write-group.md` §「插件只读面」。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::session_read::SessionWorkspaceRef;
use crate::workflow::LiteralTrue;

pub const PLUGINS_LIST_METHOD: &str = "plugins/list";
pub const PLUGINS_DESCRIBE_METHOD: &str = "plugins/describe";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginScope {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "workspace")]
    Workspace,
}

/// `modePluginOptionValueSchema`：string | number | boolean。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginOptionValue {
    Text(String),
    Number(Number),
    Boolean(bool),
}

/// `modePluginUserConfigOptionSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUserConfigOption {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<PluginOptionValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub option_type: Option<PluginUserConfigOptionType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginUserConfigOptionType {
    #[serde(rename = "string")]
    String,
    #[serde(rename = "number")]
    Number,
    #[serde(rename = "boolean")]
    Boolean,
    #[serde(rename = "directory")]
    Directory,
    #[serde(rename = "file")]
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginHookType {
    #[serde(rename = "command")]
    Command,
    #[serde(rename = "process")]
    Process,
}

/// `shell: z.union([z.literal(true), z.string()])`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginHookShell {
    Enabled(LiteralTrue),
    Command(String),
}

/// `modePluginHookDetailSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHookDetail {
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    #[serde(rename = "type")]
    pub hook_type: PluginHookType,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(rename = "async", default, skip_serializing_if = "Option::is_none")]
    pub is_async: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<PluginHookShell>,
    /// `z.number().positive()`：允许小数，保留原样。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    pub source_path: String,
    pub runnable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginComponentKind {
    #[serde(rename = "agent")]
    Agent,
    #[serde(rename = "command")]
    Command,
    #[serde(rename = "skill")]
    Skill,
    #[serde(rename = "hook")]
    Hook,
    #[serde(rename = "mcp")]
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginComponentItem {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginComponentGroup {
    pub kind: PluginComponentKind,
    pub items: Vec<PluginComponentItem>,
}

/// `modePluginInfoSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub enabled: bool,
    pub source: String,
    pub marketplace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_count: Option<u64>,
    pub skill_root_count: u64,
    pub command_root_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<PluginComponentGroup>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_mcp_server_names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_mcp_server_names: Option<Vec<String>>,
    pub mcp_server_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_details: Option<Vec<PluginHookDetail>>,
    pub root_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_config: Option<BTreeMap<String, PluginUserConfigOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_options: Option<BTreeMap<String, PluginOptionValue>>,
    /// 缺省表示 package 可用；`missing` 表示已声明但目标 Host 尚未物化。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_status: Option<PluginPackageStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_source: Option<PluginScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_source: Option<PluginScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_sources: Option<BTreeMap<String, PluginScope>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginPackageStatus {
    #[serde(rename = "missing")]
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginDiagnosticSeverity {
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "error")]
    Error,
}

/// `modePluginDiagnosticSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDiagnostic {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<PluginDiagnosticSeverity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
}

/// `modePluginsListParamsSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsListParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_scope: Option<PluginScope>,
}

/// `modePluginsListResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsListResult {
    pub plugins: Vec<PluginInfo>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

/// `modePluginsDescribeParamsSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsDescribeParams {
    pub workspace: SessionWorkspaceRef,
    pub marketplace: String,
    pub plugin_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDescribeMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `modePluginsDescribeResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsDescribeResult {
    pub components: Vec<PluginComponentGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<PluginDiagnostic>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<PluginDescribeMetadata>,
}

pub const PLUGINS_OVERVIEW_METHOD: &str = "plugins/overview";

/// `modePluginStoreListingSchema`（全部可选）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginStoreListing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name_i18n: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_i18n: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_of_service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hero_image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example_prompts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example_prompts_i18n: Option<BTreeMap<String, Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_paid_plan: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarketplaceRefreshFailure {
    pub code: String,
    pub failed_at: String,
    pub message: String,
}

/// `modePluginMarketplaceSummarySchema`；`source` 是开放 JSON 对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginMarketplaceSummary {
    pub id: String,
    pub name: String,
    pub source: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
    pub plugin_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_official: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub featured: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_failure: Option<MarketplaceRefreshFailure>,
}

/// `modeAvailablePluginSummarySchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailablePluginSummary {
    pub id: String,
    pub name: String,
    pub marketplace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_types: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<PluginStoreListing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundled_unavailable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_unavailable: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginUpdateStatus {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "update-available")]
    UpdateAvailable,
    #[serde(rename = "version-changed")]
    VersionChanged,
}

/// `modeInstalledPluginSummarySchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledPluginSummary {
    pub id: String,
    pub name: String,
    pub marketplace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub enabled: bool,
    pub scope: PluginScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_types: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_details: Option<Vec<PluginHookDetail>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_status: Option<PluginUpdateStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<PluginStoreListing>,
}

/// `modePluginsOverviewParamsSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsOverviewParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_scope: Option<PluginScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOverviewCapability {
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `modePluginsOverviewResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsOverviewResult {
    pub marketplaces: Vec<PluginMarketplaceSummary>,
    pub available_plugins: Vec<AvailablePluginSummary>,
    pub installed_plugins: Vec<InstalledPluginSummary>,
    pub restorable_builtins: Vec<AvailablePluginSummary>,
    pub diagnostics: Vec<PluginDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_marketplace_enabled: Option<bool>,
    pub capability: PluginOverviewCapability,
}
