//! `skills/referenceCatalog`（C4 T2）：workspace/session authority 的技能目录。
//!
//! 观察（`agent-c4-contract.md` §2.1）：
//! - 不带 `sessionId` → `authority:"workspace"`；带活动 session → `authority:"session"`；
//! - 未知/非活动 session → `-32004 Session is not active: <id>`；
//! - 条目只含 `enabled:true`（协议是 `z.literal(true)`）。

use agent_protocol::skills::{
    SkillReferenceAuthority, SkillsReferenceCatalogParams, SKILLS_REFERENCE_CATALOG_METHOD,
};
use serde_json::Value;

use crate::dispatch::AgentServer;
use crate::server::ServerError;
use crate::skills::SkillCatalog;

use super::{parse_params, store_error};

pub(crate) fn handle(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: SkillsReferenceCatalogParams =
        parse_params(SKILLS_REFERENCE_CATALOG_METHOD, params)?;
    let authority = match &params.session_id {
        Some(session_id) => {
            let store = server.store()?;
            let exists = store
                .get_session(session_id)
                .map_err(store_error)?
                .is_some();
            if !exists {
                return Err(ServerError::rpc(
                    -32004,
                    format!("Session is not active: {session_id}"),
                    None,
                ));
            }
            SkillReferenceAuthority::Session
        }
        None => SkillReferenceAuthority::Workspace,
    };
    let catalog = SkillCatalog::from_process();
    let result = catalog
        .catalog(&params.workspace.workspace_path, authority)
        .map_err(ServerError::internal)?;
    serde_json::to_value(result).map_err(|error| ServerError::internal(error.to_string()))
}
