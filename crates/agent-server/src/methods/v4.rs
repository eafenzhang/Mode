//! `v4/command`、`v4/commands/query` 与 v4 附件事务（C4 T4）。
//!
//! 已接命令：`StartSavedWorkflow`（T3 裁决：嵌入 JS 引擎执行）、`SendText`
//! （C4-T4 裁决：按正确行为实现——ref 授权校验 + 文本 turn；附件→模型内容的
//! part 注入属后续切片）。未实现的命令类型返回 `status:"failed"` +
//! `fault.command.unsupported`（不 `-32601`，让客户端拿到按命令的失败）。
//!
//! 附件面：`v4/attachment/begin|chunk|commit|abort|read|previewSource` 与
//! `v4/conversation/attachmentRead|attachmentStat`；授权按「正确行为」：
//! 同 session 授权，跨 session 走 share 错误，未知 ref 走 preview 错误。

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
use super::v4_attachments::handle_send_text;

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

fn accepted_ack(command_id: &str, revision: u64, result: Option<Value>) -> CommandAck {
    CommandAck {
        memory_enabled: Some(false),
        ttft_excluded: None,
        command_id: command_id.to_string(),
        status: CommandAckStatus::Accepted,
        reason_code: None,
        message: None,
        revision_at_decision: revision.into(),
        result,
    }
}

