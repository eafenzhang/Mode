//! `session/send` 的嵌套参数类型：执行约束、浏览器环境上下文与 Bot 回推目标。
//!
//! 真源：`packages/shared/src/model-execution.ts`、`mode-protocol/index.ts` 的
//! `modeBrowserAmbientContextSchema`、`packages/shared/src/bots.ts` 的
//! `modeAutomationBotDeliveryTargetSchema`。规范见 `docs/specs/session-write-group.md`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SESSION_SEND_METHOD: &str = "session/send";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryExtractionSkip {
    #[serde(rename = "skip")]
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionScopeExecution {
    #[serde(rename = "execution")]
    Execution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelExecutionRequestAuth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForegroundSubmission {
    #[serde(rename = "submission")]
    Submission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackgroundDeny {
    #[serde(rename = "deny")]
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelExecutionSubagents {
    pub foreground_model: ForegroundSubmission,
    pub background: BackgroundDeny,
}

/// `modelExecutionSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelExecution {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_extraction: Option<MemoryExtractionSkip>,
    pub selection_scope: SelectionScopeExecution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_auth: Option<ModelExecutionRequestAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<ModelExecutionSubagents>,
}

/// `modeBrowserAmbientContextSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserAmbientContext {
    pub tab_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BotProvider {
    #[serde(rename = "feishu")]
    Feishu,
    #[serde(rename = "lark")]
    Lark,
    #[serde(rename = "weixin")]
    Weixin,
    #[serde(rename = "telegram")]
    Telegram,
    #[serde(rename = "wecom")]
    Wecom,
    #[serde(rename = "dingtalk")]
    Dingtalk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BotChatType {
    #[serde(rename = "private")]
    Private,
    #[serde(rename = "group")]
    Group,
}

/// `modeAutomationBotDeliveryTargetSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BotDeliveryTarget {
    pub provider: BotProvider,
    pub bot_id: String,
    pub provider_user_id: String,
    pub chat_type: BotChatType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OffPeakRunType {
    #[serde(rename = "init")]
    Init,
    #[serde(rename = "resume")]
    Resume,
}
