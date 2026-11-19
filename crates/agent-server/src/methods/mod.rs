//! session 方法表（LT-1 C1 Task 7）。
//!
//! 每个 handler 只做三件事：**严格解析参数**（多一个未知键 `-32602`）→ 调
//! runtime / store → 投影结果。turn 语义（准入、串行、事件、持久化边界）全部归
//! [`crate::runtime::SessionRuntime`]，这里不复制；消息/会话行的线格式投影归
//! [`projection`]（DB 富形状 ≠ wire 投影，见 `docs/specs/agent-server-c1-contract.md`
//! §3.7）。
//!
//! 方法清单（控制器裁决 3 + C2 T2 + C3 T1/T4）：`session/create|send|subscribe|read|messages|
//! list|close|resume|stop|setModel|setThoughtLevel|setMode|compact|goal|subagents`。

pub mod close;
pub mod compact;
pub mod create;
pub mod goal;
pub mod list;
pub mod mcp;
pub mod messages;
pub mod plugins;
pub mod process;
pub mod projection;
pub mod read;
pub mod resume;
pub mod send;
pub mod setters;
pub mod skills;
pub mod subagents;
pub mod subscribe;
pub mod workflows;

use agent_core::ModelCapabilities;
use agent_protocol::message::MessageModel;
use agent_protocol::session_read::{
    SessionInfo, SessionKind, SessionMode, SessionStatus, SessionTitleSource, SessionWorkspaceRef,
};
use agent_protocol::snapshot::{
    PermissionSettings, ProtocolInfo, ProtocolName, SessionModeSettings, SessionModelSettings,
    SessionProjection, SessionRuntimeState, SessionSettings, SessionStateSnapshot,
    ThoughtLevelOption, ThoughtLevelSettings,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use session_store::SessionRecord;

use crate::server::ServerError;
use crate::state::{RuntimeConfig, RuntimeError};

pub use agent_protocol::mcp::MCP_LIST_METHOD;
pub use agent_protocol::plugin::{
    PLUGINS_CANCEL_OPERATION_METHOD, PLUGINS_DESCRIBE_METHOD, PLUGINS_INSTALL_METHOD,
    PLUGINS_LIST_METHOD, PLUGINS_MARKETPLACE_ADD_METHOD, PLUGINS_MARKETPLACE_REMOVE_METHOD,
    PLUGINS_MARKETPLACE_UPDATE_METHOD, PLUGINS_OVERVIEW_METHOD, PLUGINS_REFERENCE_CATALOG_METHOD,
    PLUGINS_REFERENCE_CATALOG_WITH_CATEGORY_METHOD, PLUGINS_RESOLVE_SUGGESTED_REFERENCE_METHOD,
    PLUGINS_SET_ENABLED_METHOD, PLUGINS_UNINSTALL_METHOD, PLUGINS_UPDATE_METHOD,
};
pub use agent_protocol::session_read::{
    SESSION_LIST_METHOD, SESSION_MESSAGES_METHOD, SESSION_READ_METHOD, SESSION_SUBAGENTS_METHOD,
    SESSION_SUBSCRIBE_METHOD,
};
pub use agent_protocol::session_send::SESSION_SEND_METHOD;
pub use agent_protocol::session_write::{
    SESSION_CLOSE_METHOD, SESSION_COMPACT_METHOD, SESSION_CREATE_METHOD, SESSION_GOAL_METHOD,
    SESSION_RESUME_METHOD, SESSION_SET_MODEL_METHOD, SESSION_SET_MODE_METHOD,
    SESSION_SET_THOUGHT_LEVEL_METHOD, SESSION_STOP_METHOD,
};
pub use agent_protocol::skills::SKILLS_REFERENCE_CATALOG_METHOD;
pub use agent_protocol::workflow::{
    WORKFLOWS_DELETE_METHOD, WORKFLOWS_GET_METHOD, WORKFLOWS_LIST_METHOD, WORKFLOWS_MOVE_METHOD,
    WORKFLOWS_RUNS_METHOD, WORKFLOWS_UPDATE_META_METHOD,
};
pub use agent_protocol::workspace::PROCESS_CHILD_PROCESSES_METHOD;

/// 严格解析 params：未知键 / 类型不符一律 `-32602`，不静默吞掉。
pub(crate) fn parse_params<T: DeserializeOwned>(
    method: &str,
    params: Value,
) -> Result<T, ServerError> {
    serde_json::from_value(params)
        .map_err(|error| ServerError::invalid_params(format!("{method} 参数不合法: {error}")))
}

/// `RuntimeError` → JSON-RPC：revision 冲突与非法输入是 `-32602`（提交时参数已过期 /
/// 不合法），store / runtime 关闭是 `-32603`。
pub(crate) fn runtime_error(error: RuntimeError) -> ServerError {
    match &error {
        RuntimeError::RevisionConflict { expected, current } => {
            ServerError::invalid_params(format!(
            "stateRevision 冲突：expected {expected}，当前 {current}（输入未接受，turn 未启动）"
        ))
        }
        RuntimeError::InvalidInput(message) => {
            ServerError::invalid_params(format!("无效的会话输入：{message}"))
        }
        RuntimeError::Store(message) => ServerError::internal(format!("会话库失败：{message}")),
        RuntimeError::RuntimeClosed => ServerError::internal("会话运行时已关闭"),
    }
}

/// `session-store` 读错误的映射：游标不存在等调用方错误 `-32602`，其余 `-32603`。
pub(crate) fn store_error(error: session_store::StoreError) -> ServerError {
    match &error {
        session_store::StoreError::InvalidInput(_) => {
            ServerError::invalid_params(error.to_string())
        }
        _ => ServerError::internal(error.to_string()),
    }
}

/// 会话不存在（store 无行）。
pub(crate) fn session_not_found(session_id: &str) -> ServerError {
    ServerError::invalid_params(format!("会话不存在: {session_id}"))
}

/// 协议 `SessionMode` → 运行时/DB 的 mode 字符串（未知档位按 `build`，与
/// `state::parse_mode` 的落点一致）。
pub(crate) fn mode_to_string(mode: agent_protocol::session_read::SessionMode) -> String {
    match mode {
        SessionMode::Plan => "plan".to_string(),
        SessionMode::Build => "build".to_string(),
        SessionMode::Edit => "edit".to_string(),
        SessionMode::Yolo => "yolo".to_string(),
        SessionMode::Auto => "auto".to_string(),
    }
}

/// 组装运行时配置（create / resume 共用）。
///
/// create 阶段允许模型不可解析（控制器裁决 7）：能力解析失败只放弃能力数字
/// （调用方传缺省），模型事实仍进配置——真正的失败在 send 的 turn
/// （`CONFIGURATION_ERROR`）或 setModel 上显式暴露。`model_catalog` 由调用方
/// 预先解析（fail-soft 退 `None`，快照 `available:[]`）。
pub(crate) fn runtime_config(
    data_root: Option<std::path::PathBuf>,
    mode: &str,
    model: Option<&MessageModel>,
    capabilities: ModelCapabilities,
    thought_level: Option<String>,
    model_catalog: Option<crate::provider_config::ProviderCatalog>,
) -> RuntimeConfig {
    let mut config = RuntimeConfig {
        mode: mode.to_string(),
        capabilities,
        thought_level,
        model_catalog,
        data_root,
        ..RuntimeConfig::default()
    };
    if let Some(model) = model {
        config.provider_id = model.provider_id.clone();
        config.model_id = model.model_id.clone();
        config.reasoning_level = model
            .options
            .as_ref()
            .and_then(|options| options.reasoning_level.clone());
    }
    config
}

/// DB `session` 行 → 协议 `SessionInfo`（list / 冷快照共用）。
pub(crate) fn session_info(record: &SessionRecord, status: SessionStatus) -> SessionInfo {
    SessionInfo {
        session_id: record.id.clone(),
        workspace: SessionWorkspaceRef {
            workspace_path: record.directory.clone(),
            workspace_identity: record.workspace_id.clone(),
            remote_session_id: None,
            workspace_key: crate::state::workspace_key(record),
        },
        parent_session_id: record.parent_id.clone(),
        trace_id: record.trace_id.clone(),
        session_kind: session_kind_from(&record.task_type),
        title: record.title.clone(),
        title_source: title_source_from(&record.title_source),
        mode: permission_mode(record),
        status,
        // store 不持有模型选择事实（entry 属 runtime 面），list 行不造模型。
        model: None,
        target: None,
        created_at: epoch_ms_to_u64(record.time_created),
        updated_at: epoch_ms_to_u64(record.time_updated),
        archived_at: record.time_archived.map(epoch_ms_to_u64),
    }
}

/// 冷快照（无活跃 runtime 的 `session/read`；不拉起 runtime——控制器裁决 8）。
///
/// 计数不按历史重算（Task 0 观察：oracle 的 resume/read 投影计数保持 0）；
/// `titleSource` 按库行投影（oracle 的 read/list 行带它）。顶层 9 键与热快照同面
/// （`slashCommands` builtin 数据契约、todos/todoGroups/goal 数组恒在）；`settings.model`
/// 不带事实（store 无模型选择行面，模型目录归热 runtime——契约 §4.5 登记）。
pub(crate) fn cold_snapshot(
    record: &SessionRecord,
    messages: Vec<agent_protocol::message::MessageWithParts>,
    target: Option<agent_protocol::session_read::SessionGoal>,
    goal_verifications: Vec<agent_protocol::snapshot::GoalVerification>,
    goal_verification_timeline: Vec<agent_protocol::snapshot::GoalVerificationTimeline>,
) -> SessionStateSnapshot {
    let mode = permission_mode(record);
    let mut info = session_info(record, SessionStatus::Idle);
    info.target = Some(target.clone());
    SessionStateSnapshot {
        protocol: ProtocolInfo {
            name: ProtocolName::ModeProtocol,
            version: 1,
        },
        session: info,
        settings: SessionSettings {
            model: SessionModelSettings {
                current: None,
                available: Vec::new(),
                last_used: None,
            },
            thought_level: ThoughtLevelSettings {
                enabled: false,
                current: None,
                default_level: None,
                available: Vec::<ThoughtLevelOption>::new(),
            },
            mode: SessionModeSettings { current: mode },
            permission: Some(PermissionSettings {
                mode: Some(mode),
                rules_revision: None,
            }),
        },
        projection: SessionProjection {
            session_id: record.id.clone(),
            status: SessionStatus::Idle,
            mode,
            turn_count: 0,
            total_token_count: 0,
            context_used: 0,
            context_window: 0,
            current_turn_id: None,
            pending_permissions: Vec::new(),
            active_tool_calls: Vec::new(),
            background_jobs: Vec::new(),
            // 与热快照同面：projection 的 target 键恒在（无 goal 事实为 null）。
            target: Some(target),
            last_error: None,
        },
        runtime: SessionRuntimeState {
            event_seq: 0,
            state_revision: 0,
            delivery_kind: None,
            active_turn_id: None,
            active_turn_kind: None,
            pending_request_ids: Vec::new(),
            api_retry: None,
            context_usage: None,
            goal_verifications: Some(goal_verifications),
            goal_verification_timeline: Some(goal_verification_timeline),
        },
        messages,
        goal_stats: None,
        todos: Some(Vec::new()),
        todo_groups: Some(Vec::new()),
        slash_commands: Some(crate::state::builtin_slash_commands()),
    }
}

/// DB `permission` 列（`{"mode":"build"}`）→ 协议 mode；缺失/不合法按 `build`。
pub(crate) fn permission_mode(record: &SessionRecord) -> SessionMode {
    record
        .permission
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| value.get("mode").and_then(Value::as_str).map(str::to_owned))
        .map(|mode| match mode.as_str() {
            "plan" => SessionMode::Plan,
            "edit" => SessionMode::Edit,
            "yolo" => SessionMode::Yolo,
            "auto" => SessionMode::Auto,
            _ => SessionMode::Build,
        })
        .unwrap_or(SessionMode::Build)
}

fn session_kind_from(task_type: &str) -> SessionKind {
    match task_type {
        "fork" => SessionKind::Fork,
        "selection_side_chat" => SessionKind::SelectionSideChat,
        "workflow_parent" => SessionKind::WorkflowParent,
        "workflow_child" => SessionKind::WorkflowChild,
        "subagent_child" => SessionKind::SubagentChild,
        "nested_workflow_child" => SessionKind::NestedWorkflowChild,
        // 观察值只有 "interactive"；未知档位按 interactive 落，不新造取值。
        _ => SessionKind::Interactive,
    }
}

fn title_source_from(title_source: &str) -> Option<SessionTitleSource> {
    match title_source {
        "default" => Some(SessionTitleSource::Default),
        "first_input" => Some(SessionTitleSource::FirstInput),
        "generated" => Some(SessionTitleSource::Generated),
        "custom" => Some(SessionTitleSource::Custom),
        _ => None,
    }
}

fn epoch_ms_to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// `i64` 毫秒时间戳 → `u64`（负数/溢出按 0，不编造时间）。
pub(crate) fn epoch_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}
