//! `session/close`：注销 runtime（不删任何持久化事实）。
//!
//! - 从注册表移除并释放 runtime 句柄；**运行中的 turn 先取消**（唯一终态由
//!   `run_turn` 发出），**已接受的排队输入照常 drain**——它们已经由 CommandInbox
//!   准入（AGENTS.md：已接受的 busy/running 输入由 runtime 串行 admission），
//!   close 不回滚准入，持久化语义按完成边界照常生效。
//! - `expectedPersistence` 是持久化意图标记（immediate/deferred）；C1 的写入路径
//!   只有完成边界一种，两种取值都按 immediate 处理，事实行不丢。
//! - close 幂等：会话不在注册表（冷会话/重复 close）也返回 `closed:true`；
//!   **绝不删除** session/message 行。

use agent_protocol::session_write::{SessionCloseParams, SessionCloseResult};
use serde_json::Value;

use crate::server::ServerError;

use super::parse_params;

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionCloseParams = parse_params(crate::methods::SESSION_CLOSE_METHOD, params)?;
    if let Some(runtime) = server.remove_runtime(&params.session_id) {
        // 运行中的 turn 尽快收口（取消令牌只作用于当前 turn；排队输入照常执行）。
        let _ = runtime.cancel_turn();
        drop(runtime);
    }
    serde_json::to_value(SessionCloseResult { closed: Some(true) })
        .map_err(|error| ServerError::internal(error.to_string()))
}
