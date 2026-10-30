//! v4 命令面的最小子集：`v4/command` 的 `resolveInteraction`。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/command.ts` 的 `commandEnvelopeSchema` /
//! `commandAckSchema` / `commandPayloadSchemas.resolveInteraction`，以及
//! `packages/services/src/mode-agent/modeV4HostCommand.ts` 的 ACK 收口语义。
//!
//! `result` 在 ACK 里是「按命令类型」的判别联合；本镜像只发送 `resolveInteraction`，
//! 该命令不产生 result，所以这里保持 `Value` 并注明范围。规范见
//! `docs/specs/session-write-group.md` §「v4 命令最小闭环」。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

pub const V4_COMMAND_METHOD: &str = "v4/command";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolveInteractionAction {
    #[serde(rename = "accept")]
    Accept,
    #[serde(rename = "decline")]
    Decline,
    #[serde(rename = "cancel")]
    Cancel,
}

/// `resolveInteraction.payload.answer`。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveInteractionAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<ResolveInteractionAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveInteractionPayload {
    pub interaction_id: String,
    pub answer: ResolveInteractionAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolveInteractionType {
    #[serde(rename = "resolveInteraction")]
    ResolveInteraction,
}

/// `commandEnvelopeSchema` 的 `resolveInteraction` 形态。
///
/// `sessionId` 在协议里可空（createSession 为 null）；本命令必须携带会话 id。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveInteractionEnvelope {
    pub command_id: String,
    pub client_id: String,
    pub session_id: String,
    #[serde(rename = "type")]
    pub command_type: ResolveInteractionType,
    pub payload: ResolveInteractionPayload,
    /// 客户端时钟（毫秒），仅遥测。
    pub issued_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandAckStatus {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "rejected")]
    Rejected,
    #[serde(rename = "stale")]
    Stale,
    #[serde(rename = "duplicate")]
    Duplicate,
    #[serde(rename = "noop")]
    Noop,
    #[serde(rename = "failed")]
    Failed,
}

impl CommandAckStatus {
    /// TS `assertV4CommandAckOk`：accepted / duplicate / noop 视为幂等成功。
    pub fn is_success(self) -> bool {
        matches!(self, Self::Accepted | Self::Duplicate | Self::Noop)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Stale => "stale",
            Self::Duplicate => "duplicate",
            Self::Noop => "noop",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TtftExcluded {
    #[serde(rename = "capacity")]
    Capacity,
}

/// `commandAckSchema`；`result` 保持 `Value`（见模块注释）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandAck {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_excluded: Option<TtftExcluded>,
    pub command_id: String,
    pub status: CommandAckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// `z.number()`：允许小数，保留原样。
    pub revision_at_decision: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}
