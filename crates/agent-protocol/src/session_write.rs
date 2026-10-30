//! 会话写路径（第一批）：`session/close`、`session/setModel`、`session/setThoughtLevel`、
//! `session/setMode`。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的四组 params/result schema，以及
//! `packages/services/src/mode-agent/modeAgentService.ts` 的同名方法。规范见
//! `docs/specs/session-write-group.md`。
//!
//! 三个 setter 都返回完整 `modeSessionStateSnapshotSchema`，复用 `snapshot` 镜像；
//! `persistAsWorkspaceLastUsed` 在 schema 里带 `.default(true)`，但线上请求是
//! 「undefined 省略」——镜像用 `Option<bool>` 保留省略。

use serde::{Deserialize, Serialize};

use crate::message::MessageModel;
use crate::session_read::SessionMode;

pub const SESSION_CREATE_METHOD: &str = "session/create";
pub const SESSION_CLOSE_METHOD: &str = "session/close";
pub const SESSION_SET_MODEL_METHOD: &str = "session/setModel";
pub const SESSION_SET_THOUGHT_LEVEL_METHOD: &str = "session/setThoughtLevel";
pub const SESSION_SET_MODE_METHOD: &str = "session/setMode";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SessionPersistence {
    #[serde(rename = "immediate")]
    Immediate,
    #[serde(rename = "deferred")]
    Deferred,
}

/// `modeSessionCloseParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCloseParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_persistence: Option<SessionPersistence>,
}

/// `modeSessionCloseResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCloseResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed: Option<bool>,
}

/// `modeSessionSetModelParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSetModelParams {
    pub session_id: String,
    pub model: MessageModel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    /// schema 默认 `true`，但请求侧「缺省省略」——这里保留三态里的「省略」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persist_as_workspace_last_used: Option<bool>,
}

/// `modeSessionSetThoughtLevelParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSetThoughtLevelParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persist_as_workspace_last_used: Option<bool>,
}

/// `modeSessionSetModeParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionSetModeParams {
    pub session_id: String,
    pub mode: SessionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}
