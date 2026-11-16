//! DB 行 → `session/messages` / 快照 `messages` 的**线格式投影**。
//!
//! 契约依据（`docs/specs/agent-server-c1-contract.md` §3.7）：`session/messages`
//! 返回的是投影，不是 `message.data` / `part.data` 原样——wire 的 `partId`/`callId`
//! 对应 DB 的 `id`/`callID`，tool state 的 `startedAt`/`completedAt` 对应 DB 的
//! `time.start`/`time.end`，DB 里的 `modelSelection`/`contextSnapshot`/`anchor`/
//! `readFileState` 不进 wire。因此投影在这里**逐字段构造**严格镜像类型（各分支
//! `deny_unknown_fields`），DB 的富形状保持原样落库，不被投影改写。
//!
//! 取值纪律：只投影已存储/已观察的事实；缺失的必填字段用**中性空值**（空串 /
//! 空对象 / 0），不编造语义。`agent` 常量 `mode-agent` 来自 §3.3 的观察值。

use agent_protocol::message::{
    AssistantMessageInfo, AssistantRole, MessageInfo, MessageModel, MessagePart, MessagePath,
    MessageTime, MessageWithParts, PartTypeStepFinish, PartTypeStepStart, PartTypeText,
    PartTypeTool, StepFinishPart, StepStartPart, TextPart, TokenCacheUsage, TokenUsage, ToolPart,
    ToolState, ToolStateCompleted, ToolStateError, ToolStatePending, ToolStateRunning,
    ToolStatusCompleted, ToolStatusError, ToolStatusPending, ToolStatusRunning, UserRole,
};
use serde_json::Value;
use session_store::{MessageWithParts as StoreMessageWithParts, PartRecord, SessionRecord};

use crate::server::ServerError;

use super::epoch_u64;

/// 会话行的 wire 投影里 `agent` 字段的运行时名（§3.3 观察值）。
const AGENT_NAME: &str = "mode-agent";

/// 一批 store 行 → wire 消息（升序，与 store 的顺序一致）。
pub(crate) fn project_messages(
    record: &SessionRecord,
    messages: Vec<StoreMessageWithParts>,
) -> Result<Vec<MessageWithParts>, ServerError> {
    messages
        .into_iter()
        .map(|message| {
            project_one(record, &message).map(|(info, parts)| MessageWithParts { info, parts })
        })
        .collect()
}

fn project_one(
    record: &SessionRecord,
    message: &StoreMessageWithParts,
) -> Result<(MessageInfo, Vec<MessagePart>), ServerError> {
    let data = &message.message.data;
    let info = match data.get("role").and_then(Value::as_str) {
        Some("assistant") => MessageInfo::Assistant(project_assistant(record, message)),
        Some("user") => MessageInfo::User(project_user(message)),
        // 未知 role：显式拒绝（线格式只有 user/assistant 两支）。
        other => {
            return Err(ServerError::internal(format!(
                "message {} 的 role 不合法: {:?}",
                message.message.id, other
            )))
        }
    };
    let parts = message.parts.iter().filter_map(project_part).collect();
    Ok((info, parts))
}

fn project_user(message: &StoreMessageWithParts) -> agent_protocol::message::UserMessageInfo {
    let data = &message.message.data;
    let model = data
        .get("model")
        .map(|model| MessageModel {
            provider_id: string_field(model, "providerID", ""),
            model_id: string_field(model, "modelID", ""),
            options: None,
        })
        .filter(|model| !model.provider_id.is_empty() && !model.model_id.is_empty());
    agent_protocol::message::UserMessageInfo {
        message_id: string_field(data, "messageId", &message.message.id),
        session_id: message.message.session_id.clone(),
        role: UserRole::User,
        time: project_time(data, message.message.time_created),
        agent: string_field_or(data, "agent", AGENT_NAME),
        model,
        system: None,
        tools: None,
        synthetic: None,
        source: None,
        visibility: None,
        semantics: None,
        metadata: None,
    }
}

fn project_assistant(
    record: &SessionRecord,
    message: &StoreMessageWithParts,
) -> AssistantMessageInfo {
    let data = &message.message.data;
    let model = MessageModel {
        provider_id: string_field(data, "providerId", ""),
        model_id: string_field(data, "modelId", ""),
        options: None,
    };
    let model = (!model.provider_id.is_empty() && !model.model_id.is_empty()).then_some(model);
    AssistantMessageInfo {
        message_id: string_field(data, "messageId", &message.message.id),
        session_id: message.message.session_id.clone(),
        role: AssistantRole::Assistant,
        time: project_time(data, message.message.time_created),
        parent_message_id: string_field(data, "parentID", ""),
        agent: string_field_or(data, "agent", AGENT_NAME),
        model,
        // DB 行没有 path 时用会话工作目录投影（观察形状 `path:{cwd,root}`）。
        path: data
            .get("path")
            .map(|path| MessagePath {
                cwd: string_field(path, "cwd", ""),
                root: string_field(path, "root", ""),
            })
            .unwrap_or(MessagePath {
                cwd: record.directory.clone(),
                root: record.directory.clone(),
            }),
        cost: data
            .get("cost")
            .and_then(Value::as_number)
            .cloned()
            .unwrap_or_else(|| serde_json::Number::from(0)),
        tokens: project_tokens(data.get("tokens")),
        finish: data
            .get("finish")
            .and_then(Value::as_str)
            .map(str::to_owned),
        error: None,
        semantics: None,
        structured: None,
    }
}

