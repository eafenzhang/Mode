//! `session/send`：严格参数（含 superRefine 组合约束）→ runtime 串行准入 → 立即回执。
//!
//! 回执 `{accepted:true, sessionId, stateRevision}` 在**接受即返回**（不等 turn）；
//! turn 的失败（含缺模型的 `CONFIGURATION_ERROR`）走事件流的 `turn.failed`，
//! 不在这里同步报错（oracle 行为：缺 model → turn.failed，见
//! `docs/specs/tauri-agent-runtime.md` §1.5）。`expectedRevision` 冲突显式 `-32602`
//! 且**不拉起 turn**。

use agent_protocol::session::SessionSendParams;
use agent_protocol::session::SessionSendResult;
use serde_json::Value;

use crate::server::ServerError;
use crate::state::SessionInput;

use super::{parse_params, runtime_error};

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let method = crate::methods::SESSION_SEND_METHOD;
    let params: SessionSendParams = parse_params(method, params)?;
    // 协议 superRefine 的三条组合约束（automation/offPeak 互斥等）在发出前校验。
    params.validate().map_err(|message| {
        ServerError::invalid_params(format!("{method} 参数不合法: {message}"))
    })?;

    let runtime = server.runtime(&params.session_id)?;
    let accepted = runtime
        .submit(SessionInput {
            input_id: params.input_id,
            content: params.content,
            expected_revision: params.expected_revision,
        })
        .map_err(runtime_error)?;
    serde_json::to_value(SessionSendResult {
        accepted: true,
        session_id: params.session_id,
        state_revision: accepted.state_revision,
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}
