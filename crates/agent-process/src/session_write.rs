//! 会话写路径 facade：`session/create`、`session/close` 与三个 setter。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的同名方法。
//! 规范见 `docs/specs/session-write-group.md`。
//!
//! 差异（与只读组一致）：TS 的 `getClient` 会先等 provider/model readiness 再拉起，
//! `createSession` 还会先同步 Account Provider Config；本 facade 只做取/拉起与协议请求，
//! 这些编排留给服务层在接入频道时补齐。

use serde_json::Value;

use agent_protocol::message::MessageModel;
use agent_protocol::session::{SessionCreateParams, SessionSendParams, SessionSendResult};
use agent_protocol::session_create::{ImportHistory, McpServer};
use agent_protocol::session_read::SessionMode;
use agent_protocol::session_send::SESSION_SEND_METHOD;
use agent_protocol::session_write::{
    SessionCloseParams, SessionCloseResult, SessionCompactParams, SessionCompactResult,
    SessionGoalParams, SessionGoalResult, SessionPersistence, SessionResumeParams,
    SessionSetModeParams, SessionSetModelParams, SessionSetThoughtLevelParams,
    SESSION_CLOSE_METHOD, SESSION_COMPACT_METHOD, SESSION_CREATE_METHOD, SESSION_GOAL_METHOD,
    SESSION_RESUME_METHOD, SESSION_SET_MODEL_METHOD, SESSION_SET_MODE_METHOD,
    SESSION_SET_THOUGHT_LEVEL_METHOD,
};
use agent_protocol::snapshot::SessionStateSnapshot;
use mode_protocol_client::ClientError;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use crate::session_read::session_workspace_ref;

/// `createSession` 的调用参数；缺省字段不会序列化。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CreateSessionOptions {
    pub session_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub mode: Option<SessionMode>,
    pub model: Option<MessageModel>,
    pub persistence: Option<SessionPersistence>,
    pub thought_level: Option<String>,
    pub title_generation_enabled: Option<bool>,
    pub mcp_servers: Option<Vec<McpServer>>,
    pub tool_allowlist: Option<Vec<String>>,
    pub tool_denylist: Option<Vec<String>>,
    pub imported_history: Option<ImportHistory>,
    /// 灰度命中才下发 `true`（TS 只在 `=== true` 时写字段；`false` 与缺省等价）。
    pub off_peak_tool_enabled: bool,
    pub dynamic_workflow_enabled: bool,
}

impl AgentControlClient {
    /// `session/create`：返回新会话的完整状态快照。
    pub async fn create_session(
        &self,
        target: &AgentWorkspaceTarget,
        options: CreateSessionOptions,
    ) -> Result<SessionStateSnapshot, ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = SessionCreateParams {
            session_id: options.session_id,
            workspace: session_workspace_ref(target),
            parent_session_id: options.parent_session_id,
            mode: options.mode,
            model: options.model,
            persistence: options.persistence,
            thought_level: options.thought_level,
            title_generation_enabled: options.title_generation_enabled,
            mcp_servers: options.mcp_servers,
            tool_allowlist: options.tool_allowlist,
            tool_denylist: options.tool_denylist,
            imported_history: options.imported_history,
            off_peak_tool_enabled: options.off_peak_tool_enabled.then_some(true),
            dynamic_workflow_enabled: options.dynamic_workflow_enabled.then_some(true),
        };
        let params = serde_json::to_value(&params).expect("session/create params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_CREATE_METHOD, Some(params))
            .await?;
        parse_snapshot(&response, "session/create")
    }

