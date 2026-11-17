//! `session/compact`：严格参数 → 受理判定（revision / 单飞 / prompt 运行中）→
//! 立即应答（异步执行）。
//!
//! 观察契约：`docs/specs/agent-c2-contract.md` §1。
//!
//! - 正常受理：`{response:"", snapshot, compact:{state:"accepted", inputId}}`——
//!   `response` 恒空串，摘要/失败经后续 `turn.*` 事件表达（异步受理）；
//! - 已有 compact 在执行：`state:"already_running"`（不动 revision、不入队）；
//! - `expectedRevision` 冲突：`-32009` + `{actualRevision, expectedRevision}`，
//!   零事件零落库（受理判定在 runtime 的同一把锁里先于入队）；
//! - prompt 正在执行：`-32010 Cannot compact while a prompt is running`。
//! - `compact.operationId` 未观察到 → 恒省略；`compact.inputId` 回显请求值
//!   （未提供时回显运行时生成的生效 id）。
//! - 应答 `snapshot` 取自受理时刻的运行时状态：**可能是 compact 前的快照**
//!   （accepted 应答在 worker 进入执行之前构造，见 `SessionRuntime::snapshot_locked`）。

use agent_protocol::session_write::{
    SessionCompactInfo, SessionCompactParams, SessionCompactResult, SessionCompactState,
};
use serde_json::{json, Value};

use crate::runtime::{CompactAccept, CompactReject};
use crate::server::ServerError;

use super::parse_params;

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let method = crate::methods::SESSION_COMPACT_METHOD;
    let params: SessionCompactParams = parse_params(method, params)?;
    let runtime = server.runtime(&params.session_id)?;
    let (accept, snapshot) = runtime.request_compact(&params).map_err(compact_reject)?;
    let (state, input_id) = match accept {
        CompactAccept::Accepted { input_id } => (SessionCompactState::Accepted, input_id),
        CompactAccept::AlreadyRunning { input_id } => {
            (SessionCompactState::AlreadyRunning, input_id)
        }
    };
    serde_json::to_value(SessionCompactResult {
        response: String::new(),
        snapshot,
        compact: Some(SessionCompactInfo {
            state,
            input_id: Some(input_id),
            // `compact.operationId` 未观察到（契约 §1.1）：恒省略。
            operation_id: None,
        }),
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

/// 受理拒绝 → JSON-RPC（码与 `data` 逐字对齐观察值，契约 §1.1）。
fn compact_reject(error: CompactReject) -> ServerError {
    match error {
        CompactReject::RevisionConflict { expected, current } => ServerError::rpc(
            -32009,
            "Session state revision mismatch",
            Some(json!({
                "actualRevision": current,
                "expectedRevision": expected,
            })),
        ),
        CompactReject::PromptRunning => {
            ServerError::rpc(-32010, "Cannot compact while a prompt is running", None)
        }
        CompactReject::RuntimeClosed => ServerError::internal("会话运行时已关闭"),
    }
}
