//! 会话只读组第一片：`session/list`。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的 `listSessions`。
//! 规范见 `docs/specs/session-read-group.md`。
//!
//! TS 还会把返回的 `traceId` 记入进程内缓存；facade 没有该缓存，调用方按返回值自行保存。
//! 交互偏好 / Off-Peak 工具面 / provider registry 预热属于服务层编排，不在这一层。

use agent_protocol::session_read::{
    SessionInfo, SessionListParams, SessionListResult, SessionWorkspaceRef, SESSION_LIST_METHOD,
};
use serde_json::Value;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};

/// `listSessions` 的调用选项；`includeArchived` 缺省 `false`，`limit`/`sessionIds` 缺省省略。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListSessionsOptions {
    pub include_archived: bool,
    pub limit: Option<u64>,
    pub session_ids: Option<Vec<String>>,
    pub runtime_policy: RuntimePolicy,
}

impl AgentControlClient {
    /// `session/list`：返回会话列表，严格按 `modeSessionInfoSchema` 解析。
    pub async fn list_sessions(
        &self,
        target: &AgentWorkspaceTarget,
        options: ListSessionsOptions,
    ) -> Result<Vec<SessionInfo>, ControlError> {
        let process = self.client_for(target, options.runtime_policy).await?;
        let params = SessionListParams {
            workspace: Some(session_workspace_ref(target)),
            include_archived: options.include_archived,
            limit: options.limit,
            session_ids: options.session_ids,
        };
        let params =
            serde_json::to_value(&params).expect("session/list params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_LIST_METHOD, Some(params))
            .await?;
        parse_session_list(&response)
    }
}

/// `buildWorkspaceRef` 的结果形状：identity 保留原值（含空白），`workspaceKey` 已 trim。
fn session_workspace_ref(target: &AgentWorkspaceTarget) -> SessionWorkspaceRef {
    SessionWorkspaceRef {
        workspace_path: target.workspace_path.clone(),
        workspace_identity: target.workspace_identity.clone(),
        remote_session_id: target.remote_session_id.clone(),
        workspace_key: target.workspace_key(),
    }
}

fn parse_session_list(value: &Value) -> Result<Vec<SessionInfo>, ControlError> {
    let result: SessionListResult = serde_json::from_value(value.clone()).map_err(|error| {
        ControlError::InvalidResult(format!("invalid session/list result: {error}"))
    })?;
    Ok(result.sessions)
}
