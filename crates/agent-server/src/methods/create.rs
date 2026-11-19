//! `session/create`：严格参数 → 落 `session` 行 → 物化 runtime → 返回快照。
//!
//! 控制器裁决：
//! - 会话**没有可解析模型时允许创建**（裁决 7）；快照里 `session.model` /
//!   `settings.model.current` 缺省省略（不造空 provider）。
//! - `session.title` 在 create 时是**空串**（Task 0 观察：首条 send 后才写标题）。
//! - 同 id 重复 create（远程镜像重导形态）：已有 runtime 直接复用；冷行按 resume
//!   语义物化（§0a 的 runtime 顶替属 v4 面，C1 不复制）。close 后 drain 窗口内的
//!   同 id create 与 resume 同守卫：显式 `-32603 会话正在关闭，请稍后重试`
//!   （Task 7 复评裁决 2，见 `resume::materialize`）。

use std::sync::Arc;

use agent_core::ModelCapabilities;
use agent_protocol::message::MessageModel;
use agent_protocol::session::SessionCreateParams;
use serde_json::Value;
use session_store::SessionRecord;

use crate::runtime::SessionRuntime;
use crate::server::ServerError;

use super::resume;
use super::{mode_to_string, parse_params, runtime_config, runtime_error, store_error};

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionCreateParams = parse_params(crate::methods::SESSION_CREATE_METHOD, params)?;
    let store = server.store()?;

    let session_id = params
        .session_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("sess_{}", uuid::Uuid::new_v4()));

    // 已有行（显式 id 重导 / 重启后重建）：按行恢复；没有行：新建。
    let record = match store.get_session(&session_id).map_err(store_error)? {
        Some(record) => record,
        None => {
            let record = build_record(&session_id, &params);
            store.create_session(record.clone()).map_err(store_error)?;
            record
        }
    };

    let runtime = resume::materialize(
        server,
        record,
        params.mode.map(mode_to_string),
        params.model.as_ref(),
        params.thought_level.clone(),
        params.mcp_servers.clone(),
    )?;
    serde_json::to_value(runtime.snapshot())
        .map_err(|error| ServerError::internal(error.to_string()))
}

/// `session` 行的 C1 形状（观察值见 `agent-server-c1-contract.md` §3.2）：
/// `slug` 与 `id` 相同、title 空串、`permission` 为 `{"mode":…}`、`project_id`
/// 由 cwd 归一化派生（`agent_core::scope_id_for` 的同一口径）。
fn build_record(session_id: &str, params: &SessionCreateParams) -> SessionRecord {
    let directory = params.workspace.workspace_path.clone();
    let mode = params
        .mode
        .map(mode_to_string)
        .unwrap_or_else(|| "build".to_string());
    let now = now_ms();
    SessionRecord {
        id: session_id.to_string(),
        project_id: agent_core::scope_id_for(&directory),
        workspace_id: params.workspace.workspace_identity.clone(),
        parent_id: params.parent_session_id.clone(),
        slug: session_id.to_string(),
        directory: directory.clone(),
        path: Some(directory),
        title: String::new(),
        version: "0.0.0".to_string(),
        share_url: None,
        summary_additions: None,
        summary_deletions: None,
        summary_files: None,
        summary_diffs: None,
        revert: None,
        permission: Some(serde_json::json!({ "mode": mode }).to_string()),
        time_created: now,
        time_updated: now,
        time_compacting: None,
        time_archived: None,
        task_type: "interactive".to_string(),
        title_source: "first_input".to_string(),
        title_message_id: None,
        time_title_updated: None,
        trace_id: Some(uuid::Uuid::new_v4().to_string()),
    }
}

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// create/resume 共用的能力解析入口（hub 失败时退缺省能力，不阻断创建）。
pub(super) fn fallback_capabilities(
    server: &crate::dispatch::AgentServer,
    model: Option<&MessageModel>,
) -> ModelCapabilities {
    model
        .map(|model| {
            server
                .resolve_capabilities(&model.provider_id, &model.model_id)
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// 组装并注册 runtime 的公共入口（供 resume 调用；create 在落行后调用）。
///
/// Task 8 接线：连接级反向请求 hub 存在时，审批经
/// `ProtocolPermissionHandler` 走 `interaction/requestPermission`；
/// `allow_project` 规则落在**与 store 同一个库**（`store.db_path()`）；
/// 偏好/凭据钩子共用同一 hub。无 hub（不经 stdio 的构造）退回默认审批
/// （`AutoAllow`）且不发偏好请求。
pub(super) fn spawn_runtime(
    server: &crate::dispatch::AgentServer,
    record: SessionRecord,
    mode: &str,
    model: Option<&MessageModel>,
    thought_level: Option<String>,
    mcp_servers: Option<Vec<agent_protocol::session_create::McpServer>>,
) -> Result<Arc<SessionRuntime>, ServerError> {
    let store = server.store()?;
    let capabilities = fallback_capabilities(server, model);
    // `settings.model.available` 的目录投影（T6）：hub 目录解析失败退 `None`
    // （快照 `available:[]`），与能力的 fail-soft 同口径，不阻断创建。
    let model_catalog = model.and_then(|model| {
        server
            .provider_config()
            .provider_catalog(&model.provider_id)
            .ok()
    });
    let config = runtime_config(
        server.data_root(),
        mode,
        model,
        capabilities,
        thought_level,
        model_catalog,
    );
    let reverse = server.reverse_hub();
    // C4 T1：MCP 工具源只持有 server 清单；连接在首 turn 的阻塞线程上建立。
    let mcp = mcp_servers
        .filter(|servers| !servers.is_empty())
        .map(|servers| server.mcp_hub().session(servers));
    let wiring = crate::reverse::RuntimeWiring {
        permission_handler: reverse.as_ref().map(|hub| {
            Box::new(crate::reverse::ProtocolPermissionHandler::new(Arc::clone(
                hub,
            ))) as Box<dyn agent_core::PermissionHandler>
        }),
        rule_db_path: Some(store.db_path().to_path_buf()),
        reverse,
        mcp,
    };
    let runtime = Arc::new(
        SessionRuntime::new(record, store, server.provider_source(), config, wiring)
            .map_err(runtime_error)?,
    );
    server.insert_runtime(Arc::clone(&runtime));
    Ok(runtime)
}