/// `v4/command`：信封严格解析 → 按命令类型分派 → ACK（含失败）。
pub(crate) fn handle_command(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let envelope: ConversationCommandEnvelope = parse_params(V4_COMMAND_METHOD, params)?;
    envelope.validate().map_err(ServerError::invalid_params)?;
    let revision = revision_at_decision(server, envelope.session_id.0.as_ref());
    let ack = match envelope.command_type {
        CommandType::StartSavedWorkflow => handle_start_saved_workflow(server, &envelope, revision),
        CommandType::SendText => handle_send_text(server, &envelope, revision),
        CommandType::CreateSession => handle_create_session(server, &envelope, revision),
        CommandType::Stop => handle_stop(server, &envelope, revision),
        CommandType::Compact => handle_compact(server, &envelope, revision),
        CommandType::SwitchModelConfig => handle_switch_model(server, &envelope, revision),
        CommandType::SwitchCollaborationMode => handle_switch_mode(server, &envelope, revision),
        CommandType::SendGoalCommand => handle_send_goal(server, &envelope, revision),
        CommandType::PauseGoal => handle_goal_action(server, &envelope, revision, "pause"),
        CommandType::ResumeGoal => handle_goal_action(server, &envelope, revision, "resume"),
        CommandType::RenameSession => handle_rename_session(server, &envelope, revision),
        CommandType::DeleteSession => handle_delete_session(server, &envelope, revision),
        CommandType::CancelBackgroundWork => {
            handle_cancel_background_work(server, &envelope, revision)
        }
        CommandType::SetFollowupMode => handle_set_followup_mode(server, &envelope, revision),
        CommandType::SetAutoDrain => handle_set_auto_drain(server, &envelope, revision),
        CommandType::SendQueuedNow => match queue_item_id(&envelope) {
            Ok(id) => handle_cas_command(server, &envelope, revision, |rt| rt.send_queued_now(&id)),
            Err(ack) => Ok(ack),
        },
        CommandType::DeleteQueueItem => match queue_item_id(&envelope) {
            Ok(id) => {
                handle_cas_command(server, &envelope, revision, |rt| rt.delete_queue_item(&id))
            }
            Err(ack) => Ok(ack),
        },
        CommandType::EditQueueItem => {
            let new_text = envelope
                .payload
                .get("newText")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            match queue_item_id(&envelope) {
                Ok(id) => handle_cas_command(server, &envelope, revision, |rt| {
                    rt.edit_queue_item(&id, &new_text)
                }),
                Err(ack) => Ok(ack),
            }
        }
        CommandType::ReorderQueueItem => {
            let before = envelope
                .payload
                .get("beforeQueueItemId")
                .and_then(Value::as_str)
                .map(str::to_string);
            match queue_item_id(&envelope) {
                Ok(id) => handle_cas_command(server, &envelope, revision, |rt| {
                    rt.reorder_queue_item(&id, before.as_deref())
                }),
                Err(ack) => Ok(ack),
            }
        }
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

/// `createSession`：桌面新会话的实际创建路径（draft 预热也走这里）。
///
/// payload 的 `workspaceId` 在本机 Host 里就是 `workspaceKey`（identity 优先，否则
/// 路径）；`config` 的模型/mode 覆盖随 create 一次性进入 record。`firstInput` 存在时
/// 创建后立即按 sendText 的准入路径提交首条输入，ACK 携带 `input` 事实。
fn handle_create_session(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let Some(payload) = envelope.payload.as_object() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "createSession payload must be an object",
        ));
    };
    let Some(workspace_id) = payload
        .get("workspaceId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "createSession requires workspaceId",
        ));
    };
    let config = payload.get("config").and_then(Value::as_object);
    let first_input = payload.get("firstInput").and_then(Value::as_object);
    // 新任务的首发选择可能只出现在 `firstInput`（renderer 的 Submission 覆盖），
    // 不能只读 `config`，否则 runtime 没有 provider/model，turn 在模型请求前就失败。
    let input_selection = first_input
        .and_then(|input| input.get("modelSelection"))
        .and_then(Value::as_object);
    let selection = config
        .and_then(|config| config.get("modelSelection"))
        .and_then(Value::as_object)
        .or(input_selection);
    let mut create = serde_json::Map::new();
    create.insert(
        "workspace".to_string(),
        serde_json::json!({
            "workspacePath": workspace_id,
            "workspaceIdentity": workspace_id,
            "workspaceKey": workspace_id,
        }),
    );
    let mode = first_input
        .and_then(|input| input.get("mode"))
        .and_then(Value::as_str)
        .or_else(|| {
            config
                .and_then(|config| config.get("mode"))
                .and_then(Value::as_str)
        })
        .filter(|value| !value.is_empty());
    if let Some(mode) = mode {
        create.insert("mode".to_string(), Value::String(mode.to_string()));
    }
    let thought = selection
        .and_then(|selection| selection.get("options"))
        .and_then(|options| options.get("reasoningLevel"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            config
                .and_then(|config| config.get("thought"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        });
    let provider = selection
        .and_then(|selection| selection.get("providerId"))
        .and_then(Value::as_str)
        .or_else(|| {
            config
                .and_then(|config| config.get("provider"))
                .and_then(Value::as_str)
        })
        .filter(|value| !value.is_empty());
    let model_id = selection
        .and_then(|selection| selection.get("modelId"))
        .and_then(Value::as_str)
        .or_else(|| {
            config
                .and_then(|config| config.get("model"))
                .and_then(Value::as_str)
        })
        .filter(|value| !value.is_empty());
    if let (Some(provider), Some(model_id)) = (provider, model_id) {
        let mut model = serde_json::json!({
            "providerId": provider,
            "modelId": model_id,
        });
        if let Some(thought) = thought {
            model["options"] = serde_json::json!({ "reasoningLevel": thought });
        }
        create.insert("model".to_string(), model);
    }
    if let Some(thought) = thought {
        create.insert(
            "thoughtLevel".to_string(),
            Value::String(thought.to_string()),
        );
    }
    for key in ["mcpServers", "offPeakToolEnabled", "dynamicWorkflowEnabled"] {
        if let Some(value) = payload.get(key) {
            create.insert(key.to_string(), value.clone());
        }
    }
    let created = super::create::handle(server, Value::Object(create))?;
    let Some(session_id) = created
        .pointer("/session/sessionId")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Err(ServerError::internal(
            "session/create 未返回 sessionId".to_string(),
        ));
    };

    let mut input = None;
    if let Some(first_input) = payload.get("firstInput").and_then(Value::as_object) {
        let Some(text) = first_input.get("text").and_then(Value::as_str) else {
            return Ok(failed_ack(
                &envelope.command_id,
                revision,
                "fault.command.invalidPayload",
                "createSession.firstInput.text is required",
            ));
        };
        let mut send_payload = serde_json::Map::new();
        send_payload.insert("text".to_string(), Value::String(text.to_string()));
        // 模型选择已随 create 的 config/firstInput 写进 runtime；这里不再重复 setModel
        // （避免同一选择走两次能力解析）。
        for key in ["attachments", "mode", "planEnabled"] {
            if let Some(value) = first_input.get(key) {
                send_payload.insert(key.to_string(), value.clone());
            }
        }
        let mut send_envelope = envelope.clone();
        send_envelope.command_type = CommandType::SendText;
        send_envelope.session_id = CommandSessionId(Some(session_id.clone()));
        send_envelope.payload = Value::Object(send_payload);
        let send_revision = revision_at_decision(server, Some(&session_id));
        let ack = handle_send_text(server, &send_envelope, send_revision)?;
        if ack.status != CommandAckStatus::Accepted {
            return Ok(failed_ack(
                &envelope.command_id,
                revision,
                ack.reason_code
                    .as_deref()
                    .unwrap_or("fault.command.inputRejected"),
                ack.message.as_deref().unwrap_or("first input rejected"),
            ));
        }
        input = ack.result;
    }
    let mut result = serde_json::json!({
        "type": "createSession",
        "sessionId": session_id,
    });
    if let Some(input) = input {
        result["input"] = input;
    }
    Ok(accepted_ack(&envelope.command_id, revision, Some(result)))
}

