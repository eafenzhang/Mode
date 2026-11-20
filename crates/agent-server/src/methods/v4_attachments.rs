//! `v4/command SendText` 与 v4 附件事务 handlers（C4 T4b）。
//!
//! SendText 按 C4-T4 裁决「正确行为」实现：ref 必须已 commit 且属于本 session；
//! 文本交给会话 runtime 起普通 turn。附件→模型内容的 part 注入属后续切片
//! （agent-core 的 user message 还没有附件 part 面），此处登记。
//!
//! 授权矩阵（观察 + 裁决）：
//! - preview 面（`v4/attachment/read|previewSource`）：未知/跨 session →
//!   `fault.attachment.previewRefNotAuthorized`；
//! - share 面（`v4/conversation/attachmentRead|attachmentStat`）：未知/跨 session →
//!   `fault.attachment.shareReadNotAuthorized` / `shareStatNotAuthorized`。

use agent_protocol::v4::{CommandAck, CommandAckStatus, ConversationCommandEnvelope};
use agent_protocol::v4_queries::{
    AttachmentAbortResult, AttachmentBeginCommitted, AttachmentBeginParams, AttachmentBeginResult,
    AttachmentBeginStaging, AttachmentBeginStateTag, AttachmentChunkParams, AttachmentChunkResult,
    AttachmentCommitResult, AttachmentPreviewSourceKind, AttachmentPreviewSourceResult,
    AttachmentReadParams, AttachmentReadResult, AttachmentTerminalParams,
    ConversationAttachmentReadParams, ConversationAttachmentStatParams,
    ConversationAttachmentStatResult, V4_ATTACHMENT_ABORT_METHOD, V4_ATTACHMENT_BEGIN_METHOD,
    V4_ATTACHMENT_CHUNK_METHOD, V4_ATTACHMENT_COMMIT_METHOD, V4_ATTACHMENT_PREVIEW_SOURCE_METHOD,
    V4_ATTACHMENT_READ_METHOD, V4_CONVERSATION_ATTACHMENT_READ_METHOD,
    V4_CONVERSATION_ATTACHMENT_STAT_METHOD,
};
use serde_json::Value;

use crate::dispatch::AgentServer;
use crate::path_safety::is_safe_segment;
use crate::server::ServerError;

use super::parse_params;

// ── SendText ────────────────────────────────────────────────────────────────

/// sendText payload：`text` 必填；`attachments` 可选（`{ref,fileName,mime,bytes}`）。
struct SendTextPayload {
    text: String,
    attachment_refs: Vec<String>,
    mode: Option<String>,
    plan_enabled: Option<bool>,
    model_selection: Option<agent_protocol::message::MessageModel>,
}

fn parse_send_text_payload(payload: &Value) -> Result<SendTextPayload, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "payload must be an object".to_string())?;
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "payload.text is required".to_string())?;
    let mut attachment_refs = Vec::new();
    if let Some(items) = object.get("attachments").and_then(Value::as_array) {
        for item in items {
            let attachment_ref = item
                .get("ref")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "attachment.ref is required".to_string())?;
            attachment_refs.push(attachment_ref.to_string());
        }
    }
    Ok(SendTextPayload {
        text: text.to_string(),
        attachment_refs,
        mode: object
            .get("mode")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        plan_enabled: object.get("planEnabled").and_then(Value::as_bool),
        model_selection: object
            .get("modelSelection")
            .and_then(Value::as_object)
            .and_then(|selection| {
                let provider_id = selection.get("providerId").and_then(Value::as_str)?;
                let model_id = selection.get("modelId").and_then(Value::as_str)?;
                let options = selection
                    .get("options")
                    .and_then(Value::as_object)
                    .and_then(|options| options.get("reasoningLevel").and_then(Value::as_str))
                    .map(|level| agent_protocol::message::ModelOptions {
                        reasoning_level: Some(level.to_string()),
                    });
                Some(agent_protocol::message::MessageModel {
                    provider_id: provider_id.to_string(),
                    model_id: model_id.to_string(),
                    options,
                })
            }),
    })
}

