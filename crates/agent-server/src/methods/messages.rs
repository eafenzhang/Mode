//! `session/messages`：消息分页读（wire 投影，不回写 DB）。
//!
//! `limit` 语义（控制器裁决 4）：缺省=全量；`<=0` 在 handler 归一为「不限」
//! （Node store 语义；store 的 `Some(0) → 空` 原始行为由 session-store 的钉住
//! 用例保留）。`afterMessageId` 游标不存在时 store 报错 → `-32602`。

use agent_protocol::session_read::{SessionMessagesParams, SessionMessagesResult};
use serde_json::Value;

use crate::server::ServerError;

use super::{parse_params, session_not_found, store_error};

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionMessagesParams =
        parse_params(crate::methods::SESSION_MESSAGES_METHOD, params)?;
    let store = server.store()?;
    let record = store
        .get_session(&params.session_id)
        .map_err(store_error)?
        .ok_or_else(|| session_not_found(&params.session_id))?;

    // `limit <= 0` 归一为「不限」（Node store 语义；裁决 4）。
    let limit = params
        .limit
        .filter(|limit| *limit > 0)
        .map(|limit| limit as usize);
    let messages = store
        .messages(&record.id, params.after_message_id.as_deref(), limit)
        .map_err(store_error)?;
    let result = SessionMessagesResult {
        messages: super::projection::project_messages(&record, messages)?,
    };
    serde_json::to_value(result).map_err(|error| ServerError::internal(error.to_string()))
}
