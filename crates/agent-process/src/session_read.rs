//! 会话只读组第一片：`session/list`。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的 `listSessions`。
//! 规范见 `docs/specs/session-read-group.md`。
//!
//! TS 还会把返回的 `traceId` 记入进程内缓存；facade 没有该缓存，调用方按返回值自行保存。
//! 交互偏好 / Off-Peak 工具面 / provider registry 预热属于服务层编排，不在这一层。

use agent_protocol::message::MessageWithParts;
use agent_protocol::session_debug::{
    SessionDebugParams, SessionDebugSnapshot, SESSION_DEBUG_METHOD,
};
use agent_protocol::session_event::SessionEvent;
use agent_protocol::session_read::{
    SessionEventsParams, SessionEventsResult, SessionInfo, SessionListParams, SessionListResult,
    SessionMessagesParams, SessionMessagesResult, SessionReadParams, SessionSubagentsParams,
    SessionSubagentsResult, SessionWorkspaceRef, SESSION_EVENTS_METHOD, SESSION_LIST_METHOD,
    SESSION_MESSAGES_METHOD, SESSION_READ_METHOD, SESSION_SUBAGENTS_METHOD,
};
use agent_protocol::snapshot::{DeliveryKind, SessionStateSnapshot};
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

    /// `session/events`：按 `afterSeq` / `limit` 读取会话事件。
    ///
    /// 信封严格解析，载荷保持协议原始 JSON（强类型载荷由消费方按 `type` 解释）。
    pub async fn read_session_events(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        after_seq: Option<u64>,
        limit: Option<u64>,
    ) -> Result<Vec<SessionEvent>, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = SessionEventsParams {
            session_id: session_id.to_string(),
            after_seq,
            limit,
        };
        let params =
            serde_json::to_value(&params).expect("session/events params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_EVENTS_METHOD, Some(params))
            .await?;
        parse_session_events(&response)
    }

    /// `session/subagents`：运行中与已结束子代理；`endedLimit` 缺省 20（与 TS `?? 20` 一致）。
    pub async fn list_session_subagents(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        ended_cursor: Option<&str>,
        ended_limit: Option<u64>,
    ) -> Result<SessionSubagentsResult, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = SessionSubagentsParams {
            session_id: session_id.to_string(),
            ended_cursor: ended_cursor.map(str::to_string),
            ended_limit: ended_limit.unwrap_or(20),
        };
        let params =
            serde_json::to_value(&params).expect("session/subagents params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_SUBAGENTS_METHOD, Some(params))
            .await?;
        parse_session_subagents(&response)
    }

    /// `session/messages`：按 `afterMessageId` / `limit` 读取消息与部件。
    pub async fn read_session_messages(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        after_message_id: Option<&str>,
        limit: Option<u64>,
    ) -> Result<Vec<MessageWithParts>, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = SessionMessagesParams {
            session_id: session_id.to_string(),
            after_message_id: after_message_id.map(str::to_string),
            limit,
        };
        let params =
            serde_json::to_value(&params).expect("session/messages params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_MESSAGES_METHOD, Some(params))
            .await?;
        parse_session_messages(&response)
    }

    /// `session/read`：读取完整会话状态快照（严格按 `modeSessionStateSnapshotSchema`）。
    ///
    /// TS 在默认策略下会先同步 Account Provider Config；本 facade 没有账号事实源，
    /// 该编排留给服务层（见 `docs/specs/session-read-group.md`）。
    pub async fn read_session(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        delivery_kind: Option<DeliveryKind>,
        message_limit: Option<u64>,
        after_seq: Option<u64>,
    ) -> Result<SessionStateSnapshot, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = SessionReadParams {
            session_id: session_id.to_string(),
            delivery_kind,
            message_limit,
            after_seq,
        };
        let params =
            serde_json::to_value(&params).expect("session/read params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_READ_METHOD, Some(params))
            .await?;
        parse_session_snapshot(&response)
    }

    /// `session/debug`：诊断快照。existing-only——观察路径不得拉起 Agent。
    pub async fn read_session_debug(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
    ) -> Result<SessionDebugSnapshot, ControlError> {
        let process = self.client_for(target, RuntimePolicy::ExistingOnly).await?;
        let params = SessionDebugParams {
            session_id: session_id.to_string(),
        };
        let params =
            serde_json::to_value(&params).expect("session/debug params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_DEBUG_METHOD, Some(params))
            .await?;
        parse_session_debug(&response)
    }
}

/// `buildWorkspaceRef` 的结果形状：identity 保留原值（含空白），`workspaceKey` 已 trim。
pub(crate) fn session_workspace_ref(target: &AgentWorkspaceTarget) -> SessionWorkspaceRef {
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

fn parse_session_events(value: &Value) -> Result<Vec<SessionEvent>, ControlError> {
    let result: SessionEventsResult = serde_json::from_value(value.clone()).map_err(|error| {
        ControlError::InvalidResult(format!("invalid session/events result: {error}"))
    })?;
    Ok(result.events)
}

fn parse_session_subagents(value: &Value) -> Result<SessionSubagentsResult, ControlError> {
    serde_json::from_value(value.clone()).map_err(|error| {
        ControlError::InvalidResult(format!("invalid session/subagents result: {error}"))
    })
}

fn parse_session_messages(value: &Value) -> Result<Vec<MessageWithParts>, ControlError> {
    let result: SessionMessagesResult = serde_json::from_value(value.clone()).map_err(|error| {
        ControlError::InvalidResult(format!("invalid session/messages result: {error}"))
    })?;
    Ok(result.messages)
}

fn parse_session_snapshot(value: &Value) -> Result<SessionStateSnapshot, ControlError> {
    serde_json::from_value(value.clone()).map_err(|error| {
        ControlError::InvalidResult(format!("invalid session/read result: {error}"))
    })
}

fn parse_session_debug(value: &Value) -> Result<SessionDebugSnapshot, ControlError> {
    serde_json::from_value(value.clone()).map_err(|error| {
        ControlError::InvalidResult(format!("invalid session/debug result: {error}"))
    })
}
