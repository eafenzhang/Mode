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

// ── v4/command 通用信封 ──
//
// 真源：`packages/shared/src/mode-protocol-v4/command.ts` 的
// `commandEnvelopeSchema` / `commandTypeSchema` 与 `parseCommandEnvelope` 的组合校验。
// 逐命令 payload schema 不在本镜像范围内（共 34 个，CLI 仍是最终校验者）；这里只保证
// 信封字段与两条组合约束在发出前与 TS 一致。

/// `commandTypeSchema` 的 34 个命令。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandType {
    CreateSession,
    CreateSelectionSideSession,
    SendText,
    SendGoalCommand,
    Stop,
    Compact,
    ForkAssistant,
    ApplyFileRewind,
    EditUserQuery,
    RetryTurn,
    SetAssistantFeedback,
    SendQueuedNow,
    EditQueueItem,
    ReorderQueueItem,
    DeleteQueueItem,
    SetAutoDrain,
    ResolveInteraction,
    RespondWorkspaceHookReview,
    ToggleWorkspaceHookReviewItem,
    RevokeWorkspaceHookTrust,
    RequestWorkspaceHookReview,
    SnoozeInteractionAutoResolution,
    SwitchModelConfig,
    SwitchCollaborationMode,
    SetFollowupMode,
    PauseGoal,
    ResumeGoal,
    CancelBackgroundWork,
    ResumeWorkflowRun,
    StartSavedWorkflow,
    AmendWorkflowRunSettings,
    RenameSession,
    DeleteSession,
    DiscardSharedContext,
}

impl CommandType {
    /// TS `COMMANDS_REQUIRING_BASE_REVISION`。
    pub fn requires_base_revision(self) -> bool {
        matches!(
            self,
            Self::ApplyFileRewind
                | Self::ForkAssistant
                | Self::EditUserQuery
                | Self::RetryTurn
                | Self::SetAssistantFeedback
                | Self::SendQueuedNow
                | Self::EditQueueItem
                | Self::ReorderQueueItem
                | Self::DeleteQueueItem
                | Self::SetAutoDrain
                | Self::SwitchModelConfig
                | Self::SwitchCollaborationMode
                | Self::SetFollowupMode
                | Self::PauseGoal
                | Self::ResumeGoal
        )
    }

    /// TS `ROW_TARGETING_COMMANDS`。
    pub fn is_row_targeting(self) -> bool {
        matches!(
            self,
            Self::ApplyFileRewind
                | Self::ForkAssistant
                | Self::EditUserQuery
                | Self::RetryTurn
                | Self::SetAssistantFeedback
        )
    }
}

/// `commandEnvelopeSchema`：`sessionId` 是 required-nullable，`payload` 保持原始 JSON。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCommandEnvelope {
    /// 本地 TTFT 观测上下文；远端/可重放链路必须剥离。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft: Option<Value>,
    pub command_id: String,
    pub client_id: String,
    pub session_id: CommandSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_log_epoch: Option<String>,
    #[serde(rename = "type")]
    pub command_type: CommandType,
    pub payload: Value,
    /// 客户端时钟（毫秒），仅遥测。
    pub issued_at: Number,
}

impl ConversationCommandEnvelope {
    /// `parseCommandEnvelope` 的两条组合约束 + `baseLogEpoch` 的 trim 语义。
    pub fn validate(&self) -> Result<(), String> {
        if self
            .base_log_epoch
            .as_deref()
            .is_some_and(|epoch| epoch.trim().is_empty())
        {
            return Err("baseLogEpoch must be non-empty".to_string());
        }
        if self.command_type.requires_base_revision() && self.base_revision.is_none() {
            return Err("CAS commands require baseRevision".to_string());
        }
        if self.command_type.is_row_targeting() && self.base_log_epoch.is_none() {
            return Err("CAS commands require baseLogEpoch".to_string());
        }
        Ok(())
    }

