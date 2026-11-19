//! `session/resume`：冷恢复（无活跃 runtime）从 store 重建 runtime；热会话直接复用。
//!
//! 模型/模式事实的恢复走 `session_entry`（Task 0 观察 §3.6：`runtime/model_selection`
//! 与 `runtime-execution-state`，两种分隔符都是线上事实，不归一）。没有 entry 的旧
//! 会话回退到 params 的迁移 hint（`thoughtLevel`），模型缺失则保持「无模型」状态——
//! send 时以 `CONFIGURATION_ERROR` 显式失败（控制器裁决 7），不编默认模型。
//!
//! **drain 窗口**（Task 7 复评裁决 2）：close 后已接受的输入仍由旧 runtime drain；
//! 窗口内的 resume（以及同 id `session/create`，共用 [`materialize`]）显式报
//! `-32603 会话正在关闭，请稍后重试`，不隐式重挂 drain 中的 runtime——否则会出现
//! 两个 runtime owner，各自按 `max(sequence)+1` 续号产生重复序号。drain 完成后
//! `closing` 台账条目被清理任务移除，resume 恢复可用。

use std::sync::Arc;

use agent_protocol::message::{MessageModel, ModelOptions};
use agent_protocol::session_write::SessionResumeParams;
use serde_json::Value;
use session_store::{SessionEntryRecord, SessionRecord};

use crate::runtime::SessionRuntime;
use crate::server::ServerError;

use super::create;
use super::{parse_params, session_not_found, store_error};

/// `runtime/model_selection` 的 DB 行 id（注意 `:runtime-` 连字符分隔符，线上事实）。
const MODEL_SELECTION_ENTRY_ID: &str = "runtime-model-selection";
const MODEL_SELECTION_ENTRY_TYPE: &str = "runtime/model_selection";
/// `runtime-execution-state` 用 `:runtime:` 冒号分隔符（同为线上事实）。
const EXECUTION_STATE_ENTRY_ID: &str = "runtime-execution-state";
const EXECUTION_STATE_ENTRY_TYPE: &str = "runtime/execution_state";

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionResumeParams = parse_params(crate::methods::SESSION_RESUME_METHOD, params)?;
    let store = server.store()?;
    let record = store
        .get_session(&params.session_id)
        .map_err(store_error)?
        .ok_or_else(|| session_not_found(&params.session_id))?;

    let runtime = materialize(
        server,
        record.clone(),
        None,
        None,
        params.thought_level.clone(),
        params.mcp_servers.clone(),
    )?;

    let messages = store
        .messages(&record.id, None, None)
        .map_err(store_error)?;
    let mut snapshot = runtime.snapshot();
    snapshot.messages = super::projection::project_messages(&record, messages)?;
    // 观察（`agent-c3-contract.md` §1.8）：resume 应答比 create/read 多 `goalStats`。
    snapshot.goal_stats = runtime.goal_stats();
    serde_json::to_value(snapshot).map_err(|error| ServerError::internal(error.to_string()))
}