fn handle_stop(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "stop requires a sessionId",
        ));
    };
    match super::goal::handle_stop(server, serde_json::json!({ "sessionId": session_id })) {
        Ok(_) => Ok(accepted_ack(&envelope.command_id, revision, None)),
        Err(error) => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.executionFailed",
            &error.message(),
        )),
    }
}

fn handle_compact(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "compact requires a sessionId",
        ));
    };
    let mut params = serde_json::json!({
        "sessionId": session_id,
        "inputId": envelope.command_id,
    });
    if let Some(base) = envelope.base_revision.clone() {
        params["expectedRevision"] = base.into();
    }
    match super::compact::handle(server, params) {
        Ok(_) => Ok(accepted_ack(&envelope.command_id, revision, None)),
        Err(error) => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.executionFailed",
            &error.message(),
        )),
    }
}

fn handle_switch_model(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "switchModelConfig requires a sessionId",
        ));
    };
    let payload = envelope.payload.as_object();
    let provider = payload
        .and_then(|payload| payload.get("provider"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let model_id = payload
        .and_then(|payload| payload.get("model"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let (Some(provider), Some(model_id)) = (provider, model_id) else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "switchModelConfig requires provider and model",
        ));
    };
    let thought = payload
        .and_then(|payload| payload.get("thought"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let mut model = serde_json::json!({
        "providerId": provider,
        "modelId": model_id,
    });
    if let Some(thought) = thought {
        model["options"] = serde_json::json!({ "reasoningLevel": thought });
    }
    let mut set_model = serde_json::json!({
        "sessionId": session_id,
        "model": model,
    });
    if let Some(base) = envelope.base_revision.clone() {
        set_model["expectedRevision"] = base.into();
    }
    if let Err(error) = super::setters::handle_set_model(server, set_model) {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.executionFailed",
            &error.message(),
        ));
    }
    if let Some(thought) = thought {
        let params = serde_json::json!({
            "sessionId": session_id,
            "thoughtLevel": thought,
        });
        if let Err(error) = super::setters::handle_set_thought_level(server, params) {
            return Ok(failed_ack(
                &envelope.command_id,
                revision,
                "fault.command.executionFailed",
                &error.message(),
            ));
        }
    }
    Ok(accepted_ack(&envelope.command_id, revision, None))
}

fn handle_switch_mode(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "switchCollaborationMode requires a sessionId",
        ));
    };
    let Some(mode) = envelope
        .payload
        .get("mode")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "switchCollaborationMode requires mode",
        ));
    };
    let mut params = serde_json::json!({ "sessionId": session_id, "mode": mode });
    if let Some(base) = envelope.base_revision.clone() {
        params["expectedRevision"] = base.into();
    }
    match super::setters::handle_set_mode(server, params) {
        Ok(_) => Ok(accepted_ack(&envelope.command_id, revision, None)),
        Err(error) => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.executionFailed",
            &error.message(),
        )),
    }
}

fn handle_send_goal(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "sendGoalCommand requires a sessionId",
        ));
    };
    let Some(objective) = envelope
        .payload
        .get("text")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "sendGoalCommand requires text",
        ));
    };
    let params = serde_json::json!({
        "sessionId": session_id,
        "inputId": envelope.command_id,
        "action": "set",
        "objective": objective,
    });
    match super::goal::handle(server, params) {
        Ok(_) => Ok(accepted_ack(&envelope.command_id, revision, None)),
        Err(error) => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.executionFailed",
            &error.message(),
        )),
    }
}

fn handle_goal_action(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
    action: &str,
) -> Result<CommandAck, ServerError> {
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "goal command requires a sessionId",
        ));
    };
    let params = serde_json::json!({ "sessionId": session_id, "action": action });
    match super::goal::handle(server, params) {
        Ok(_) => Ok(accepted_ack(&envelope.command_id, revision, None)),
        Err(error) => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.executionFailed",
            &error.message(),
        )),
    }
}

fn handle_rename_session(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "renameSession requires a sessionId",
        ));
    };
    let Some(title) = envelope.payload.get("title").and_then(Value::as_str) else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "renameSession requires title",
        ));
    };
    let result = match server.find_runtime(session_id) {
        Some(runtime) => runtime.rename(title),
        None => server
            .store()?
            .set_session_title(session_id, title, "custom", super::create::now_ms())
            .map_err(|error| crate::state::RuntimeError::Store(error.to_string())),
    };
    match result {
        Ok(()) => {
            if let Ok(store) = server.store() {
                if let Ok(Some(record)) = store.get_session(session_id) {
                    server.sessions_index().upsert_from_store(&record);
                }
            }
            Ok(accepted_ack(&envelope.command_id, revision, None))
        }
        Err(error) => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.executionFailed",
            &error.to_string(),
        )),
    }
}