    /// TS `sendConversationCommandV4` 的 `planEnabled` 探测（顶层 / config / firstInput）。
    pub fn plan_enabled(&self) -> bool {
        let Some(payload) = self.payload.as_object() else {
            return false;
        };
        let enabled = |value: Option<&Value>| value.and_then(Value::as_bool) == Some(true);
        enabled(payload.get("planEnabled"))
            || enabled(
                payload
                    .get("config")
                    .and_then(Value::as_object)
                    .and_then(|config| config.get("planEnabled")),
            )
            || enabled(
                payload
                    .get("firstInput")
                    .and_then(Value::as_object)
                    .and_then(|input| input.get("planEnabled")),
            )
    }

    /// 主机侧富化：`desktop_continuous` 为假或远程 workspace 时剥离 `ttft`，
    /// 并按本轮 `automationId` / `offPeakTaskId` 合并 `sendText` 的 mutation denylist。
    pub fn apply_host_enrichment(&mut self, desktop_continuous: bool, remote: bool) {
        if !desktop_continuous || remote {
            self.ttft = None;
        }
        self.merge_send_text_tool_denylist();
    }

    fn merge_send_text_tool_denylist(&mut self) {
        if self.command_type != CommandType::SendText {
            return;
        }
        let Some(payload) = self.payload.as_object_mut() else {
            return;
        };
        let has_non_empty = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        };
        let additions: &[&str] = if has_non_empty("automationId") {
            &["CronCreate", "CronUpdate", "CronDelete"]
        } else if has_non_empty("offPeakTaskId") {
            &["OffPeakCreate"]
        } else {
            return;
        };
        let mut merged: Vec<String> = payload
            .get("toolDisallowlist")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        for name in additions {
            if !merged.iter().any(|existing| existing == name) {
                merged.push((*name).to_string());
            }
        }
        payload.insert(
            "toolDisallowlist".to_string(),
            Value::Array(merged.into_iter().map(Value::String).collect()),
        );
    }
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
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = CommandSessionId;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or null")
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(CommandSessionId(None))
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(CommandSessionId(None))
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                String::deserialize(deserializer).map(|value| CommandSessionId(Some(value)))
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(CommandSessionId(Some(value.to_string())))
            }
        }

        // `deserialize_any` 而不是 `Option::deserialize`：后者的 missing-field 路径
        // 会把「字段缺失」也当成 null；协议要求 sessionId 字段必须存在。
        deserializer.deserialize_any(Visitor)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(json: Value) -> ConversationCommandEnvelope {
        serde_json::from_value(json).expect("envelope")
    }

    fn send_text(extra: Value) -> ConversationCommandEnvelope {
        let mut payload = serde_json::json!({ "text": "hi" });
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        envelope(serde_json::json!({
            "commandId": "cmd-1",
            "clientId": "client-1",
            "sessionId": "s1",
            "type": "sendText",
            "payload": payload,
            "issuedAt": 1
        }))
    }

    #[test]
    fn envelope_requires_session_id_and_rejects_unknown_fields() {
        let missing = serde_json::from_value::<ConversationCommandEnvelope>(serde_json::json!({
            "commandId": "cmd-1",
            "clientId": "client-1",
            "type": "stop",
            "payload": {},
            "issuedAt": 1
        }));
        assert!(missing.is_err());

        let unknown = serde_json::from_value::<ConversationCommandEnvelope>(serde_json::json!({
            "commandId": "cmd-1",
            "clientId": "client-1",
            "sessionId": "s1",
            "type": "stop",
            "payload": {},
            "issuedAt": 1,
            "unexpected": true
        }));
        assert!(unknown.is_err());
    }

    #[test]
    fn nullable_session_id_round_trips() {
        let value = serde_json::json!({
            "commandId": "cmd-1",
            "clientId": "client-1",
            "sessionId": null,
            "type": "createSession",
            "payload": { "workspaceId": "w1" },
            "issuedAt": 1
        });
        let parsed = envelope(value.clone());
        assert_eq!(parsed.session_id.0, None);
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    }

    #[test]
    fn cas_commands_require_revision_and_epoch() {
        let missing_revision = envelope(serde_json::json!({
            "commandId": "cmd-1",
            "clientId": "client-1",
            "sessionId": "s1",
            "type": "applyFileRewind",
            "payload": {},
            "issuedAt": 1
        }));
        assert_eq!(
            missing_revision.validate().unwrap_err(),
            "CAS commands require baseRevision"
        );

        let missing_epoch = envelope(serde_json::json!({
            "commandId": "cmd-1",
            "clientId": "client-1",
            "sessionId": "s1",
            "type": "applyFileRewind",
            "baseRevision": 1,
            "payload": {},
            "issuedAt": 1
        }));
        assert_eq!(
            missing_epoch.validate().unwrap_err(),
            "CAS commands require baseLogEpoch"
        );

        let blank_epoch = envelope(serde_json::json!({
            "commandId": "cmd-1",
            "clientId": "client-1",
            "sessionId": "s1",
            "type": "applyFileRewind",
            "baseRevision": 1,
            "baseLogEpoch": "  ",
            "payload": {},
            "issuedAt": 1
        }));
        assert_eq!(
            blank_epoch.validate().unwrap_err(),
            "baseLogEpoch must be non-empty"
        );
    }

    #[test]
    fn plan_enabled_reads_all_three_shapes() {
        assert!(envelope(serde_json::json!({
            "commandId": "cmd-1", "clientId": "c", "sessionId": null,
            "type": "createSession", "payload": { "planEnabled": true }, "issuedAt": 1
        }))
        .plan_enabled());
        assert!(envelope(serde_json::json!({
            "commandId": "cmd-1", "clientId": "c", "sessionId": "s",
            "type": "sendText", "payload": { "config": { "planEnabled": true } }, "issuedAt": 1
        }))
        .plan_enabled());
        assert!(envelope(serde_json::json!({
            "commandId": "cmd-1", "clientId": "c", "sessionId": null,
            "type": "createSession", "payload": { "firstInput": { "planEnabled": true } }, "issuedAt": 1
        }))
        .plan_enabled());
        assert!(!send_text(serde_json::json!({})).plan_enabled());
    }

    #[test]
    fn host_enrichment_strips_ttft_for_remote_or_replayable() {
        let mut local = send_text(serde_json::json!({}));
        local.ttft = Some(serde_json::json!({ "mark": 1 }));
        local.apply_host_enrichment(true, false);
        assert!(local.ttft.is_some());

        let mut remote = send_text(serde_json::json!({}));
        remote.ttft = Some(serde_json::json!({ "mark": 1 }));
        remote.apply_host_enrichment(true, true);
        assert!(remote.ttft.is_none());

        let mut replayable = send_text(serde_json::json!({}));
        replayable.ttft = Some(serde_json::json!({ "mark": 1 }));
        replayable.apply_host_enrichment(false, false);
        assert!(replayable.ttft.is_none());
    }

    #[test]
    fn host_enrichment_merges_send_text_denylist_without_duplicates() {
        let mut automation = send_text(serde_json::json!({
            "automationId": "a1",
            "toolDisallowlist": ["Custom", "CronCreate"]
        }));
        automation.apply_host_enrichment(true, false);
        assert_eq!(
            automation.payload["toolDisallowlist"],
            serde_json::json!(["Custom", "CronCreate", "CronUpdate", "CronDelete"])
        );

        let mut off_peak = send_text(serde_json::json!({ "offPeakTaskId": "o1" }));
        off_peak.apply_host_enrichment(true, false);
        assert_eq!(
            off_peak.payload["toolDisallowlist"],
            serde_json::json!(["OffPeakCreate"])
        );

        // 非 sendText 或没有本轮归因时不注入。
        let mut stop = envelope(serde_json::json!({
            "commandId": "cmd-1", "clientId": "c", "sessionId": "s",
            "type": "stop", "payload": { "automationId": "a1" }, "issuedAt": 1
        }));
        stop.apply_host_enrichment(true, false);
        assert!(stop.payload.get("toolDisallowlist").is_none());
    }
}