pub(crate) fn handle_send_text(
    server: &AgentServer,
    envelope: &ConversationCommandEnvelope,
    revision: u64,
) -> Result<CommandAck, ServerError> {
    let payload = match parse_send_text_payload(&envelope.payload) {
        Ok(payload) => payload,
        Err(message) => {
            return Ok(failed_ack(
                &envelope.command_id,
                revision,
                "fault.command.invalidPayload",
                &message,
            ))
        }
    };
    let Some(session_id) = envelope.session_id.0.as_deref() else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.invalidPayload",
            "sendText requires a sessionId",
        ));
    };
    for attachment_ref in &payload.attachment_refs {
        match server
            .v4_state()
            .attachments
            .authorize(session_id, attachment_ref)
        {
            crate::attachments::RefAuthorization::Authorized(_) => {}
            crate::attachments::RefAuthorization::Foreign(stored) => {
                return Ok(failed_ack(
                    &envelope.command_id,
                    revision,
                    "fault.command.attachmentRefNotAuthorized",
                    &format!(
                        "Attachment ref belongs to another session: {attachment_ref} ({})",
                        stored.session_id
                    ),
                ))
            }
            crate::attachments::RefAuthorization::Unknown => {
                return Ok(failed_ack(
                    &envelope.command_id,
                    revision,
                    "fault.command.attachmentRefUnknown",
                    &format!("Unknown attachment ref: {attachment_ref}"),
                ))
            }
        }
    }
    let Some(runtime) = server.find_runtime(session_id) else {
        return Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.sessionNotActive",
            &format!("Session is not active: {session_id}"),
        ));
    };
    // 首发 sendText 可能落在**无 config 的预热 draft** 上：本条的 modelSelection/mode
    // 才是用户实际选择，必须在提交前写进 runtime（否则 turn 在模型请求前失败）。
    // 已有模型的会话不重复 setModel：选择由 switchModelConfig/create 负责。
    let runtime_config = runtime.config();
    if runtime_config.provider_id.is_empty() || runtime_config.model_id.is_empty() {
        if let Some(selection) = envelope
            .payload
            .get("modelSelection")
            .and_then(Value::as_object)
        {
            let provider = selection
                .get("providerId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let model_id = selection
                .get("modelId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            if let (Some(provider), Some(model_id)) = (provider, model_id) {
                let mut model = serde_json::json!({
                    "providerId": provider,
                    "modelId": model_id,
                });
                if let Some(level) = selection
                    .get("options")
                    .and_then(|options| options.get("reasoningLevel"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    model["options"] = serde_json::json!({ "reasoningLevel": level });
                }
                if let Err(error) = super::setters::handle_set_model(
                    server,
                    serde_json::json!({ "sessionId": session_id, "model": model }),
                ) {
                    return Ok(failed_ack(
                        &envelope.command_id,
                        revision,
                        "fault.command.configRejected",
                        &error.message(),
                    ));
                }
            }
        }
    }
    if let Some(mode) = envelope
        .payload
        .get("mode")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        if let Err(error) = super::setters::handle_set_mode(
            server,
            serde_json::json!({ "sessionId": session_id, "mode": mode }),
        ) {
            return Ok(failed_ack(
                &envelope.command_id,
                revision,
                "fault.command.configRejected",
                &error.message(),
            ));
        }
    }
    let input = crate::state::SessionInput::text(payload.text.clone());
    let meta = crate::runtime::QueueMetaInput {
        client_id: envelope.client_id.clone(),
        mode: payload.mode.clone(),
        plan_enabled: payload.plan_enabled,
        model_selection: payload.model_selection.clone(),
    };
    match runtime.submit_with_meta(input, meta) {
        // ACK 结果形状按 `commandResultSchema`：sendText 的 accepted 结果是
        // `inputAccepted`（renderer 按 discriminated union 解码，`sendText` 会被判无效）。
        Ok(accepted) => {
            let delivery = match accepted.admission {
                agent_core::Admission::Started => "startNow",
                agent_core::Admission::Queued { .. } => "queue",
            };
            Ok(CommandAck {
                memory_enabled: Some(false),
                ttft_excluded: None,
                command_id: envelope.command_id.clone(),
                status: CommandAckStatus::Accepted,
                reason_code: None,
                message: None,
                revision_at_decision: accepted.state_revision.into(),
                result: Some(serde_json::json!({
                    "type": "inputAccepted",
                    "delivery": delivery,
                    "inputId": accepted.input_id,
                })),
            })
        }
        Err(error) => Ok(failed_ack(
            &envelope.command_id,
            revision,
            "fault.command.executionFailed",
            &error.to_string(),
        )),
    }
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

// ── 附件事务 ────────────────────────────────────────────────────────────────

fn attachment_fault(error: crate::attachments::AttachmentError) -> ServerError {
    let data = match &error {
        crate::attachments::AttachmentError::Io(_) => serde_json::json!({ "name": "Error" }),
        other => serde_json::json!({ "name": "ModeAttachmentFaultError", "code": other.fault() }),
    };
    ServerError::rpc(-32603, error.fault(), Some(data))
}

/// 上行事务的路径段校验：附件落盘路径由 session/upload id 直接拼接。
fn validate_upload_ids(session_id: &str, upload_id: &str) -> Result<(), ServerError> {
    if !is_safe_segment(session_id) || !is_safe_segment(upload_id) {
        return Err(ServerError::invalid_params(
            "attachment sessionId/uploadId must be safe path segments",
        ));
    }
    Ok(())
}

pub(crate) fn handle_attachment_begin(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: AttachmentBeginParams = parse_params(V4_ATTACHMENT_BEGIN_METHOD, params)?;
    params.validate().map_err(ServerError::invalid_params)?;
    validate_upload_ids(&params.session_id, &params.upload_id)?;
    // 上限防止 totalChunks=u64::MAX 的 commit 枚举/分块内存耗尽。
    if params.total_chunks > 10_000 || params.total_bytes > 256 * 1024 * 1024 {
        return Err(ServerError::invalid_params(
            "attachment upload exceeds supported size limits",
        ));
    }
    match server
        .v4_state()
        .attachments
        .begin(
            &params.upload_id,
            &params.session_id,
            &params.file_name,
            &params.mime,
            params.total_bytes,
            params.total_chunks,
        )
        .map_err(attachment_fault)?
    {
        crate::attachments::BeginOutcome::Staging(next_chunk_index) => {
            serde_json::to_value(AttachmentBeginResult::Staging(AttachmentBeginStaging {
                upload_id: params.upload_id,
                state: AttachmentBeginStateTag::Staging,
                next_chunk_index,
            }))
            .map_err(|error| ServerError::internal(error.to_string()))
        }
        crate::attachments::BeginOutcome::Committed(attachment_ref) => {
            serde_json::to_value(AttachmentBeginResult::Committed(AttachmentBeginCommitted {
                upload_id: params.upload_id,
                state: AttachmentBeginStateTag::Committed,
                next_chunk_index: params.total_chunks,
                attachment_ref,
            }))
            .map_err(|error| ServerError::internal(error.to_string()))
        }
    }
}

pub(crate) fn handle_attachment_chunk(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: AttachmentChunkParams = parse_params(V4_ATTACHMENT_CHUNK_METHOD, params)?;
    validate_upload_ids(&params.session_id, &params.upload_id)?;
    // 单块 base64 上限（约 8 MiB 原文），避免单次 RPC 打满内存。
    if params.data_base64.len() > 12 * 1024 * 1024 {
        return Err(ServerError::invalid_params("attachment chunk is too large"));
    }
    let next = server
        .v4_state()
        .attachments
        .chunk(
            &params.upload_id,
            &params.session_id,
            params.chunk_index,
            &params.data_base64,
        )
        .map_err(attachment_fault)?;
    serde_json::to_value(AttachmentChunkResult {
        upload_id: params.upload_id,
        next_chunk_index: next,
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_attachment_commit(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: AttachmentTerminalParams = parse_params(V4_ATTACHMENT_COMMIT_METHOD, params)?;
    validate_upload_ids(&params.session_id, &params.upload_id)?;
    let data_root = server
        .data_root()
        .ok_or_else(|| ServerError::internal("数据根不可用：附件无落点"))?;
    let (attachment_ref, _stored) = server
        .v4_state()
        .attachments
        .commit(&params.upload_id, &params.session_id, &data_root)
        .map_err(attachment_fault)?;
    serde_json::to_value(AttachmentCommitResult { attachment_ref })
        .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_attachment_abort(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: AttachmentTerminalParams = parse_params(V4_ATTACHMENT_ABORT_METHOD, params)?;
    validate_upload_ids(&params.session_id, &params.upload_id)?;
    server
        .v4_state()
        .attachments
        .abort(&params.upload_id, &params.session_id)
        .map_err(attachment_fault)?;
    serde_json::to_value(AttachmentAbortResult {})
        .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_attachment_read(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: AttachmentReadParams = parse_params(V4_ATTACHMENT_READ_METHOD, params)?;
    params.validate().map_err(ServerError::invalid_params)?;
    match server
        .v4_state()
        .attachments
        .authorize(&params.session_id, &params.attachment_ref)
    {
        crate::attachments::RefAuthorization::Authorized(stored) => {
            let (data_base64, next_offset) = server
                .v4_state()
                .attachments
                .read(&stored, params.offset, params.limit)
                .map_err(attachment_fault)?;
            serde_json::to_value(AttachmentReadResult {
                data_base64,
                media_type: stored.media_type,
                total_bytes: stored.total_bytes,
                next_offset,
            })
            .map_err(|error| ServerError::internal(error.to_string()))
        }
        _ => Err(ServerError::rpc(
            -32603,
            "fault.attachment.previewRefNotAuthorized",
            Some(serde_json::json!({ "name": "Error" })),
        )),
    }
}

pub(crate) fn handle_attachment_preview_source(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: agent_protocol::v4_queries::AttachmentPreviewSourceParams =
        parse_params(V4_ATTACHMENT_PREVIEW_SOURCE_METHOD, params)?;
    params.validate().map_err(ServerError::invalid_params)?;
    match server
        .v4_state()
        .attachments
        .authorize(&params.session_id, &params.attachment_ref)
    {
        crate::attachments::RefAuthorization::Authorized(stored) => {
            serde_json::to_value(AttachmentPreviewSourceResult::LocalPath(
                agent_protocol::v4_queries::AttachmentPreviewLocalPath {
                    kind: AttachmentPreviewSourceKind::LocalPath,
                    path: stored.path.display().to_string(),
                    media_type: stored.media_type,
                },
            ))
            .map_err(|error| ServerError::internal(error.to_string()))
        }
        _ => Err(ServerError::rpc(
            -32603,
            "fault.attachment.previewRefNotAuthorized",
            Some(serde_json::json!({ "name": "Error" })),
        )),
    }
}

pub(crate) fn handle_conversation_attachment_read(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: ConversationAttachmentReadParams =
        parse_params(V4_CONVERSATION_ATTACHMENT_READ_METHOD, params)?;
    match server
        .v4_state()
        .attachments
        .authorize(&params.session_id, &params.attachment_ref)
    {
        crate::attachments::RefAuthorization::Authorized(stored) => {
            let (data_base64, next_offset) = server
                .v4_state()
                .attachments
                .read(&stored, params.offset, params.limit)
                .map_err(attachment_fault)?;
            serde_json::to_value(AttachmentReadResult {
                data_base64,
                media_type: stored.media_type,
                total_bytes: stored.total_bytes,
                next_offset,
            })
            .map_err(|error| ServerError::internal(error.to_string()))
        }
        _ => Err(ServerError::rpc(
            -32603,
            "fault.attachment.shareReadNotAuthorized",
            Some(serde_json::json!({
                "name": "ModeAttachmentFaultError",
                "code": "fault.attachment.shareReadNotAuthorized",
            })),
        )),
    }
}

pub(crate) fn handle_conversation_attachment_stat(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: ConversationAttachmentStatParams =
        parse_params(V4_CONVERSATION_ATTACHMENT_STAT_METHOD, params)?;
    match server
        .v4_state()
        .attachments
        .authorize(&params.session_id, &params.attachment_ref)
    {
        crate::attachments::RefAuthorization::Authorized(stored) => {
            let mtime_ms = std::fs::metadata(&stored.path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|elapsed| serde_json::Number::from(elapsed.as_millis() as u64));
            serde_json::to_value(ConversationAttachmentStatResult {
                media_type: stored.media_type,
                total_bytes: stored.total_bytes,
                mtime_ms,
            })
            .map_err(|error| ServerError::internal(error.to_string()))
        }
        _ => Err(ServerError::rpc(
            -32603,
            "fault.attachment.shareStatNotAuthorized",
            Some(serde_json::json!({
                "name": "ModeAttachmentFaultError",
                "code": "fault.attachment.shareStatNotAuthorized",
            })),
        )),
    }
}
