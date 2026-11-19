//! `mcp/list`（C4 T1）：连接状态机与失败诊断。
//!
//! 观察（`agent-c4-contract.md` §1）：
//! - `mode:"status"` 只读，不建连；未连接的 server 报 `disconnected`；
//! - `mode:"connect"` 幂等建连，成功报 `connected/protocolEra`，失败报
//!   `failed/failureKind/error`；
//! - 不传 `mcpServers` 时 statuses 为空。

use agent_protocol::mcp::{McpListMode, McpListParams, McpListResult, MCP_LIST_METHOD};
use serde_json::Value;

use crate::dispatch::AgentServer;
use crate::server::ServerError;

use super::parse_params;

pub(crate) fn handle(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: McpListParams = parse_params(MCP_LIST_METHOD, params)?;
    let servers = params.mcp_servers.clone().unwrap_or_default();
    let mode = params.mode.unwrap_or(McpListMode::Connect);
    if mode == McpListMode::Connect {
        for config in &servers {
            server.mcp_hub().connect(config);
        }
    }
    let statuses = server.mcp_hub().statuses(&servers);
    serde_json::to_value(McpListResult { statuses })
        .map_err(|error| ServerError::internal(error.to_string()))
}
