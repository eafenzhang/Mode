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

// ── topic frame / wire frame ──
//
// 真源：`transport.ts` 的 `workspaceConfigTopicFrameSchema` /
// `workspaceConfigTopicWireFrameSchema`。三个生产 topic（conversation /
// sessions-index / workspace-config）共用同一帧外壳与 notification，只有 payload 类型不同。

use serde_json::Number;

use crate::v4_transport::{
    FragmentWireFrame, FramePayloadKindDeltas, FramePayloadKindSnapshot, WireDeliveryKind,
    WireFrameKindComplete,
};

pub const WORKSPACE_CONFIG_TOPIC_PREFIX: &str = "workspace-config/";

/// `workspaceConfigTopic(workspaceId)`。
pub fn workspace_config_topic(workspace_id: &str) -> String {
    format!("{WORKSPACE_CONFIG_TOPIC_PREFIX}{workspace_id}")
}

/// `parseWorkspaceConfigTopic`：前缀 + 非空 workspaceId。
pub fn parse_workspace_config_topic(topic: &str) -> Option<&str> {
    let workspace_id = topic.strip_prefix(WORKSPACE_CONFIG_TOPIC_PREFIX)?;
    (!workspace_id.is_empty()).then_some(workspace_id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceConfigSnapshotPayload {
    pub kind: FramePayloadKindSnapshot,
    pub snapshot: WorkspaceConfigSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceConfigDeltasPayload {
    pub kind: FramePayloadKindDeltas,
    pub deltas: Vec<WorkspaceConfigDelta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceConfigFramePayload {
    Snapshot(Box<WorkspaceConfigSnapshotPayload>),
    Deltas(WorkspaceConfigDeltasPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceConfigTopicFrame {
    pub topic: String,
    pub subscription_id: String,
    pub from_seq: Number,
    pub to_seq: Number,
    pub sent_at: Number,
    pub payload: WorkspaceConfigFramePayload,
}

impl WorkspaceConfigTopicFrame {
    pub fn validate(&self) -> Result<(), String> {
        if parse_workspace_config_topic(&self.topic).is_none() {
            return Err("invalid workspace-config topic".to_string());
        }
        if let WorkspaceConfigFramePayload::Snapshot(payload) = &self.payload {
            payload.snapshot.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceConfigCompleteWireFrame {
    pub wire_version: u64,
    pub kind: WireFrameKindComplete,
    pub delivery_kind: WireDeliveryKind,
    pub logical_frame_id: String,
    pub logical_frame_ordinal: u64,
    pub topic: String,
    pub subscription_id: String,
    pub frame: WorkspaceConfigTopicFrame,
}

impl WorkspaceConfigCompleteWireFrame {
    pub fn validate(&self) -> Result<(), String> {
        self.frame.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceConfigWireFrame {
    Complete(Box<WorkspaceConfigCompleteWireFrame>),
    Fragment(FragmentWireFrame),
}

impl WorkspaceConfigWireFrame {
    pub fn subscription_id(&self) -> &str {
        match self {
            Self::Complete(frame) => &frame.subscription_id,
            Self::Fragment(frame) => &frame.subscription_id,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let topic = match self {
            Self::Complete(frame) => {
                frame.validate()?;
                &frame.topic
            }
            Self::Fragment(frame) => &frame.topic,
        };
        if parse_workspace_config_topic(topic).is_none() {
            return Err("invalid workspace-config topic".to_string());
        }
        Ok(())
    }
}
