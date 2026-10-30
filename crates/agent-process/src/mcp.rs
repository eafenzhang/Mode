//! `mcp/list` facade：workspace MCP 状态。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的 `listMcpServerStatuses`。
//!
//! 差异：TS 使用独立的 mcp-status 泳道 client，并在旧 Agent 不认识 `mode`/`mcpServers`
//! 时按 `unrecognized_keys` 做兼容重试；本 facade 复用 workspace runtime，且不做该重试
//! （错误原样上抛，由服务层决定）。

use agent_protocol::mcp::{McpListMode, McpListParams, McpListResult, MCP_LIST_METHOD};
use agent_protocol::session_create::McpServer;

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};
use crate::session_read::session_workspace_ref;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListMcpServersOptions {
    pub mcp_servers: Option<Vec<McpServer>>,
    pub mode: Option<McpListMode>,
}

impl AgentControlClient {
    /// `mcp/list`：返回按 serverName 索引的状态快照。
    pub async fn list_mcp_server_statuses(
        &self,
        target: &AgentWorkspaceTarget,
        options: ListMcpServersOptions,
    ) -> Result<McpListResult, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let params = McpListParams {
            workspace: session_workspace_ref(target),
            mcp_servers: options.mcp_servers,
            mode: options.mode,
        };
        let params = serde_json::to_value(&params).expect("mcp/list params 序列化不会失败");
        let response = process
            .client()
            .request(MCP_LIST_METHOD, Some(params))
            .await?;
        serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid mcp/list result: {error}"))
        })
    }
}
