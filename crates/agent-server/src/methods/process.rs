//! `process/childProcesses`（C4 T1）：MCP 子进程清单。
//!
//! 观察（`agent-c4-contract.md` §1.1）：自定义 stdio server 报 `mcpSource:"custom"`；
//! http 连接不产生子进程行。oracle 对同一 server 会同时列出启动器与 server 两行，
//! 本实现直接托管 server 进程，因此每个连接一行（T7 对拍登记该差异）。

use agent_protocol::workspace::{
    ChildProcessesParams, ChildProcessesResult, PROCESS_CHILD_PROCESSES_METHOD,
};
use serde_json::Value;

use crate::dispatch::AgentServer;
use crate::server::ServerError;

use super::parse_params;

pub(crate) fn handle(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let _params: ChildProcessesParams = parse_params(PROCESS_CHILD_PROCESSES_METHOD, params)?;
    serde_json::to_value(ChildProcessesResult {
        processes: server.mcp_hub().child_processes(),
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}
