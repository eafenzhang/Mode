//! 快照模型、校验与有界化。
//!
//! 真源：`packages/shared/src/conversation-mirror.ts`。字段顺序就是 JSON 键序（serde 按声明序
//! 输出），有界化的字节预算按 `JSON.stringify` 的紧凑形态计算，所以两边字节数一致。

use serde::{Deserialize, Serialize};

pub const VERSION: u64 = 1;
pub const MIN_WINDOW_SIZE: usize = 10;
pub const MAX_RECORDS: usize = 20;
pub const MAX_MESSAGES_PER_SESSION: usize = 200;
pub const MAX_TEXT_CHARS_PER_MESSAGE: usize = 4_000;
pub const MAX_RECORD_BYTES: usize = 256 * 1024;
pub const MAX_SNAPSHOT_BYTES: usize = 512 * 1024;
const TRUNCATION_SUFFIX: &str = "…";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Message {
    pub role: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub session_id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: u64,
    pub truncated: bool,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub version: u64,
    pub origin_device_id: String,
    pub origin_label: String,
    pub generated_at: u64,
    pub workspace_path: String,
    pub records: Vec<Record>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    Invalid(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(formatter, "invalid conversation mirror snapshot: {reason}"),
        }
    }
}

impl std::error::Error for ValidationError {}

/// 解析并校验（对应 zod `.strict()` + `.trim().min(1)` 的语义）。
///
/// 与 TS 一致：`originDeviceId` / `originLabel` / `workspacePath` / `sessionId` 会被 **trim**；
/// 未知字段直接判无效；`version` 必须是 1。
pub fn validate_snapshot(value: &serde_json::Value) -> Result<Snapshot, ValidationError> {
    let mut snapshot: Snapshot = serde_json::from_value(value.clone())
        .map_err(|error| ValidationError::Invalid(error.to_string()))?;
    if snapshot.version != VERSION {
        return Err(ValidationError::Invalid("unsupported version".into()));
    }
    snapshot.origin_device_id = trim_non_empty(&snapshot.origin_device_id, "originDeviceId")?;
    snapshot.origin_label = trim_non_empty(&snapshot.origin_label, "originLabel")?;
    snapshot.workspace_path = trim_non_empty(&snapshot.workspace_path, "workspacePath")?;
    for record in &mut snapshot.records {
        record.session_id = trim_non_empty(&record.session_id, "sessionId")?;
        for message in &record.messages {
            if message.role != "user" && message.role != "assistant" {
                return Err(ValidationError::Invalid(format!("invalid role {}", message.role)));
            }
        }
    }
    Ok(snapshot)
}

fn trim_non_empty(value: &str, field: &str) -> Result<String, ValidationError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ValidationError::Invalid(format!("{field} must not be empty")));
    }
    Ok(trimmed.to_string())
}

/// 同一 origin 内按 sessionId 去重，保留 updatedAt 较新的一条；顺序保持首次出现的位置。
pub fn dedupe_records(records: &[Record]) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::with_capacity(records.len());
    let mut positions: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for record in records {
        match positions.get(&record.session_id) {
            Some(index) => {
                if record.updated_at >= out[*index].updated_at {
                    out[*index] = record.clone();
                }
            }
            None => {
                positions.insert(record.session_id.clone(), out.len());
                out.push(record.clone());
            }
        }
    }
    out
}

pub fn record_byte_size(record: &Record) -> usize {
    serde_json::to_vec(record).map(|bytes| bytes.len()).unwrap_or(0)
}

pub fn snapshot_byte_size(snapshot: &Snapshot) -> usize {
    serde_json::to_vec(snapshot).map(|bytes| bytes.len()).unwrap_or(0)
}

/// 截断到 `max_chars` 个 **UTF-16 码元**（JS `slice` 的口径），超出时补 `…`。
fn bound_text(text: &str, max_chars: usize) -> String {
    let mut utf16_units = 0usize;
    for (index, ch) in text.char_indices() {
        let units = ch.len_utf16();
        if utf16_units + units > max_chars {
            return format!("{}{TRUNCATION_SUFFIX}", &text[..index]);
        }
        utf16_units += units;
    }
    text.to_string()
}

/// 单条记录有界化：保留尾部消息、逐条截断文本，再按字节预算从**最旧**消息开始丢弃。
pub fn bound_record(record: &Record) -> Record {
    let kept_start = record.messages.len().saturating_sub(MAX_MESSAGES_PER_SESSION);
    let kept = &record.messages[kept_start..];
    let mut messages: Vec<Message> = kept
        .iter()
        .map(|message| {
            let text = bound_text(&message.text, MAX_TEXT_CHARS_PER_MESSAGE);
            if text == message.text {
                message.clone()
            } else {
                Message { text, ..message.clone() }
            }
        })
        .collect();
    let mut truncated = record.truncated
        || kept.len() < record.messages.len()
        || messages.iter().zip(kept).any(|(new, old)| new.text != old.text);

    while messages.len() > 1 {
        let candidate = Record { messages: messages.clone(), ..record.clone() };
        if record_byte_size(&candidate) <= MAX_RECORD_BYTES {
            break;
        }
        messages.remove(0);
        truncated = true;
    }

    Record {
        messages,
        truncated,
        ..record.clone()
    }
}

/// 快照有界化：去重 → 按 updatedAt 降序取前 20 → 逐条有界化 → 按字节预算从最旧记录开始丢弃。
pub fn bound_snapshot(snapshot: &Snapshot) -> Snapshot {
    let mut records = dedupe_records(&snapshot.records);
    records.sort_by_key(|record| std::cmp::Reverse(record.updated_at));
    records.truncate(MAX_RECORDS);
    let mut bounded: Vec<Record> = records.iter().map(bound_record).collect();

    while bounded.len() > 1 {
        let candidate = Snapshot { records: bounded.clone(), ..snapshot.clone() };
        if snapshot_byte_size(&candidate) <= MAX_SNAPSHOT_BYTES {
            break;
        }
        bounded.pop();
    }

    Snapshot {
        records: bounded,
        ..snapshot.clone()
    }
}
