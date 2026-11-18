//! `session/list`：按 workspace（`directory`）列会话。
//!
//! 控制器裁决 5：workspace 查询键**只有** `workspace.workspacePath`（= store 的
//! `directory` 列）；`project_id` 是派生 scope，不是 workspace key。params 不带
//! workspace 时列出全部会话（Task 0 观察：oracle 的 `session/list` params 可以
//! 只有 `{includeArchived}`）。`sessionIds` 在 handler 侧过滤；`includeArchived`
//! 恒有效（false 排除归档行）。

use agent_protocol::session_read::{SessionListParams, SessionListResult};
use serde_json::Value;

use crate::server::ServerError;

use super::{parse_params, store_error};

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionListParams = parse_params(crate::methods::SESSION_LIST_METHOD, params)?;
    let store = server.store()?;

    let limit = params.limit.map(|limit| limit as usize);
    // `sessionIds` 是 handler 侧过滤：先过滤再截断，否则 SQL 的 LIMIT 可能先把目标
    // 会话切掉，返回空列表（最终评审 Minor）。没有 sessionIds 时才让 SQL 直接 LIMIT。
    let query_limit = if params.session_ids.is_some() {
        None
    } else {
        limit
    };
    let records = match &params.workspace {
        Some(workspace) => store
            .list_sessions(
                &workspace.workspace_path,
                params.include_archived,
                query_limit,
            )
            .map_err(store_error)?,
        None => store
            .list_all_sessions(params.include_archived, query_limit)
            .map_err(store_error)?,
    };

    let mut sessions: Vec<_> = records
        .iter()
        .map(|record| {
            super::session_info(record, agent_protocol::session_read::SessionStatus::Idle)
        })
        .collect();
    if let Some(session_ids) = &params.session_ids {
        sessions.retain(|session| session_ids.contains(&session.session_id));
        if let Some(limit) = limit {
            sessions.truncate(limit);
        }
    }
    serde_json::to_value(SessionListResult { sessions })
        .map_err(|error| ServerError::internal(error.to_string()))
}
