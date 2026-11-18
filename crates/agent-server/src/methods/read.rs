//! `session/read`：读会话快照（含消息投影）。
//!
//! 冷会话（无活跃 runtime）直接读 store，**不拉起 runtime**（控制器裁决 8）；
//! 热会话取 runtime 快照再附上 store 投影的消息。`messageLimit` 沿用 Node store
//! 语义：缺省=全量，`<=0` 归一为「不限」（store 保留 `Some(0) → 空` 的原始行为与
//! 钉住用例，归一在 handler——控制器裁决 4）。计数不按历史重算（Task 0 观察）。

use agent_protocol::session_read::SessionReadParams;
use serde_json::Value;

use crate::server::ServerError;

use super::{parse_params, session_not_found, store_error};

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionReadParams = parse_params(crate::methods::SESSION_READ_METHOD, params)?;
    let store = server.store()?;
    let record = store
        .get_session(&params.session_id)
        .map_err(store_error)?
        .ok_or_else(|| session_not_found(&params.session_id))?;

    let message_limit = params
        .message_limit
        .filter(|limit| *limit > 0)
        .map(|limit| limit as usize);
    let messages = store
        .messages(&record.id, None, message_limit)
        .map_err(store_error)?;
    let projected = super::projection::project_messages(&record, messages)?;

    let snapshot = match server.find_runtime(&record.id) {
        Some(runtime) => {
            let mut snapshot = runtime.snapshot();
            snapshot.messages = projected;
            snapshot
        }
        None => {
            let (target, verifications, timeline) =
                crate::runtime::load_goal_projection(&store, &record.id).map_err(store_error)?;
            super::cold_snapshot(&record, projected, target, verifications, timeline)
        }
    };
    serde_json::to_value(snapshot).map_err(|error| ServerError::internal(error.to_string()))
}
