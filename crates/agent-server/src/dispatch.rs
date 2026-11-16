//! 方法分派。C1 后续任务在 [`AgentServer::dispatch`] 里接入 session 方法表；
//! 本任务只落空壳，任何方法都显式返回 `-32601`，不返回空结果。

use serde_json::Value;

use crate::server::ServerError;

/// Agent Server 实例。当前不持有状态；后续任务在这里加入会话注册表与存储门。
#[derive(Debug, Default)]
pub struct AgentServer;

impl AgentServer {
    pub fn new() -> Self {
        Self
    }

    /// 按方法名分派一次请求。
    ///
    /// 骨架阶段没有已实现的方法，未知方法统一 `-32601 Method not found`；
    /// 后续任务按 spec §5.1 的方法表逐项接入，未支持的方法仍走这里。
    pub fn dispatch(&self, method: &str, _params: Value) -> Result<Value, ServerError> {
        Err(ServerError::method_not_found(method))
    }
}
