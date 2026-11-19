//! `v4/command` 与 `v4/commands/query`（C4 T4）。
//!
//! 本切片实现 `StartSavedWorkflow`（T3 裁决：嵌入 JS 引擎执行）与命令对账读面；
//! `SendText` / 附件命令在附件切片接入。未实现的命令类型返回
//! `status:"failed"` + `fault.command.unsupported`（不 `-32601`，让客户端拿到
//! 按命令的失败而非方法级错误）。

use agent_protocol::v4::{
    CommandAck, CommandAckStatus, CommandKey, CommandQueryItem, CommandQueryOutcome,
    CommandSessionId, CommandType, CommandsQueryParams, CommandsQueryResult,
    ConversationCommandEnvelope, V4_COMMANDS_QUERY_METHOD, V4_COMMAND_METHOD,
};
use agent_protocol::workflow::SavedWorkflowScope;
use serde_json::Value;

use crate::dispatch::AgentServer;
use crate::server::ServerError;
use crate::workflows;

use super::parse_params;

/// startSavedWorkflow 的 payload（协议未在此镜像建模，按观察键严格读）。
struct StartSavedWorkflowPayload {
    name: String,
    scope: Option<SavedWorkflowScope>,
}

fn parse_start_payload(payload: &Value) -> Result<StartSavedWorkflowPayload, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "payload must be an object".to_string())?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "payload.name is required".to_string())?;
    let scope = match object.get("scope").and_then(Value::as_str) {
        Some("global") => Some(SavedWorkflowScope::Global),
        Some("project") => Some(SavedWorkflowScope::Project),
        None => None,
        Some(other) => return Err(format!("payload.scope is invalid: {other}")),
    };
    Ok(StartSavedWorkflowPayload {
        name: name.to_string(),
        scope,
    })
}

fn failed_ack(
    command_id: &str,
    revision_at_decision: u64,
    reason_code: &str,
    message: &str,
) -> CommandAck {
    CommandAck {
        memory_enabled: Some(false),
        ttft_excluded: None,
        command_id: command_id.to_string(),
        status: CommandAckStatus::Failed,
        reason_code: Some(reason_code.to_string()),
        message: Some(message.to_string()),
        revision_at_decision: revision_at_decision.into(),
        result: None,
    }
}

fn revision_at_decision(server: &AgentServer, session_id: Option<&String>) -> u64 {
    session_id
        .and_then(|session_id| server.find_runtime(session_id))
        .map(|runtime| runtime.state_revision())
        .unwrap_or(0)
}

/// `v4/command`：信封严格解析 → 按命令类型分派 → ACK（含失败）。
pub(crate) fn handle_command(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let envelope: ConversationCommandEnvelope = parse_params(V4_COMMAND_METHOD, params)?;
    envelope
        .validate()
        .map_err(ServerError::invalid_params)?;
    let revision = revision_at_decision(server, envelope.session_id.0.as_ref());
    let ack = match envelope.command_type {
        CommandType::StartSavedWorkflow => handle_start_saved_workflow(server, &envelope, revision),
        _ => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.unsupported",
            &format!(
                "Command {:?} is not supported by this agent runtime",
                envelope.command_type
            ),
        )),
    }?;
    server.v4_state().record_command(
        envelope.session_id.0.clone(),
        envelope.command_id.clone(),
        ack.clone(),
    );
    serde_json::to_value(ack).map_err(|error| ServerError::internal(error.to_string()))
}

fn handle_start_saved_workflow(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let payload = match parse_start_payload(&envelope.payload) {
        Ok(payload) => payload,
        Err(message) => {
            return Ok(failed_ack(
                &envelope.command_id,
                revision,
                "fault.command.savedWorkflowStartRejected.invalid_payload",
                &message,
            ))
        }
    };
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.savedWorkflowStartRejected.invalid_payload",
            "startSavedWorkflow requires a sessionId",
        ));
    };
    let data_root = match server.data_root() {
        Some(data_root) => data_root,
        None => {
            return Ok(failed_ack(
                &envelope.command_id,
                revision,
                "fault.command.savedWorkflowStartRejected.store_failed",
                "数据根不可用：workflow 无事实源",
            ))
        }
    };
    let store = server.store()?;
    let runtime = server.find_runtime(session_id);
    let workspace_path = runtime
        .as_ref()
        .map(|runtime| runtime.record().directory.clone())
        .unwrap_or_else(|| data_root.display().to_string());
    let dirs = workflows::WorkflowDirs::new(&workspace_path, &data_root);
    match workflows::start_saved_workflow(
        &store,
        &dirs,
        &workspace_path,
        session_id,
        &payload.name,
        payload.scope,
    ) {
        Ok(started) => Ok(CommandAck {
            memory_enabled: Some(false),
            ttft_excluded: None,
            command_id: envelope.command_id.clone(),
            status: CommandAckStatus::Accepted,
            reason_code: None,
            message: None,
            revision_at_decision: revision.into(),
            result: Some(serde_json::json!({
                "type": "startSavedWorkflow",
                "runId": started.run_id,
                "toolCallId": started.tool_call_id,
            })),
        }),
        Err(rejection) => Ok(failed_ack(
            &envelope.command_id,
            revision,
            &rejection.reason_code,
            &rejection.message,
        )),
    }
}

/// `v4/commands/query`：按 key 对账已受理命令的 ACK；未受理返回 `"unknown"`。
pub(crate) fn handle_commands_query(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: CommandsQueryParams = parse_params(V4_COMMANDS_QUERY_METHOD, params)?;
    params
        .validate()
        .map_err(ServerError::invalid_params)?;
    let results = params
        .commands
        .iter()
        .map(|key| CommandQueryItem {
            key: CommandKey {
                session_id: CommandSessionId(key.session_id.0.clone()),
                command_id: key.command_id.clone(),
            },
            result: server
                .v4_state()
                .lookup(key.session_id.0.as_deref(), &key.command_id)
                .map(|ack| CommandQueryOutcome::Ack(Box::new(ack)))
                .unwrap_or(CommandQueryOutcome::Unknown(
                    agent_protocol::v4::UnknownCommand::Unknown,
                )),
        })
        .collect();
    serde_json::to_value(CommandsQueryResult {
        results,
        clock: None,
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}
