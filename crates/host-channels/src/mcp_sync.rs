//! `mcp-sync` 频道适配器。
//!
//! 真源：`packages/services/src/mcp-sync/mcpSync.ts`。
//! 本地七个方法落到 `mcp-sync-service`；`listWorkspaceMcpServerStatuses` 是 UI 的
//! 注入面，落到 W4 控制面（`AgentControlClient::list_mcp_server_statuses`）——
//! mcp/list 词的 Host 消费收拢到这一处，与 TS 的注释同口径。

use std::sync::Arc;

use agent_process::{AgentControlClient, AgentWorkspaceTarget, ListMcpServersOptions};
use agent_protocol::mcp::McpListMode;
use agent_protocol::session_create::McpServer as ProtocolMcpServer;
use mcp_sync_service::{ImportParams, LoadRequest, McpSyncService, SavePayload};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Deserialize;

use crate::params::first_arg_object;

pub const MCP_SYNC_CHANNEL: &str = "mcp-sync";

pub const LOAD_MCP_FROM_USER_DIRECTORY: &str = "loadMcpFromUserDirectory";
pub const LIST_WORKSPACE_MCP_SERVER_STATUSES: &str = "listWorkspaceMcpServerStatuses";
pub const SAVE_MCP_TO_USER_DIRECTORY: &str = "saveMcpToUserDirectory";
pub const LIST_LOCAL_USER_MCP_CANDIDATES: &str = "listLocalUserMcpCandidates";
pub const LIST_REMOTE_USER_MCP_STATUSES: &str = "listRemoteUserMcpStatuses";
pub const EXPORT_MCP_SERVERS: &str = "exportMcpServers";
pub const CHECK_REMOTE_USER_MCP_WRITE_ACCESS: &str = "checkRemoteUserMcpWriteAccess";
pub const IMPORT_MCP_SERVERS: &str = "importMcpServers";

pub struct McpSyncChannel {
    service: Arc<McpSyncService>,
    control: Arc<AgentControlClient>,
}

impl McpSyncChannel {
    pub fn new(service: Arc<McpSyncService>, control: Arc<AgentControlClient>) -> Self {
        Self { service, control }
    }

    /// 生产装配：home 由默认环境解析（HOME → USERPROFILE → homedir）。
    pub fn from_process(control: Arc<AgentControlClient>) -> Self {
        Self::new(
            Arc::new(McpSyncService::new(
                mcp_sync_service::McpSyncServiceOptions { home_dir: None },
            )),
            control,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceMcpStatusParams {
    workspace_path: String,
    #[serde(default)]
    workspace_identity: Option<String>,
    #[serde(default)]
    mcp_servers: Option<Vec<ProtocolMcpServer>>,
    #[serde(default)]
    mode: Option<McpListMode>,
}

#[derive(Debug, Deserialize)]
struct NamesParams {
    names: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerIdsParams {
    server_ids: Vec<String>,
}

fn parse_params<T: serde::de::DeserializeOwned>(
    command: &str,
    arg: Value,
) -> Result<T, ChannelError> {
    let object = first_arg_object(&arg)?;
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|error| ChannelError::error(format!("invalid {command} request: {error}")))
}

/// `loadMcpFromUserDirectory` 的 request 可省略（TS `request?`）。
fn parse_load_request(arg: Value) -> Result<Option<LoadRequest>, ChannelError> {
    match arg {
        Value::Undefined => Ok(None),
        other => {
            let object = first_arg_object(&other)?;
            serde_json::from_value(serde_json::Value::Object(object))
                .map(Some)
                .map_err(|error| ChannelError::error(format!("invalid load request: {error}")))
        }
    }
}

/// 一次失败序列化也返回可解析的 JSON（与其它频道同口径）。
fn json_value<T: serde::Serialize>(value: &T) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}

fn json_text(json: serde_json::Value) -> Value {
    Value::Json(json.to_string())
}

impl ServerChannel for McpSyncChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let service = Arc::clone(&self.service);
        let control = Arc::clone(&self.control);
        Box::pin(async move {
            match command.as_str() {
                LOAD_MCP_FROM_USER_DIRECTORY => {
                    let request = parse_load_request(arg)?;
                    let servers = service
                        .load_mcp_from_user_directory(request.as_ref())
                        .map_err(ChannelError::error)?;
                    Ok(json_text(serde_json::json!({ "servers": servers })))
                }
                LIST_WORKSPACE_MCP_SERVER_STATUSES => {
                    let params: WorkspaceMcpStatusParams =
                        parse_params("listWorkspaceMcpServerStatuses", arg)?;
                    let mut target = AgentWorkspaceTarget::new(params.workspace_path);
                    if let Some(identity) = &params.workspace_identity {
                        target = target.with_identity(identity.clone());
                    }
                    let result = control
                        .list_mcp_server_statuses(
                            &target,
                            ListMcpServersOptions {
                                mcp_servers: params.mcp_servers,
                                mode: params.mode,
                            },
                        )
                        .await
                        .map_err(|error| ChannelError::error(error.to_string()))?;
                    Ok(json_value(&result))
                }
                SAVE_MCP_TO_USER_DIRECTORY => {
                    let payload: SavePayload = parse_params("saveMcpToUserDirectory", arg)?;
                    service
                        .save_mcp_to_user_directory(&payload)
                        .map_err(ChannelError::error)?;
                    Ok(Value::Undefined)
                }
                LIST_LOCAL_USER_MCP_CANDIDATES => {
                    let result = service
                        .list_local_user_mcp_candidates()
                        .map_err(ChannelError::error)?;
                    Ok(json_value(&result))
                }
                LIST_REMOTE_USER_MCP_STATUSES => {
                    let params: NamesParams = parse_params("listRemoteUserMcpStatuses", arg)?;
                    let result = service
                        .list_remote_user_mcp_statuses(&params.names)
                        .map_err(ChannelError::error)?;
                    Ok(json_value(&result))
                }
                EXPORT_MCP_SERVERS => {
                    let params: ServerIdsParams = parse_params("exportMcpServers", arg)?;
                    let result = service
                        .export_mcp_servers(&params.server_ids)
                        .map_err(ChannelError::error)?;
                    Ok(json_value(&result))
                }
                CHECK_REMOTE_USER_MCP_WRITE_ACCESS => {
                    let result = service
                        .check_remote_user_mcp_write_access()
                        .map_err(ChannelError::error)?;
                    Ok(json_value(&result))
                }
                IMPORT_MCP_SERVERS => {
                    let params: ImportParams = parse_params("importMcpServers", arg)?;
                    let result = service
                        .import_mcp_servers(&params)
                        .map_err(ChannelError::error)?;
                    Ok(json_value(&result))
                }
                other => Err(ChannelError::error(format!(
                    "mcp-sync channel: unknown command {other}"
                ))),
            }
        })
    }
}