fn project_time(data: &Value, fallback_created: i64) -> MessageTime {
    let time = data.get("time");
    MessageTime {
        created: time
            .and_then(|time| time.get("created"))
            .and_then(Value::as_i64)
            .map(epoch_u64)
            .unwrap_or_else(|| epoch_u64(fallback_created)),
        completed: time
            .and_then(|time| time.get("completed"))
            .and_then(Value::as_i64)
            .map(epoch_u64),
    }
}

fn project_tokens(value: Option<&Value>) -> TokenUsage {
    let cache = value.and_then(|tokens| tokens.get("cache"));
    TokenUsage {
        total: value
            .and_then(|tokens| tokens.get("total"))
            .and_then(Value::as_u64),
        input: num_field(value, "input"),
        output: num_field(value, "output"),
        reasoning: num_field(value, "reasoning"),
        cache: TokenCacheUsage {
            read: num_field(cache, "read"),
            write: num_field(cache, "write"),
        },
    }
}

fn num_field(value: Option<&Value>, key: &str) -> u64 {
    value
        .and_then(|value| value.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn string_field(data: &Value, key: &str, fallback: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_string()
}

/// 空串也回退：`title` 等必填字段不落空（缺失/空都回默认值）。
fn string_field_or(data: &Value, key: &str, fallback: &str) -> String {
    let value = data.get(key).and_then(Value::as_str).unwrap_or_default();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

/// 一个 part 行 → wire part。未建模的 part 类型（`timeline`/`subagent`/…）返回
/// `None`——投影收窄，不在 read 路径上整条失败；DB 行保持原样。
fn project_part(part: &PartRecord) -> Option<MessagePart> {
    let data = &part.data;
    let ids = PartIds::of(part);
    match data.get("type").and_then(Value::as_str)? {
        "text" => Some(MessagePart::Text(TextPart {
            part_id: ids.part_id,
            session_id: ids.session_id,
            message_id: ids.message_id,
            part_type: PartTypeText::Text,
            text: string_field(data, "text", ""),
            synthetic: None,
            ignored: None,
            metadata: None,
        })),
        "step-start" => Some(MessagePart::StepStart(StepStartPart {
            part_id: ids.part_id,
            session_id: ids.session_id,
            message_id: ids.message_id,
            part_type: PartTypeStepStart::StepStart,
            snapshot: None,
        })),
        "step-finish" => Some(MessagePart::StepFinish(StepFinishPart {
            part_id: ids.part_id,
            session_id: ids.session_id,
            message_id: ids.message_id,
            part_type: PartTypeStepFinish::StepFinish,
            reason: string_field(data, "reason", ""),
            snapshot: None,
            cost: data
                .get("cost")
                .and_then(Value::as_number)
                .cloned()
                .unwrap_or_else(|| serde_json::Number::from(0)),
            tokens: project_tokens(data.get("tokens")),
        })),
        "tool" => {
            let tool_name = string_field(data, "tool", "");
            let state = project_tool_state(data.get("state")?, &tool_name, part)?;
            Some(MessagePart::Tool(ToolPart {
                part_id: ids.part_id,
                session_id: ids.session_id,
                message_id: ids.message_id,
                part_type: PartTypeTool::Tool,
                call_id: string_field(data, "callID", ""),
                tool: tool_name,
                state,
                metadata: None,
            }))
        }
        _ => None,
    }
}

struct PartIds {
    part_id: String,
    session_id: String,
    message_id: String,
}

impl PartIds {
    fn of(part: &PartRecord) -> Self {
        Self {
            part_id: part.id.clone(),
            session_id: part.session_id.clone(),
            message_id: part.message_id.clone(),
        }
    }
}

/// DB tool part 的 `state` → wire 判别联合。
///
/// `startedAt`/`completedAt` 来自 DB `time.start`/`time.end`（§3.7）；缺失回退到
/// part 行自己的时间列（旧版行兜底，不编时间）。
fn project_tool_state(state: &Value, tool_name: &str, part: &PartRecord) -> Option<ToolState> {
    let input = state
        .get("input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let started_at = || {
        state
            .get("time")
            .and_then(|time| time.get("start"))
            .and_then(Value::as_i64)
            .map(epoch_u64)
            .unwrap_or_else(|| epoch_u64(part.time_created))
    };
    let completed_at = || {
        state
            .get("time")
            .and_then(|time| time.get("end"))
            .and_then(Value::as_i64)
            .map(epoch_u64)
            .unwrap_or_else(|| epoch_u64(part.time_updated))
    };
    match state.get("status").and_then(Value::as_str)? {
        "completed" => Some(ToolState::Completed(ToolStateCompleted {
            status: ToolStatusCompleted::Completed,
            input,
            output: string_field(state, "output", ""),
            title: string_field_or(state, "title", tool_name),
            metadata: state
                .get("metadata")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            started_at: started_at(),
            completed_at: completed_at(),
        })),
        "error" => Some(ToolState::Error(ToolStateError {
            status: ToolStatusError::Error,
            input,
            error: string_field(state, "error", ""),
            metadata: state.get("metadata").and_then(Value::as_object).cloned(),
            started_at: started_at(),
            completed_at: completed_at(),
        })),
        "running" => Some(ToolState::Running(ToolStateRunning {
            status: ToolStatusRunning::Running,
            input,
            title: state
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned),
            metadata: state.get("metadata").and_then(Value::as_object).cloned(),
            started_at: started_at(),
        })),
        "pending" => Some(ToolState::Pending(ToolStatePending {
            status: ToolStatusPending::Pending,
            input,
            raw: string_field(state, "raw", ""),
        })),
        _ => None,
    }
}
