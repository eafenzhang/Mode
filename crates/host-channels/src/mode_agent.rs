//! `mode-agent` 频道适配器（S2 第一批：会话只读组 + MCP 状态）。
//!
//! spec：`docs/specs/host-ownership-switch.md` §5 S2 与
//! `docs/specs/agent-method-coverage.json`。频道默认**不注册**：只有
//! `MODE_HOST_INPROCESS_CHANNELS` 显式包含 `mode-agent` 时才由壳接管。
//! 未迁移的方法一律返回 `unsupported_method`（决策 2 契约），不静默降级。
//!
//! 已知保真缺口（S2 硬门禁前必须补齐，见 spec §5 与覆盖矩阵）：
//! - `readSession` 的 Account Provider Config 预同步与 TS 的 passive-read
//!   runtimePolicy 由服务层编排，本层硬编码 `StartIfNeeded`；
//! - `listMcpServerStatuses` 不做 TS 的 `unrecognized_keys` 兼容重试。

use std::sync::Arc;

use agent_process::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use agent_protocol::mcp::{McpListMode, McpListResult};
use agent_protocol::message::MessageWithParts;
use agent_protocol::session_create::McpServer;
use agent_protocol::session_debug::SessionDebugSnapshot;
use agent_protocol::session_event::SessionEvent;
use agent_protocol::session_read::{SessionInfo, SessionSubagentsResult};
use agent_protocol::snapshot::{DeliveryKind, SessionStateSnapshot};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Deserialize;

use crate::params::first_arg_object;

pub const MODE_AGENT_CHANNEL: &str = "mode-agent";

/// 决策 2 契约：Rust 侧未实现的方法返回该错误码（host-ownership-switch §6 决策 2）。
pub const UNSUPPORTED_METHOD_CODE: &str = "unsupported_method";

pub struct ModeAgentChannel {
    control: Arc<AgentControlClient>,
}

