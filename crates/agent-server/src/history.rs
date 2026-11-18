//! DB 行 → `Session` 冷物化事实（LT-1 C1 最终评审 C2/C3）。
//!
//! resume / create-on-existing / 进程重启后，运行时的 `Session` 不能从空历史开始：
//! 下一轮模型请求必须带上此前的 user / assistant / tool result 消息，`FileState`
//! 必须恢复已读路径（Read 去重、Write/Edit 的先读再写），turn 计数器必须接着
//! 持久化的最大 turn 号继续（否则 `turn_0` 与旧 `turn_usage` 主键冲突）。
//!
//! 这里只做**读取与投影**：DB 行（`message` + `part` 的富形状）仍是唯一事实源，
//! `ModelMessage` 只是 provider 视角的视图。未观察/未建模的字段不补、不猜。

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_core::{FileState, ModelMessage, ModelToolCall, ToolContext};
use serde_json::Value;
use session_store::MessageWithParts;

/// 冷物化恢复出来的会话事实。
pub(crate) struct RestoredSession {
    /// provider 视角的历史（不含系统提示；新 turn 的输入由 `run_turn` 追加）。
    pub(crate) messages: Vec<ModelMessage>,
    /// `Read` 去重与 Write/Edit 前置条件。
    pub(crate) file_state: FileState,
    /// 下一个可用的 `turnNumber`（持久化最大 turn 号 + 1）。
    pub(crate) next_turn_number: u64,
    /// 已存在的 turn id（新 turn 分配时跳过，保证主键不冲突）。
    pub(crate) used_turn_ids: BTreeSet<String>,
}

/// 把一个会话已持久化的消息/部件恢复成模型历史与文件记忆。
///
/// `turn_ids` 是 `turn_usage` 里已落库的 turn id；消息 `anchor.turnId` 也会并入
/// `used_turn_ids`（历史库可能只有消息、没有 usage 行）。
pub(crate) fn restore(
    directory: &str,
    rows: &[MessageWithParts],
    turn_ids: &[String],
) -> RestoredSession {
    let ctx = ToolContext {
        cwd: directory.to_string(),
        artifact: None,
        tasks: None,
    };
    let mut messages = Vec::new();
    let mut file_state = FileState::new();
    let mut used_turn_ids: BTreeSet<String> = turn_ids.iter().cloned().collect();

    for row in rows {
        let data = &row.message.data;
        if let Some(turn_id) = data
            .get("anchor")
            .and_then(|anchor| anchor.get("turnId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            used_turn_ids.insert(turn_id.to_string());
        }
        match data.get("role").and_then(Value::as_str) {
            Some("user") => {
                let text = collect_text(&row.parts);
                if !text.is_empty() {
                    messages.push(ModelMessage::User(text));
                }
            }
            Some("assistant") => {
                restore_assistant(row, &ctx, &mut messages, &mut file_state);
            }
            // 未知 role：不猜语义；冷恢复跳过（wire 投影会显式拒绝未知 role）。
            _ => {}
        }
    }

    let next_turn_number = used_turn_ids
        .iter()
        .filter_map(|turn_id| turn_number_from_id(turn_id))
        .max()
        .map(|max| max + 1)
        .unwrap_or(0);

    RestoredSession {
        messages,
        file_state,
        next_turn_number,
        used_turn_ids,
    }
}

/// assistant 消息：文本 + 工具调用进历史；每个工具部件补一条 tool result。
///
/// 合成 timeline 消息（`semantics.kind:"timeline_event"`，C1 不写、但可能读 Node 库）
/// 不进入模型历史。工具调用只取 `completed` / `error` 的部件——`pending` 调用没有
/// 结果，放进历史会制造悬空的 `tool_call`。
fn restore_assistant(
    row: &MessageWithParts,
    ctx: &ToolContext,
    messages: &mut Vec<ModelMessage>,
    file_state: &mut FileState,
) {
    let kind = row
        .message
        .data
        .get("semantics")
        .and_then(|semantics| semantics.get("kind"))
        .and_then(Value::as_str);
    if kind == Some("timeline_event") {
        return;
    }

    let text = collect_text(&row.parts);
    let mut tool_calls = Vec::new();
    let mut results: Vec<(String, String)> = Vec::new();
    for part in &row.parts {
        let data = &part.data;
        if data.get("type").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(tool_name) = data.get("tool").and_then(Value::as_str) else {
            continue;
        };
        let tool_call_id = data
            .get("callID")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| part.id.clone());
        let state = data.get("state");
        let status = state
            .and_then(|state| state.get("status"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(status, "completed" | "error") {
            continue;
        }
        let input = state
            .and_then(|state| state.get("input"))
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));

        tool_calls.push(ModelToolCall {
            tool_call_id: tool_call_id.clone(),
            tool_name: tool_name.to_string(),
            input: input.clone(),
        });
        let content = match status {
            "completed" => state
                .and_then(|state| state.get("output"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => state
                .and_then(|state| state.get("error"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        results.push((tool_call_id, content));

        if status == "completed" {
            seed_file_state(file_state, tool_name, &input, state, ctx);
        }
    }

    if text.is_empty() && tool_calls.is_empty() {
        return;
    }
    messages.push(ModelMessage::Assistant {
        text: (!text.is_empty()).then_some(text),
        tool_calls,
    });
    for (tool_call_id, content) in results {
        messages.push(ModelMessage::ToolResult {
            tool_call_id,
            content,
        });
    }
}

/// 成功的 Read/Write/Edit 恢复文件记忆（错误不进文件记忆，与运行期语义一致）。
fn seed_file_state(
    file_state: &mut FileState,
    tool_name: &str,
    input: &Value,
    state: Option<&Value>,
    ctx: &ToolContext,
) {
    let Some(file_path) = input.get("file_path").and_then(Value::as_str) else {
        return;
    };
    match tool_name {
        "Read" => {
            let offset = input.get("offset").and_then(Value::as_i64);
            let limit = input.get("limit").and_then(Value::as_i64);
            let mtime = state
                .and_then(|state| state.get("metadata"))
                .and_then(|metadata| metadata.get("readFileState"))
                .and_then(|read_file_state| read_file_state.get("mtimeMs"))
                .and_then(Value::as_i64)
                .and_then(millis_to_system_time);
            file_state.seed_read(file_path, offset, limit, mtime, ctx);
        }
        "Write" | "Edit" => file_state.record_written(file_path, ctx),
        _ => {}
    }
}

fn collect_text(parts: &[session_store::PartRecord]) -> String {
    let mut text = String::new();
    for part in parts {
        if part.data.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        if let Some(chunk) = part.data.get("text").and_then(Value::as_str) {
            text.push_str(chunk);
        }
    }
    text
}

fn turn_number_from_id(turn_id: &str) -> Option<u64> {
    turn_id.strip_prefix("turn_")?.parse::<u64>().ok()
}

fn millis_to_system_time(millis: i64) -> Option<SystemTime> {
    let millis = u64::try_from(millis).ok()?;
    UNIX_EPOCH.checked_add(Duration::from_millis(millis))
}
