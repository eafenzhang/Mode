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

// ── commands/query ──
//
// 真源：`commandsQueryParamsSchema` / `commandQueryItemSchema` /
// `commandsQueryResultSchema`（`packages/shared/src/mode-protocol-v4/command.ts`）。
// 该方法是重连后的按 `commandId` 对账读面，不产生写副作用。
// `sessionId` 在 key 里是「必填但可为 null」（createSession/全局幂等桶），
// 所以用新类型区分「缺失」与 `null`；serde 对裸 `Option` 会把缺失也当成 None。

pub const V4_COMMANDS_QUERY_METHOD: &str = "v4/commands/query";
pub const COMMANDS_QUERY_MIN: usize = 1;
pub const COMMANDS_QUERY_MAX: usize = 64;
pub const COMMAND_KEY_MAX_ID_LEN: usize = 128;

/// `commandKeySchema.sessionId`：字段必须存在，值可为 null。
///
/// 手写 `CommandKey` 的 `Deserialize`：serde 的 missing-field 路径会把
/// `Option` 的「缺失」也当成 None，而协议里 `sessionId` 是 required-nullable，
/// 两者必须区分。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct CommandSessionId(pub Option<String>);

impl<'de> Deserialize<'de> for CommandSessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Option::<String>::deserialize(deserializer).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandKey {
    pub session_id: CommandSessionId,
    pub command_id: String,
}

impl<'de> Deserialize<'de> for CommandKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "camelCase")]
        enum Field {
            SessionId,
            CommandId,
        }

        struct KeyVisitor;

        impl<'de> serde::de::Visitor<'de> for KeyVisitor {
            type Value = CommandKey;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a command key with sessionId and commandId")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut session_id: Option<CommandSessionId> = None;
                let mut command_id: Option<String> = None;
                while let Some(field) = map.next_key::<Field>()? {
                    match field {
                        Field::SessionId => {
                            if session_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("sessionId"));
                            }
                            session_id = Some(map.next_value()?);
                        }
                        Field::CommandId => {
                            if command_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("commandId"));
                            }
                            command_id = Some(map.next_value()?);
                        }
                    }
                }
                Ok(CommandKey {
                    session_id: session_id
                        .ok_or_else(|| serde::de::Error::missing_field("sessionId"))?,
                    command_id: command_id
                        .ok_or_else(|| serde::de::Error::missing_field("commandId"))?,
                })
            }
        }

        deserializer.deserialize_map(KeyVisitor)
    }
}

impl CommandKey {
    pub fn global(command_id: impl Into<String>) -> Self {
        Self {
            session_id: CommandSessionId(None),
            command_id: command_id.into(),
        }
    }

    pub fn session(session_id: impl Into<String>, command_id: impl Into<String>) -> Self {
        Self {
            session_id: CommandSessionId(Some(session_id.into())),
            command_id: command_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandsQueryParams {
    pub commands: Vec<CommandKey>,
    /// 协议里是 `z.literal(true)`；`false` 由 `validate` 拒绝，缺省省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<bool>,
}

impl CommandsQueryParams {
    /// 协议的两条 `superRefine`：命令条数 1..=64、clock 探针不能查询会话命令。
    pub fn validate(&self) -> Result<(), String> {
        validate_command_count(self.commands.len())?;
        if self.clock == Some(false) {
            return Err("clock must be true when present".to_string());
        }
        if self.clock == Some(true) && self.commands.iter().any(|key| key.session_id.0.is_some()) {
            return Err("clock probes cannot query session commands".to_string());
        }
        Ok(())
    }
}

fn validate_command_count(count: usize) -> Result<(), String> {
    if !(COMMANDS_QUERY_MIN..=COMMANDS_QUERY_MAX).contains(&count) {
        return Err(format!(
            "commands must contain {COMMANDS_QUERY_MIN}..={COMMANDS_QUERY_MAX} keys, got {count}"
        ));
    }
    Ok(())
}

/// `localTtftClockSchema`：仅 `commands/query` 的时钟探针返回。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalTtftClock {
    pub instance_id: String,
    pub received_at: Number,
    pub sent_at: Number,
}

impl LocalTtftClock {
    fn validate(&self) -> Result<(), String> {
        if self.instance_id.is_empty() || self.instance_id.len() > COMMAND_KEY_MAX_ID_LEN {
            return Err("clock instanceId must be 1..=128 chars".to_string());
        }
        if !is_finite_non_negative(&self.received_at) || !is_finite_non_negative(&self.sent_at) {
            return Err("clock times must be finite and non-negative".to_string());
        }
        Ok(())
    }
}

fn is_finite_non_negative(value: &Number) -> bool {
    value
        .as_f64()
        .is_some_and(|number| number.is_finite() && number >= 0.0)
}

/// `commandQueryItemSchema.result` 的 `"unknown"` 分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnknownCommand {
    #[serde(rename = "unknown")]
    Unknown,
}

/// `commandQueryItemSchema.result`：ACK 或 `"unknown"`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommandQueryOutcome {
    Ack(Box<CommandAck>),
    Unknown(UnknownCommand),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandQueryItem {
    pub key: CommandKey,
    pub result: CommandQueryOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandsQueryResult {
    pub results: Vec<CommandQueryItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<LocalTtftClock>,
}

impl CommandsQueryResult {
    pub fn validate(&self) -> Result<(), String> {
        validate_command_count(self.results.len())?;
        if let Some(clock) = &self.clock {
            clock.validate()?;
        }
        Ok(())
    }
}