impl ModeAgentChannel {
    pub fn new(control: Arc<AgentControlClient>) -> Self {
        Self { control }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceParams {
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
    #[serde(default)]
    remote_session_id: Option<String>,
}

impl WorkspaceParams {
    fn target(&self) -> AgentWorkspaceTarget {
        let mut target = AgentWorkspaceTarget::new(self.workspace_path.clone());
        if let Some(identity) = &self.workspace_identity {
            target = target.with_identity(identity.clone());
        }
        if let Some(remote_session) = &self.remote_session_id {
            target = target.with_remote_session(remote_session.clone());
        }
        target
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSessionsParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    session_ids: Option<Vec<String>>,
    #[serde(default)]
    runtime_policy: Option<String>,
    #[serde(default)]
    include_archived: Option<bool>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadSessionParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    delivery_kind: Option<String>,
    #[serde(default)]
    message_limit: Option<u64>,
    #[serde(default)]
    after_seq: Option<u64>,
    // runtimePolicy 暂不接受：agent-process 的 read_session 硬编码 StartIfNeeded（见模块头
    // 已知保真缺口）。保留键会被 serde 忽略，不制造假接线。
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadSessionMessagesParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    after_message_id: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadSessionEventsParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    after_seq: Option<u64>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSessionSubagentsParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
    #[serde(default)]
    ended_cursor: Option<String>,
    #[serde(default)]
    ended_limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionTargetParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    session_id: String,
}

fn parse_params<T: serde::de::DeserializeOwned>(
    command: &str,
    arg: Value,
) -> Result<T, ChannelError> {
    let object = first_arg_object(&arg)?;
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|error| ChannelError::error(format!("invalid {command} request: {error}")))
}

fn policy_from(value: Option<&str>) -> Result<RuntimePolicy, ChannelError> {
    match value {
        None | Some("start-if-needed") => Ok(RuntimePolicy::StartIfNeeded),
        Some("existing-only") => Ok(RuntimePolicy::ExistingOnly),
        Some(other) => Err(ChannelError::error(format!(
            "invalid runtimePolicy: {other}"
        ))),
    }
}

fn delivery_from(value: Option<&str>) -> Result<Option<DeliveryKind>, ChannelError> {
    match value {
        None => Ok(None),
        Some("desktop-continuous") => Ok(Some(DeliveryKind::DesktopContinuous)),
        Some("web-remote-replayable") => Ok(Some(DeliveryKind::WebRemoteReplayable)),
        Some(other) => Err(ChannelError::error(format!(
            "invalid deliveryKind: {other}"
        ))),
    }
}

fn control_error(error: ControlError) -> ChannelError {
    ChannelError::error(error.to_string())
}

fn json_value<T: serde::Serialize>(value: &T) -> Result<Value, ChannelError> {
    serde_json::to_value(value)
        .map(|json| Value::Json(json.to_string()))
        .map_err(|error| ChannelError::error(format!("serialize result failed: {error}")))
}

/// 未迁移方法：显式 `unsupported_method`（带回滚指引），不做静默降级。
fn unsupported(command: &str) -> ChannelError {
    ChannelError::error(format!(
        "mode-agent method {command} 依赖真实模型/网络或尚未迁移；当前频道由 Rust 所有，\
         如需使用请把它从 MODE_HOST_INPROCESS_CHANNELS 移除以回滚到 Node Host"
    ))
    .with_extra("code", Value::Str(UNSUPPORTED_METHOD_CODE.into()))
}

impl ServerChannel for ModeAgentChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let control = Arc::clone(&self.control);
        Box::pin(async move {
            match command.as_str() {
                "listSessions" => {
                    let params: ListSessionsParams = parse_params("listSessions", arg)?;
                    let policy = policy_from(params.runtime_policy.as_deref())?;
                    let sessions: Vec<SessionInfo> = control
                        .list_sessions(
                            &params.workspace.target(),
                            agent_process::ListSessionsOptions {
                                include_archived: params.include_archived.unwrap_or(false),
                                limit: params.limit,
                                session_ids: params.session_ids,
                                runtime_policy: policy,
                            },
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&sessions)
                }
                "readSession" => {
                    let params: ReadSessionParams = parse_params("readSession", arg)?;
                    let snapshot: SessionStateSnapshot = control
                        .read_session(
                            &params.workspace.target(),
                            &params.session_id,
                            delivery_from(params.delivery_kind.as_deref())?,
                            params.message_limit,
                            params.after_seq,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&snapshot)
                }
                "readSessionMessages" => {
                    let params: ReadSessionMessagesParams =
                        parse_params("readSessionMessages", arg)?;
                    let messages: Vec<MessageWithParts> = control
                        .read_session_messages(
                            &params.workspace.target(),
                            &params.session_id,
                            params.after_message_id.as_deref(),
                            params.limit,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&messages)
                }
                "readSessionEvents" => {
                    let params: ReadSessionEventsParams = parse_params("readSessionEvents", arg)?;
                    let events: Vec<SessionEvent> = control
                        .read_session_events(
                            &params.workspace.target(),
                            &params.session_id,
                            params.after_seq,
                            params.limit,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&events)
                }
                "listSessionSubagents" => {
                    let params: ListSessionSubagentsParams =
                        parse_params("listSessionSubagents", arg)?;
                    let result: SessionSubagentsResult = control
                        .list_session_subagents(
                            &params.workspace.target(),
                            &params.session_id,
                            params.ended_cursor.as_deref(),
                            params.ended_limit,
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                "readSessionDebug" => {
                    let params: SessionTargetParams = parse_params("readSessionDebug", arg)?;
                    let snapshot: SessionDebugSnapshot = control
                        .read_session_debug(&params.workspace.target(), &params.session_id)
                        .await
                        .map_err(control_error)?;
                    json_value(&snapshot)
                }
                "listMcpServerStatuses" => {
                    let params: ListMcpParams = parse_params("listMcpServerStatuses", arg)?;
                    let result: McpListResult = control
                        .list_mcp_server_statuses(
                            &params.workspace.target(),
                            agent_process::ListMcpServersOptions {
                                mcp_servers: params.mcp_servers,
                                mode: params.mode,
                            },
                        )
                        .await
                        .map_err(control_error)?;
                    json_value(&result)
                }
                other => Err(unsupported(other)),
            }
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListMcpParams {
    #[serde(flatten)]
    workspace: WorkspaceParams,
    #[serde(default)]
    mcp_servers: Option<Vec<McpServer>>,
    #[serde(default)]
    mode: Option<McpListMode>,
}
