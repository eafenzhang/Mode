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

// ── 插件写侧（配置/启用/校验/恢复内置）──

pub const PLUGINS_SET_ENABLED_METHOD: &str = "plugins/setEnabled";
pub const PLUGINS_RESTORE_BUILTIN_METHOD: &str = "plugins/restoreBuiltin";
pub const PLUGINS_CONFIGURE_METHOD: &str = "plugins/configure";
pub const PLUGINS_RESET_CONFIG_METHOD: &str = "plugins/resetConfig";
pub const PLUGINS_VALIDATE_METHOD: &str = "plugins/validate";

/// `modePluginsSetEnabledParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsSetEnabledParams {
    pub workspace: SessionWorkspaceRef,
    pub plugin_id: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<PluginScope>,
}

/// `modePluginsSetEnabledResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsSetEnabledResult {
    pub plugin: PluginInfo,
    pub enabled: bool,
}

/// `modePluginsRestoreBuiltinParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsRestoreBuiltinParams {
    pub workspace: SessionWorkspaceRef,
    pub plugin_id: String,
}

/// `modePluginsRestoreBuiltinResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsRestoreBuiltinResult {
    pub plugin_id: String,
    pub diagnostics: Vec<PluginDiagnostic>,
}

/// `modePluginsConfigureParamsSchema`：`options` 是任意 JSON 对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsConfigureParams {
    pub workspace: SessionWorkspaceRef,
    pub plugin_id: String,
    pub options: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_option_keys: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<PluginScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
}

/// `modePluginsConfigureResultSchema`（resetConfig 复用同一结果）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsConfigureResult {
    pub plugin_id: String,
    pub diagnostics: Vec<PluginDiagnostic>,
}

/// `modePluginsResetConfigParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsResetConfigParams {
    pub workspace: SessionWorkspaceRef,
    pub plugin_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<PluginScope>,
}

/// `modePluginsValidateParamsSchema`：三者至少给一个由调用方保证，schema 不强制。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsValidateParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginCompatibility {
    pub runnable: Vec<String>,
    pub diagnostic_only: Vec<String>,
    pub unsupported: Vec<String>,
}

/// `modePluginsValidateResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsValidateResult {
    pub ok: bool,
    pub diagnostics: Vec<PluginDiagnostic>,
    pub compatibility: PluginCompatibility,
}

// ── 对话引用 catalog 与建议引用解析 ──

pub const PLUGINS_REFERENCE_CATALOG_METHOD: &str = "plugins/referenceCatalog";
pub const PLUGINS_REFERENCE_CATALOG_WITH_CATEGORY_METHOD: &str =
    "plugins/referenceCatalogWithCategory";
pub const PLUGINS_RESOLVE_SUGGESTED_REFERENCE_METHOD: &str = "plugins/resolveSuggestedReference";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginReferenceCatalogParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// `modePluginReferenceCatalogEntrySchema`：identifiers-only + 展示字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginReferenceCatalogEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub plugin_id: String,
    pub name: String,
    pub marketplace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name_i18n: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_i18n: Option<BTreeMap<String, String>>,
    pub enabled: bool,
    pub conflicting_plugin_ids: Vec<String>,
    pub skill_qualified_names: Vec<String>,
    pub mcp_server_names: Vec<String>,
    /// 旧 Host 不投影该字段时按空数组兼容。
    #[serde(default)]
    pub subagent_names: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginReferenceAuthority {
    #[serde(rename = "session")]
    Session,
    #[serde(rename = "workspace")]
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsReferenceCatalogResult {
    pub authority: PluginReferenceAuthority,
    pub plugins: Vec<PluginReferenceCatalogEntry>,
}

/// `modePluginSuggestedReferenceStatusSchema`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginSuggestedReferenceStatus {
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "missing")]
    Missing,
    #[serde(rename = "conflict")]
    Conflict,
    #[serde(rename = "unavailable")]
    Unavailable,
}

/// `modeDeliveryKindSchema`（不是 v4 的 wire delivery kind）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginDeliveryKind {
    #[serde(rename = "desktop-continuous")]
    DesktopContinuous,
    #[serde(rename = "web-remote-replayable")]
    WebRemoteReplayable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsResolveSuggestedReferenceParams {
    pub workspace: SessionWorkspaceRef,
    pub stable_id: String,
    pub operation_id: String,
    pub client_mode: PluginDeliveryKind,
    pub delivery_kind: PluginDeliveryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceTrustOfficial {
    #[serde(rename = "official")]
    Official,
}

/// `modePluginsResolveSuggestedReferenceResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsResolveSuggestedReferenceResult {
    pub stable_id: String,
    pub status: PluginSuggestedReferenceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_trust: Option<SourceTrustOfficial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<PluginStoreListing>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

impl PluginsResolveSuggestedReferenceResult {
    /// 协议 `superRefine`：actionable 状态必须带可信安装身份。
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(
            self.status,
            PluginSuggestedReferenceStatus::Ready
                | PluginSuggestedReferenceStatus::Disabled
                | PluginSuggestedReferenceStatus::Missing
        ) {
            return Ok(());
        }
        if self.marketplace.as_deref().is_none_or(str::is_empty)
            || self.plugin_name.as_deref().is_none_or(str::is_empty)
            || self.source_trust != Some(SourceTrustOfficial::Official)
        {
            return Err(
                "actionable suggested Plugin results require trusted install identity".to_string(),
            );
        }
        Ok(())
    }
}

// ── 市场与安装写侧 ──

pub const PLUGINS_MARKETPLACE_ADD_METHOD: &str = "plugins/marketplace/add";
pub const PLUGINS_MARKETPLACE_REMOVE_METHOD: &str = "plugins/marketplace/remove";
pub const PLUGINS_MARKETPLACE_UPDATE_METHOD: &str = "plugins/marketplace/update";
pub const PLUGINS_INSTALL_METHOD: &str = "plugins/install";
pub const PLUGINS_CANCEL_OPERATION_METHOD: &str = "plugins/cancelOperation";
pub const PLUGINS_UNINSTALL_METHOD: &str = "plugins/uninstall";
pub const PLUGINS_UPDATE_METHOD: &str = "plugins/update";
pub const PLUGIN_OPERATION_PROGRESS_NOTIFICATION: &str = "plugins/operationProgress";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsMarketplaceAddParams {
    pub workspace: SessionWorkspaceRef,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsMarketplaceRemoveParams {
    pub workspace: SessionWorkspaceRef,
    pub marketplace: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsMarketplaceUpdateParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

/// `modePluginsMarketplaceMutationResultSchema`：三组字段都可选。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsMarketplaceMutationResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<PluginMarketplaceSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplaces: Option<Vec<PluginMarketplaceSummary>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<PluginDiagnostic>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsInstallParams {
    pub workspace: SessionWorkspaceRef,
    pub plugin_name: String,
    pub marketplace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<PluginScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsCancelOperationParams {
    pub operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsCancelOperationResult {
    pub operation_id: String,
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsUninstallParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_cache: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsInstallResult {
    pub installed_plugins: Vec<InstalledPluginSummary>,
    pub dependency_closure: Vec<String>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsUninstallResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_plugin: Option<InstalledPluginSummary>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginsUpdateParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marketplace: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginOperationProgressState {
    #[serde(rename = "refreshing")]
    Refreshing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationProgressNotification {
    pub operation_id: String,
    pub state: PluginOperationProgressState,
}