    /// `session/send`：提交一次输入，返回受理确认（`accepted` 必须是 `true`）。
    ///
    /// 组合约束在发出前校验：`automationId`/`offPeakTaskId` 互斥、`offPeakRunType`
    /// 依赖 `offPeakTaskId`、`modelExecution` 依赖 `modelSelection`。
    pub async fn send_prompt(
        &self,
        target: &AgentWorkspaceTarget,
        params: SessionSendParams,
    ) -> Result<SessionSendResult, ControlError> {
        params
            .validate()
            .map_err(|message| ControlError::InvalidParams(message.to_string()))?;
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = serde_json::to_value(&params).expect("session/send params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_SEND_METHOD, Some(params))
            .await?;
        let result: SessionSendResult = serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid session/send result: {error}"))
        })?;
        if !result.accepted {
            return Err(ControlError::InvalidResult(
                "invalid session/send result: accepted must be true".to_string(),
            ));
        }
        Ok(result)
    }

    /// `session/close`：返回 `closed`（Agent 未返回该字段时按普通 close 成功处理为 `true`）。
    pub async fn close_session(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        expected_persistence: Option<SessionPersistence>,
    ) -> Result<bool, ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = SessionCloseParams {
            session_id: session_id.to_string(),
            expected_persistence,
        };
        let params = serde_json::to_value(&params).expect("session/close params 序列化不会失败");
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
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = SessionSetModelParams {
            session_id: session_id.to_string(),
            model: model.clone(),
            expected_revision,
            persist_as_workspace_last_used,
        };
        let params = serde_json::to_value(&params).expect("session/setModel params 序列化不会失败");
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
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
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
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let params = SessionSetModeParams {
            session_id: session_id.to_string(),
            mode,
            expected_revision,
        };
        let params = serde_json::to_value(&params).expect("session/setMode params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_SET_MODE_METHOD, Some(params))
            .await?;
        parse_snapshot(&response, "session/setMode")
    }

    /// `session/compact`：返回 response + 完整快照（可选 compact 受理状态）。
    pub async fn compact_session(
        &self,
        target: &AgentWorkspaceTarget,
        params: SessionCompactParams,
    ) -> Result<SessionCompactResult, ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let value = serde_json::to_value(&params).expect("session/compact params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_COMPACT_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid session/compact result: {error}"))
        })
    }

    /// `session/goal`：action 覆盖 show/set/replace/pause/resume/clear。
    pub async fn goal_session(
        &self,
        target: &AgentWorkspaceTarget,
        params: SessionGoalParams,
    ) -> Result<SessionGoalResult, ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let value = serde_json::to_value(&params).expect("session/goal params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_GOAL_METHOD, Some(value))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid session/goal result: {error}"))
        })
    }

    /// `session/resume`：冷恢复；旧 Agent 对新增字段报 `-32602` 时按 TS 同一集合
    /// 省略字段重试一次（只信任 `data.message` 里的结构化 zod issues）。
    pub async fn resume_session(
        &self,
        target: &AgentWorkspaceTarget,
        params: SessionResumeParams,
    ) -> Result<SessionStateSnapshot, ControlError> {
        let process = self
            .client_for(target, RuntimePolicy::StartIfNeeded)
            .await?;
        let first = request_resume(process.client(), &params).await;
        match first {
            Ok(response) => parse_snapshot(&response, "session/resume"),
            Err(error) => {
                let fields = resume_compat_fields(&error);
                if fields.is_empty() {
                    return Err(error);
                }
                let retry_params = params.omit_fields(&fields);
                let response = request_resume(process.client(), &retry_params).await?;
                parse_snapshot(&response, "session/resume")
            }
        }
    }
}

async fn request_resume(
    client: &mode_protocol_client::ProtocolClient,
    params: &SessionResumeParams,
) -> Result<Value, ControlError> {
    let value = serde_json::to_value(params).expect("session/resume params 序列化不会失败");
    client
        .request(SESSION_RESUME_METHOD, Some(value))
        .await
        .map_err(ControlError::Client)
}

/// 解析 `-32602` 的 `data.message`（JSON 数组）并与 TS 集合比对：
/// 只要出现不认识的键（或结构不符），就不做兼容重试，原错误上抛。
fn resume_compat_fields(error: &ControlError) -> Vec<String> {
    let ControlError::Client(ClientError::Server {
        code: -32602,
        data: Some(data),
        ..
    }) = error
    else {
        return Vec::new();
    };
    let Some(serialized) = data.get("message").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Ok(issues) = serde_json::from_str::<Value>(serialized) else {
        return Vec::new();
    };
    let Some(issues) = issues.as_array() else {
        return Vec::new();
    };
    let mut keys: Vec<String> = Vec::new();
    for issue in issues {
        if issue.get("code").and_then(Value::as_str) != Some("unrecognized_keys") {
            continue;
        }
        if issue
            .get("path")
            .and_then(Value::as_array)
            .is_some_and(|path| !path.is_empty())
        {
            continue;
        }
        let Some(issue_keys) = issue.get("keys").and_then(Value::as_array) else {
            continue;
        };
        for key in issue_keys {
            let Some(key) = key.as_str() else {
                continue;
            };
            if !SessionResumeParams::OPTIONAL_COMPAT_FIELDS.contains(&key) {
                return Vec::new();
            }
            if !keys.iter().any(|existing| existing == key) {
                keys.push(key.to_string());
            }
        }
    }
    keys
}

fn parse_snapshot(value: &Value, method: &str) -> Result<SessionStateSnapshot, ControlError> {
    serde_json::from_value(value.clone())
        .map_err(|error| ControlError::InvalidResult(format!("invalid {method} result: {error}")))
}
