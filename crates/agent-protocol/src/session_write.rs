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

pub const SESSION_COMPACT_METHOD: &str = "session/compact";
pub const SESSION_GOAL_METHOD: &str = "session/goal";

/// `modeSessionCompactParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCompactParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionCompactState {
    #[serde(rename = "accepted")]
    Accepted,
    #[serde(rename = "already_running")]
    AlreadyRunning,
}

/// `modeSessionCompactResultSchema.compact`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCompactInfo {
    pub state: SessionCompactState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

/// `modeSessionCompactResultSchema`：结果复用完整状态快照。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCompactResult {
    pub response: String,
    pub snapshot: crate::snapshot::SessionStateSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact: Option<SessionCompactInfo>,
}

/// `modeSessionGoalActionSchema`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionGoalAction {
    #[serde(rename = "show")]
    Show,
    #[serde(rename = "set")]
    Set,
    #[serde(rename = "replace")]
    Replace,
    #[serde(rename = "pause")]
    Pause,
    #[serde(rename = "resume")]
    Resume,
    #[serde(rename = "clear")]
    Clear,
}

/// `modeSessionGoalParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionGoalParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    pub action: SessionGoalAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
}

/// `modeSessionGoalResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionGoalResult {
    pub response: String,
    pub snapshot: crate::snapshot::SessionStateSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_turn: Option<bool>,
}

pub const SESSION_RESUME_METHOD: &str = "session/resume";

/// `modeSessionResumeParamsSchema`：冷恢复重建 runtime 时必须沿用 create 的工具面约束。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionResumeParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<crate::session::WorkspaceRef>,
    /// 旧 session 没有 runtime/model_selection entry 时的迁移 hint。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<crate::session_create::McpServer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_allowlist: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_denylist: Option<Vec<String>>,
    /// 与 create 同语义；TS 只在 true 时下发。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_peak_tool_enabled: Option<bool>,
    /// 与 create 同语义；TS 只在 true 时下发。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_workflow_enabled: Option<bool>,
}

impl SessionResumeParams {
    /// 旧 Agent 不认的顶层可选字段（与 TS 兼容重试集合一致）。
    pub const OPTIONAL_COMPAT_FIELDS: [&'static str; 6] = [
        "thoughtLevel",
        "mcpServers",
        "toolAllowlist",
        "toolDenylist",
        "offPeakToolEnabled",
        "dynamicWorkflowEnabled",
    ];

    /// 按兼容重试要求省略指定顶层字段（字段名用协议 camelCase）。
    pub fn omit_fields(&self, omitted: &[String]) -> Self {
        let has = |name: &str| omitted.iter().any(|value| value == name);
        Self {
            session_id: self.session_id.clone(),
            workspace: self.workspace.clone(),
            thought_level: if has("thoughtLevel") {
                None
            } else {
                self.thought_level.clone()
            },
            mcp_servers: if has("mcpServers") {
                None
            } else {
                self.mcp_servers.clone()
            },
            tool_allowlist: if has("toolAllowlist") {
                None
            } else {
                self.tool_allowlist.clone()
            },
            tool_denylist: if has("toolDenylist") {
                None
            } else {
                self.tool_denylist.clone()
            },
            off_peak_tool_enabled: if has("offPeakToolEnabled") {
                None
            } else {
                self.off_peak_tool_enabled
            },
            dynamic_workflow_enabled: if has("dynamicWorkflowEnabled") {
                None
            } else {
                self.dynamic_workflow_enabled
            },
        }
    }
}
