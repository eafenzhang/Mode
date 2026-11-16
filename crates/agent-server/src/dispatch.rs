//! 方法分派。Task 4b 接入 `provider/updateAccountConfig`（账号 Provider 配置交付，
//! 由 [`crate::provider_config::ProviderConfigHub`] 处理）；session 方法归 Task 7，
//! 其余方法显式返回 `-32601`，不返回空结果。

use serde_json::Value;

use crate::provider_config::{ProviderConfigHub, PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD};
use crate::server::ServerError;

/// Agent Server 实例。C1 按任务逐步接入状态：provider 配置 hub（Task 4b）、
/// 会话注册表与存储门（Task 7）。
#[derive(Debug, Default)]
pub struct AgentServer {
    provider_config: ProviderConfigHub,
}

impl AgentServer {
    pub fn new() -> Self {
        Self::default()
    }

    /// 按方法名分派一次请求。
    ///
    /// 已接入：`provider/updateAccountConfig`。未知/未实现方法统一
    /// `-32601 Method not found`；后续任务按 spec §5.1 的方法表逐项接入，
    /// 未支持的方法仍走这里。
    pub fn dispatch(&self, method: &str, params: Value) -> Result<Value, ServerError> {
        match method {
            PROVIDER_UPDATE_ACCOUNT_CONFIG_METHOD => self
                .provider_config
                .handle_account_config(params)
                .map_err(ServerError::from),
            _ => Err(ServerError::method_not_found(method)),
        }
    }

    /// provider 配置 hub；Task 7 的 session 方法经它把模型选择解析成可用端点。
    pub fn provider_config(&self) -> &ProviderConfigHub {
        &self.provider_config
    }
}
