//! 与 TS `packages/shared/src/command-types.ts` + `settings-source.ts` 对齐的线上类型。
//!
//! 字段名一律 camelCase（RPC 直接把这些结构序列化给渲染层）；可选字段为 `None` 时
//! **不出现**在 JSON 里，与 TS 的 `undefined` 语义一致（不能写成 `null`）。

use serde::{Deserialize, Serialize};

/// 目前只有 `modeAgent`（`MODE_COMMAND_AGENT_SOURCES` 也只有它）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandAgentSource {
    #[default]
    ModeAgent,
}

/// `SettingsDirectorySource`；`claude` 只保留类型兼容，不参与命令发现。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsDirectorySource {
    Mode,
    Agents,
    Claude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsDirectoryScope {
    User,
    Project,
}

/// 命令自身的 scope：`global`（用户目录）或 `project`（工作区目录）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandScope {
    Global,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandSource {
    User,
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandStorageLevel {
    User,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDirectoryLocation {
    pub source: SettingsDirectorySource,
    pub scope: SettingsDirectoryScope,
    pub directory_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserCommand {
    pub name: String,
    pub prompt: String,
    pub content: String,
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    pub id: String,
    pub source: CommandSource,
    pub agent_source: CommandAgentSource,
    pub location: SettingsDirectoryLocation,
    pub enabled: bool,
    pub scope: CommandScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommand {
    pub name: String,
    pub prompt: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    pub id: String,
    pub source: CommandSource,
    pub enabled: bool,
    pub plugin_name: String,
    pub plugin_marketplace: String,
    pub plugin_enabled: bool,
    pub scope: CommandScope,
    pub file_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModeCommand {
    User(UserCommand),
    Plugin(PluginCommand),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandsCapability {
    pub user_scope_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandsListResult {
    pub commands: Vec<ModeCommand>,
    pub user_commands: Vec<UserCommand>,
    pub plugin_commands: Vec<PluginCommand>,
    pub capability: CommandsCapability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandConfig {
    pub name: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListParams {
    pub agent_source: Option<CommandAgentSource>,
    pub workspace_path: Option<String>,
    /// TS 接收但从不使用；保留字段以维持参数形状。
    pub workspace_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandCreateParams {
    pub config: CommandConfig,
    #[serde(default)]
    pub agent_source: Option<CommandAgentSource>,
    #[serde(default)]
    pub storage_level: Option<CommandStorageLevel>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandUpdateParams {
    /// TS 类型必填但服务从不读取；这里可选并忽略。
    #[serde(default)]
    pub command_id: Option<String>,
    pub config: CommandConfig,
    #[serde(default)]
    pub agent_source: Option<CommandAgentSource>,
    #[serde(default)]
    pub old_file_path: Option<String>,
    #[serde(default)]
    pub storage_level: Option<CommandStorageLevel>,
    #[serde(default)]
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDeleteParams {
    #[serde(default)]
    pub command_id: Option<String>,
    pub file_path: String,
    #[serde(default)]
    pub agent_source: Option<CommandAgentSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSetEnabledParams {
    #[serde(default)]
    pub command_id: Option<String>,
    pub file_path: String,
    pub enabled: bool,
    #[serde(default)]
    pub agent_source: Option<CommandAgentSource>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrimaryDirectoryParams {
    pub agent_source: Option<CommandAgentSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimaryDirectoryResult {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteCommandResult {
    pub command: UserCommand,
}
