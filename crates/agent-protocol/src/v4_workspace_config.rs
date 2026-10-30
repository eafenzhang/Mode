//! v4 `workspace-config` topic：workspace 级配置目录与 slash 命令目录。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/workspace-config.ts`。规范见
//! `docs/specs/session-write-group.md` §「workspace-config topic」。
//!
//! 全部 schema 都未加 `.strict()`：未知键按 zod strip 语义忽略。订阅复用
//! `v4/conversation/subscribe`（topic = `workspace-config/<workspaceId>`），
//! 因此本模块只有帧载荷类型，没有新的 RPC 方法。

use serde::{Deserialize, Serialize};

/// `workspaceConfigSelectValueSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceConfigSelectValue {
    pub value: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<SelectValueOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_thought_levels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_default_thought_level: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectValueOrigin {
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "injected")]
    Injected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceConfigOptionType {
    #[serde(rename = "select")]
    Select,
    #[serde(rename = "boolean")]
    Boolean,
}

/// `currentValue` 是 `string | boolean` 的联合。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceConfigValue {
    Text(String),
    Switch(bool),
}

/// `workspaceConfigOptionSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceConfigOption {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(rename = "type")]
    pub option_type: WorkspaceConfigOptionType,
    pub current_value: WorkspaceConfigValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<WorkspaceConfigSelectValue>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlashCommandSource {
    #[serde(rename = "builtin")]
    Builtin,
    #[serde(rename = "custom")]
    Custom,
}

/// `workspaceSlashCommandSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSlashCommand {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SlashCommandSource>,
}

/// topic 载荷本体：整体替换语义（conflated 最新态，绝不深合并）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceConfigState {
    pub config_options: Vec<WorkspaceConfigOption>,
    pub slash_commands: Vec<WorkspaceSlashCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigUpdatedOp {
    #[serde(rename = "config.updated")]
    ConfigUpdated,
}

/// `workspaceConfigSnapshotSchema`：`protocolVersion` 固定为 1。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceConfigSnapshot {
    pub protocol_version: u64,
    pub workspace_id: String,
    pub log_epoch: String,
    pub config: WorkspaceConfigState,
}

impl WorkspaceConfigSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version != 1 {
            return Err(format!(
                "workspace-config protocolVersion must be 1, got {}",
                self.protocol_version
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpdatedDelta {
    pub op: ConfigUpdatedOp,
    pub config: WorkspaceConfigState,
}

/// `workspaceConfigDeltaSchema`：封闭集合只有一个 op（整体替换态，不做字段级增量）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceConfigDelta {
    ConfigUpdated(ConfigUpdatedDelta),
}