/// 取/物化会话 runtime（create 的「已有行」路径与 resume 共用）。
///
/// - 注册表里已有：直接复用（不二次 spawn，不重置事件与队列）。
/// - 冷会话：entry 事实优先，params 作 fallback；物化后 upsert entry（幂等）。
pub(super) fn materialize(
    server: &crate::dispatch::AgentServer,
    record: SessionRecord,
    fallback_mode: Option<String>,
    fallback_model: Option<&MessageModel>,
    fallback_thought: Option<String>,
    mcp_servers: Option<Vec<agent_protocol::session_create::McpServer>>,
) -> Result<Arc<SessionRuntime>, ServerError> {
    if let Some(resident) = server.find_runtime(&record.id) {
        return Ok(resident);
    }
    // drain 窗口守卫（Task 7 复评裁决 2）：close 后已接受的输入仍在旧 runtime 里
    // drain，此时再物化会出现第二个 runtime owner（两条写路径、重复 sequence）。
    // 显式报错而不是隐式重挂；drain 完成后台账已清，此后走冷恢复。
    if let Some(watcher) = server.find_closing(&record.id) {
        if *watcher.borrow() {
            // 信号已到但清理任务尚未轮到：视为 drain 完成，按冷恢复继续。
            server.remove_closing(&record.id);
        } else {
            return Err(ServerError::internal("会话正在关闭，请稍后重试"));
        }
    }
    let store = server.store()?;
    let entries = store.entries(&record.id).map_err(store_error)?;
    let entry_model = entries
        .iter()
        .find(|entry| entry.entry_type == MODEL_SELECTION_ENTRY_TYPE)
        .and_then(|entry| entry.data.get("modelSelection"))
        .and_then(model_from_value);
    let entry_mode = entries
        .iter()
        .find(|entry| entry.entry_type == EXECUTION_STATE_ENTRY_TYPE)
        .and_then(|entry| entry.data.get("mode"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let model = entry_model.or_else(|| fallback_model.cloned());
    let mode = entry_mode
        .or(fallback_mode)
        .unwrap_or_else(|| "build".to_string());
    // thoughtLevel 恢复：params 优先；没有时回退模型选择 entry 的
    // `options.reasoningLevel`（已持久化事实；oracle 的真实恢复源未观察，
    // 观察值两处一致——`agent-c2-contract.md` §4.5 登记）。
    let thought = fallback_thought.or_else(|| {
        model
            .as_ref()
            .and_then(|model| model.options.as_ref())
            .and_then(|options| options.reasoning_level.clone())
    });
    let runtime = create::spawn_runtime(
        server,
        record.clone(),
        &mode,
        model.as_ref(),
        thought,
        mcp_servers,
    )?;

    // 物化后把生效事实写回 entry（幂等 upsert）：重启后 resume 可再次恢复。
    let now = create::now_ms();
    if let Some(model) = &model {
        put_model_selection(&store, &record.id, model, now)?;
    }
    put_execution_state(&store, &record.id, &mode, now)?;
    Ok(runtime)
}

pub(super) fn put_model_selection(
    store: &session_store::SessionStore,
    session_id: &str,
    model: &MessageModel,
    now: i64,
) -> Result<(), ServerError> {
    let mut selection = serde_json::json!({
        "providerId": model.provider_id,
        "modelId": model.model_id,
    });
    if let Some(options) = &model.options {
        if let Some(level) = &options.reasoning_level {
            selection["options"] = serde_json::json!({ "reasoningLevel": level });
        }
    }
    store
        .put_entry(SessionEntryRecord {
            id: format!("{session_id}:{MODEL_SELECTION_ENTRY_ID}"),
            session_id: session_id.to_string(),
            entry_type: MODEL_SELECTION_ENTRY_TYPE.to_string(),
            time_created: now,
            time_updated: now,
            data: serde_json::json!({ "modelSelection": selection }),
        })
        .map_err(store_error)
}

pub(super) fn put_execution_state(
    store: &session_store::SessionStore,
    session_id: &str,
    mode: &str,
    now: i64,
) -> Result<(), ServerError> {
    store
        .put_entry(SessionEntryRecord {
            id: format!("{session_id}:{EXECUTION_STATE_ENTRY_ID}"),
            session_id: session_id.to_string(),
            entry_type: EXECUTION_STATE_ENTRY_TYPE.to_string(),
            time_created: now,
            time_updated: now,
            data: serde_json::json!({ "mode": mode, "planEnabled": false }),
        })
        .map_err(store_error)
}

/// entry 的 `modelSelection` → `MessageModel`（严格字段，缺失放弃）。
fn model_from_value(value: &Value) -> Option<MessageModel> {
    let provider_id = value.get("providerId").and_then(Value::as_str)?;
    let model_id = value.get("modelId").and_then(Value::as_str)?;
    if provider_id.is_empty() || model_id.is_empty() {
        return None;
    }
    Some(MessageModel {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
        options: value
            .get("options")
            .and_then(|options| options.get("reasoningLevel"))
            .and_then(Value::as_str)
            .map(|level| ModelOptions {
                reasoning_level: Some(level.to_string()),
            }),
    })
}
