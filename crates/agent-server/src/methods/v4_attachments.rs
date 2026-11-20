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
use crate::server::ServerError;

use super::parse_params;

// ── SendText ────────────────────────────────────────────────────────────────

/// sendText payload：`text` 必填；`attachments` 可选（`{ref,fileName,mime,bytes}`）。
struct SendTextPayload {
    text: String,
    attachment_refs: Vec<String>,
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
    let input = crate::state::SessionInput::text(payload.text.clone());
    match runtime.submit(input) {
        Ok(_) => Ok(CommandAck {
            memory_enabled: Some(false),
            ttft_excluded: None,
            command_id: envelope.command_id.clone(),
            status: CommandAckStatus::Accepted,
            reason_code: None,
            message: None,
            revision_at_decision: revision.into(),
            result: Some(serde_json::json!({ "type": "sendText" })),
        }),
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

pub(crate) fn handle_attachment_begin(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: AttachmentBeginParams = parse_params(V4_ATTACHMENT_BEGIN_METHOD, params)?;
    params.validate().map_err(ServerError::invalid_params)?;
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
        crate::attachments::BeginOutcome::Staging => {
            serde_json::to_value(AttachmentBeginResult::Staging(AttachmentBeginStaging {
                upload_id: params.upload_id,
                state: AttachmentBeginStateTag::Staging,
                next_chunk_index: 0,
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