fn handle_delete_session(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "deleteSession requires a sessionId",
        ));
    };
    if let Some(runtime) = server.find_runtime(session_id) {
        // busy 会话先拒绝：worker 仍在写库，删行会被后续写入复活成孤儿。
        if runtime.is_busy() {
            return Ok(failed_ack(
                &envelope.command_id,
                revision,
                "fault.command.busyCannotDelete",
                "Session is running; stop it before deleting",
            ));
        }
        server.remove_runtime(session_id);
        let _ = runtime.cancel_turn();
    }
    let removed = server
        .store()
        .ok()
        .and_then(|store| store.get_session(session_id).ok().flatten());
    match server.store()?.delete_session(session_id) {
        Ok(_) => {
            if let Some(record) = removed {
                server.sessions_index().remove_record(&record);
            }
            Ok(accepted_ack(&envelope.command_id, revision, None))
        }
        Err(error) => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.executionFailed",
            &error.to_string(),
        )),
    }
}

fn handle_cancel_background_work(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "cancelBackgroundWork requires a sessionId",
        ));
    };
    let Some(work_id) = envelope
        .payload
        .get("workId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "cancelBackgroundWork requires workId",
        ));
    };
    let Some(runtime) = server.find_runtime(session_id) else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.sessionNotActive",
            &format!("Session is not active: {session_id}"),
        ));
    };
    match runtime.cancel_background_work(work_id) {
        Some(_) => Ok(accepted_ack(&envelope.command_id, revision, None)),
        None => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.backgroundWorkNotFound",
            &format!("Background work not found: {work_id}"),
        )),
    }
}

/// CAS 队列/配置命令的公共壳：stale → `status:"stale"`（UI 用 revisionAtDecision
/// 重试）；runtime 不在 → sessionNotActive；业务错误 → failed。
fn handle_cas_command<F>(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
    apply: F,
) -> Result<CommandAck, ServerError>
where
    F: FnOnce(&crate::runtime::SessionRuntime) -> Result<u64, crate::state::RuntimeError>,
{
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "command requires a sessionId",
        ));
    };
    let Some(runtime) = server.find_runtime(session_id) else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.sessionNotActive",
            &format!("Session is not active: {session_id}"),
        ));
    };
    let current = runtime.state_revision();
    let expected = envelope
        .base_revision
        .as_ref()
        .and_then(serde_json::Number::as_u64);
    if let Some(expected) = expected {
        if expected != current {
            return Ok(CommandAck {
                memory_enabled: Some(false),
                ttft_excluded: None,
                command_id: envelope.command_id.clone(),
                status: CommandAckStatus::Stale,
                reason_code: None,
                message: None,
                revision_at_decision: current.into(),
                result: None,
            });
        }
    }
    match apply(&runtime) {
        Ok(_) => Ok(accepted_ack(&envelope.command_id, revision, None)),
        Err(error) => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.executionFailed",
            &error.to_string(),
        )),
    }
}

fn handle_set_followup_mode(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let mode = match envelope.payload.get("mode").and_then(Value::as_str) {
        Some("queue") => agent_protocol::v4_snapshot::FollowupMode::Queue,
        Some("guide") => agent_protocol::v4_snapshot::FollowupMode::Guide,
        _ => {
            return Ok(failed_ack(
                &envelope.command_id,
                revision,
                "fault.command.invalidPayload",
                "setFollowupMode.mode must be queue|guide",
            ))
        }
    };
    handle_cas_command(server, envelope, revision, |runtime| {
        runtime.set_followup_mode(mode, None)
    })
}

fn handle_set_auto_drain(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let Some(auto_drain) = envelope.payload.get("autoDrain").and_then(Value::as_bool) else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "setAutoDrain requires autoDrain",
        ));
    };
    handle_cas_command(server, envelope, revision, |runtime| {
        runtime.set_auto_drain(auto_drain, None)
    })
}

/// 队列项命令的 `queueItemId` 提取；缺失/空白 → 失败 ACK。
#[allow(clippy::result_large_err)] // Err 携带完整失败 ACK（与成功面同形状），调用方直接透传
fn queue_item_id(envelope: &ConversationCommandEnvelope) -> Result<String, CommandAck> {
    match envelope
        .payload
        .get("queueItemId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        Some(id) => Ok(id.to_string()),
        None => Err(failed_ack(
            &envelope.command_id,
            envelope
                .base_revision
                .as_ref()
                .and_then(serde_json::Number::as_u64)
                .unwrap_or(0),
            "fault.command.invalidPayload",
            "command requires queueItemId",
        )),
    }
}

/// `v4/commands/query`：按 key 对账已受理命令的 ACK；未受理返回 `"unknown"`。
pub(crate) fn handle_commands_query(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: CommandsQueryParams = parse_params(V4_COMMANDS_QUERY_METHOD, params)?;
    params.validate().map_err(ServerError::invalid_params)?;
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
