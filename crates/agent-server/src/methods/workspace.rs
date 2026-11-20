//! workspace 策略写方法（Host 切到 Rust owner 后仍要同步的 workspace 级事实）。
//!
//! 协议镜像与语义见 `agent_protocol::workspace_policy` /
//! `docs/specs/session-write-group.md` §2q：严格解析，结果回显 workspace 与生效值；
//! 没有会话副作用。交互 snooze 计数与 model-IO 的会话回填本进程不持有，按 0 返回。

use agent_protocol::workspace_policy::{
    WorkspaceUpdateDynamicWorkflowPolicyParams, WorkspaceUpdateDynamicWorkflowPolicyResult,
    WorkspaceUpdateInteractionPreferencesParams, WorkspaceUpdateInteractionPreferencesResult,
    WorkspaceUpdateModelIoPreferencesParams, WorkspaceUpdateModelIoPreferencesResult,
    WorkspaceUpdateOffPeakToolPolicyParams, WorkspaceUpdateOffPeakToolPolicyResult,
};
use serde::Serialize;
use serde_json::Value;

use crate::server::ServerError;

use super::parse_params;

fn to_value<T: Serialize>(value: T) -> Result<Value, ServerError> {
    serde_json::to_value(value).map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_update_interaction_preferences(
    _server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: WorkspaceUpdateInteractionPreferencesParams = parse_params(
        agent_protocol::workspace_policy::WORKSPACE_UPDATE_INTERACTION_PREFERENCES_METHOD,
        params,
    )?;
    to_value(WorkspaceUpdateInteractionPreferencesResult {
        workspace: params.workspace,
        ask_user_question_auto_resolution_enabled: params
            .preferences
            .ask_user_question_auto_resolution_enabled,
        snoozed_interaction_count: 0,
    })
}

pub(crate) fn handle_update_model_io_preferences(
    _server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: WorkspaceUpdateModelIoPreferencesParams = parse_params(
        agent_protocol::workspace_policy::WORKSPACE_UPDATE_MODEL_IO_PREFERENCES_METHOD,
        params,
    )?;
    to_value(WorkspaceUpdateModelIoPreferencesResult {
        workspace: params.workspace,
        full_retention_enabled: params.preferences.full_retention_enabled,
        updated_session_count: 0,
    })
}

pub(crate) fn handle_update_off_peak_tool_policy(
    _server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: WorkspaceUpdateOffPeakToolPolicyParams = parse_params(
        agent_protocol::workspace_policy::WORKSPACE_UPDATE_OFF_PEAK_TOOL_POLICY_METHOD,
        params,
    )?;
    to_value(WorkspaceUpdateOffPeakToolPolicyResult {
        workspace: params.workspace,
        enabled: params.enabled,
    })
}

pub(crate) fn handle_update_dynamic_workflow_policy(
    _server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: WorkspaceUpdateDynamicWorkflowPolicyParams = parse_params(
        agent_protocol::workspace_policy::WORKSPACE_UPDATE_DYNAMIC_WORKFLOW_POLICY_METHOD,
        params,
    )?;
    to_value(WorkspaceUpdateDynamicWorkflowPolicyResult {
        workspace: params.workspace,
        enabled: params.enabled,
    })
}

/// `workspace/readPresentation`：workspace 级 mode + slashCommands。
///
/// mode 取该 workspace 最近更新的会话的权限档（与 UI 打开最近任务一致）；没有会话时
/// 按 build。slashCommands 与快照同一份 builtin 数据契约。
pub(crate) fn handle_read_presentation(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: agent_protocol::workspace::WorkspacePresentationParams = parse_params(
        agent_protocol::workspace::WORKSPACE_READ_PRESENTATION_METHOD,
        params,
    )?;
    let mode = server
        .store()
        .ok()
        .and_then(|store| {
            store
                .list_sessions(&params.workspace.workspace_path, true, Some(1))
                .ok()
        })
        .and_then(|sessions| sessions.first().map(super::permission_mode))
        .unwrap_or(agent_protocol::session_read::SessionMode::Build);
    to_value(agent_protocol::workspace::WorkspacePresentation {
        workspace: params.workspace,
        mode,
        slash_commands: crate::state::builtin_slash_commands(),
    })
}
