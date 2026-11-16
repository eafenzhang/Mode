//! 三个 setter：`session/setModel` / `session/setThoughtLevel` / `session/setMode`。
//!
//! 共同语义：严格参数 →（可选）`expectedRevision` 校验 → 原位更新 runtime 配置
//! （worker 在下个 turn 前应用到 `Session`）→ 需要持久化的模型/模式事实 upsert
//! `session_entry`（观察形状 §3.6）→ 返回**完整状态快照**。
//!
//! `setModel` 经 hub 解析（能力数字来自模型规则）；解析失败显式 `-32603` 且
//! **不改动**内存事实（控制器裁决 7：setModel 的失败面在这里，不像 create 那样
//! 宽容）。setter 要求会话在注册表里（冷会话先 resume）。

use agent_protocol::session_write::{
    SessionSetModeParams, SessionSetModelParams, SessionSetThoughtLevelParams,
};
use serde_json::Value;

use crate::server::ServerError;

use super::resume;
use super::{mode_to_string, parse_params, runtime_error};

pub(crate) fn handle_set_model(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionSetModelParams =
        parse_params(crate::methods::SESSION_SET_MODEL_METHOD, params)?;
    let runtime = server.runtime(&params.session_id)?;

    let capabilities =
        server.resolve_capabilities(&params.model.provider_id, &params.model.model_id)?;
    let model = params.model.clone();
    runtime
        .update_config(params.expected_revision, |config| {
            config.provider_id = model.provider_id.clone();
            config.model_id = model.model_id.clone();
            config.capabilities = capabilities;
        })
        .map_err(runtime_error)?;

    let store = server.store()?;
    resume::put_model_selection(&store, &params.session_id, &params.model, now_ms())?;
    snapshot_result(&runtime)
}

pub(crate) fn handle_set_thought_level(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionSetThoughtLevelParams =
        parse_params(crate::methods::SESSION_SET_THOUGHT_LEVEL_METHOD, params)?;
    let runtime = server.runtime(&params.session_id)?;
    let level = params.thought_level.clone();
    runtime
        .update_config(params.expected_revision, |config| {
            config.thought_level = level;
        })
        .map_err(runtime_error)?;
    snapshot_result(&runtime)
}

pub(crate) fn handle_set_mode(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionSetModeParams =
        parse_params(crate::methods::SESSION_SET_MODE_METHOD, params)?;
    let runtime = server.runtime(&params.session_id)?;
    let mode = mode_to_string(params.mode);
    runtime
        .update_config(params.expected_revision, |config| {
            config.mode = mode.clone();
        })
        .map_err(runtime_error)?;

    let store = server.store()?;
    resume::put_execution_state(&store, &params.session_id, &mode, now_ms())?;
    snapshot_result(&runtime)
}

fn snapshot_result(runtime: &crate::runtime::SessionRuntime) -> Result<Value, ServerError> {
    serde_json::to_value(runtime.snapshot())
        .map_err(|error| ServerError::internal(error.to_string()))
}

/// `persistAsWorkspaceLastUsed`：C1 没有 workspace 级「last used」事实源，
/// 忽略该标记（请求形状保持严格解析）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
