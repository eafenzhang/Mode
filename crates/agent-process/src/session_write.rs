//! 会话写路径 facade：`session/close` 与三个 setter。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的同名方法。
//! 规范见 `docs/specs/session-write-group.md`。
//!
//! 差异（与只读组一致）：TS 的 `getClient` 会先等 provider/model readiness 再拉起；
//! 本 facade 只做取/拉起，readiness 门禁留给服务层在接入频道时补齐。

use serde_json::Value;

use agent_protocol::message::MessageModel;
use agent_protocol::session_read::SessionMode;
use agent_protocol::session_write::{
    SessionCloseParams, SessionCloseResult, SessionPersistence, SessionSetModeParams,
    SessionSetModelParams, SessionSetThoughtLevelParams, SESSION_CLOSE_METHOD,
    SESSION_SET_MODE_METHOD, SESSION_SET_MODEL_METHOD, SESSION_SET_THOUGHT_LEVEL_METHOD,
};
use agent_protocol::snapshot::SessionStateSnapshot;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};

impl AgentControlClient {
    /// `session/close`：返回 `closed`（Agent 未返回该字段时按普通 close 成功处理为 `true`）。
    pub async fn close_session(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        expected_persistence: Option<SessionPersistence>,
    ) -> Result<bool, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = SessionCloseParams {
            session_id: session_id.to_string(),
            expected_persistence,
        };
        let params =
            serde_json::to_value(&params).expect("session/close params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_CLOSE_METHOD, Some(params))
            .await?;
        let result: SessionCloseResult = serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid session/close result: {error}"))
        })?;
        Ok(result.closed.unwrap_or(true))
    }

    /// `session/setModel`：返回新的完整状态快照。
    pub async fn set_session_model(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        model: &MessageModel,
        expected_revision: Option<u64>,
        persist_as_workspace_last_used: Option<bool>,
    ) -> Result<SessionStateSnapshot, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = SessionSetModelParams {
            session_id: session_id.to_string(),
            model: model.clone(),
            expected_revision,
            persist_as_workspace_last_used,
        };
        let params =
            serde_json::to_value(&params).expect("session/setModel params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_SET_MODEL_METHOD, Some(params))
            .await?;
        parse_snapshot(&response, "session/setModel")
    }

    /// `session/setThoughtLevel`：`thought_level == None` 时省略键。
    pub async fn set_session_thought_level(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        thought_level: Option<&str>,
        expected_revision: Option<u64>,
        persist_as_workspace_last_used: Option<bool>,
    ) -> Result<SessionStateSnapshot, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = SessionSetThoughtLevelParams {
            session_id: session_id.to_string(),
            thought_level: thought_level.map(str::to_string),
            expected_revision,
            persist_as_workspace_last_used,
        };
        let params =
            serde_json::to_value(&params).expect("session/setThoughtLevel params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_SET_THOUGHT_LEVEL_METHOD, Some(params))
            .await?;
        parse_snapshot(&response, "session/setThoughtLevel")
    }

    /// `session/setMode`：返回新的完整状态快照。
    pub async fn set_session_mode(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        mode: SessionMode,
        expected_revision: Option<u64>,
    ) -> Result<SessionStateSnapshot, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = SessionSetModeParams {
            session_id: session_id.to_string(),
            mode,
            expected_revision,
        };
        let params =
            serde_json::to_value(&params).expect("session/setMode params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_SET_MODE_METHOD, Some(params))
            .await?;
        parse_snapshot(&response, "session/setMode")
    }
}

fn parse_snapshot(
    value: &Value,
    method: &str,
) -> Result<SessionStateSnapshot, ControlError> {
    serde_json::from_value(value.clone()).map_err(|error| {
        ControlError::InvalidResult(format!("invalid {method} result: {error}"))
    })
}
