//! `session/goal`：严格参数 → runtime 受理/只读 → `{response, snapshot, startedTurn}`。
//!
//! 行为依据：`docs/specs/agent-c3-contract.md` §1（黑盒观察）。
//!
//! - 六种 action 的响应文案与 `startedTurn` 由 [`crate::runtime::SessionRuntime::request_goal`]
//!   决定；本模块只做参数解析、错误码映射与线格式投影；
//! - revision 冲突 → `-32009` + `{actualRevision, expectedRevision}`（零副作用）；
//! - prompt/continuation 运行中（除 `pause`）→ `-32010 Cannot manage goals while a
//!   prompt is running`；
//! - `startedTurn` 三键恒在（观察：false 也出现）。
//!
//! `session/stop` 同时在 C3 T1 落地（用户中断续跑的最小面）：参数 `{sessionId}`，
//! 应答 `{}`；目标暂停由 [`crate::runtime::SessionRuntime::cancel_turn`] 收口。

use agent_protocol::session_write::{
    SessionGoalParams, SessionGoalResult, SessionStopParams, SESSION_STOP_METHOD,
};
use serde_json::{json, Value};

use crate::runtime::GoalReject;
use crate::server::ServerError;

use super::parse_params;

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionGoalParams = parse_params(crate::methods::SESSION_GOAL_METHOD, params)?;
    let runtime = server.runtime(&params.session_id)?;
    let outcome = runtime.request_goal(&params).map_err(goal_reject)?;
    serde_json::to_value(SessionGoalResult {
        response: outcome.response,
        snapshot: outcome.snapshot,
        started_turn: Some(outcome.started_turn),
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

/// `session/stop`：取消当前 turn（goal 目标随之 paused）；应答 `{}`。
pub(crate) fn handle_stop(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionStopParams = parse_params(SESSION_STOP_METHOD, params)?;
    if let Some(runtime) = server.find_runtime(&params.session_id) {
        runtime
            .cancel_turn()
            .map_err(|error| ServerError::internal(error.to_string()))?;
    }
    Ok(json!({}))
}

/// 受理拒绝 → JSON-RPC（码与 `data` 对齐观察值）。
fn goal_reject(error: GoalReject) -> ServerError {
    match error {
        GoalReject::RevisionConflict { expected, current } => ServerError::rpc(
            -32009,
            "Session state revision mismatch",
            Some(json!({
                "actualRevision": current,
                "expectedRevision": expected,
            })),
        ),
        GoalReject::PromptRunning => ServerError::rpc(
            -32010,
            "Cannot manage goals while a prompt is running",
            None,
        ),
        GoalReject::Store(message) => ServerError::internal(format!("会话库失败：{message}")),
        GoalReject::RuntimeClosed => ServerError::internal("会话运行时已关闭"),
    }
}
