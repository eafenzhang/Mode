//! `initialize`：Host 启动预热，返回可用性与协议身份。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的 `initialize`。
//! 差异：TS 的 `getClient` 还会等 provider/model readiness；本层只做拉起（storage gate
//! 由后续请求/`prepare_storage` 驱动），provider 未就绪由调用方按 `available=false` 处理。
//! 失败不抛错：返回 `available: false` + `reason`，与 TS 的首屏可恢复语义一致。

use crate::control::{AgentControlClient, AgentWorkspaceTarget, RuntimePolicy};

pub const MODE_PROTOCOL_NAME: &str = "Mode Protocol";
pub const MODE_PROTOCOL_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInitializeResult {
    pub available: bool,
    pub workspace_key: String,
    pub protocol_name: String,
    pub protocol_version: u64,
    pub transport_kind: Option<String>,
    pub reason: Option<String>,
    pub reason_code: Option<String>,
}

impl AgentControlClient {
    /// 拉起（或复用）该 workspace 的 Agent；失败折叠为 `available=false`。
    pub async fn initialize(&self, target: &AgentWorkspaceTarget) -> AgentInitializeResult {
        let workspace_key = target.workspace_key();
        match self.client_for(target, RuntimePolicy::StartIfNeeded).await {
            Ok(_) => AgentInitializeResult {
                available: true,
                workspace_key,
                protocol_name: MODE_PROTOCOL_NAME.to_string(),
                protocol_version: MODE_PROTOCOL_VERSION,
                transport_kind: Some("stdio".to_string()),
                reason: None,
                reason_code: None,
            },
            Err(error) => AgentInitializeResult {
                available: false,
                workspace_key,
                protocol_name: MODE_PROTOCOL_NAME.to_string(),
                protocol_version: MODE_PROTOCOL_VERSION,
                transport_kind: None,
                reason: Some(error.to_string()),
                reason_code: None,
            },
        }
    }
}
