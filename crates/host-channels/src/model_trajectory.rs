//! model-io 轨迹读取（真源 `packages/services/src/mode-agent/modelTrajectory.ts` +
//! `modelTrajectoryFileTail.ts`）。
//!
//! - model-io 由 Agent runner 落盘：`{home}/.mode/cli/{debug,rollout}/
//!   model-io-<sanitized(taskId)>.jsonl`，一条记录一行 JSON（`type: "model_io"`）。
//! - 只读该 session 的单文件，按 `sessionId === taskId` 精确匹配；从文件尾部读固定
//!   上限（32 MiB），从行中间开始时丢弃首行残片。
//! - 新 model-io 为省体积写 delta（`messagesKind: "delta"` + `messageOffset`）；
//!   读出时按上一条（已展开）记录还原完整上下文；`tail` 表示新起点，不拼接更早历史。
//! - 默认只返回最近 200 条调用；超出时 `truncated: true`。

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

const DEFAULT_LIMIT: usize = 200;
const MAX_READ_BYTES: u64 = 32 * 1024 * 1024;
const SESSION_TITLE_PROMPT_PREFIX: &str = "Generate a concise title for this coding session.";

/// 按进程环境解析 model-io 目录（去重、稳定顺序）。
pub fn resolve_model_io_dirs() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = home_dir() {
        roots.push(home.join(".mode").join("cli"));
    }
    if let Ok(base) = std::env::var("MODE_BASE_DIR") {
        let base = base.trim();
        if !base.is_empty() {
            roots.push(PathBuf::from(base).join(".mode").join("cli"));
        }
    }
    let mut dirs = Vec::new();
    for root in roots {
        for kind in ["debug", "rollout"] {
            let dir = root.join(kind);
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
}

/// 与 runner-debug 的 `sanitizeFileSegment` 一致：仅保留文件名安全字符。
pub fn sanitize_session_segment(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
            last_dash = ch == '-';
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    trimmed.chars().take(80).collect()
}

/// 按进程环境读取某 task 的模型调用轨迹。
pub fn read_model_trajectory(task_id: &str, limit: Option<f64>) -> Value {
    read_model_trajectory_from_dirs(task_id, limit, &resolve_model_io_dirs())
}

/// 指定目录读取（测试注入）。
pub fn read_model_trajectory_from_dirs(
    task_id: &str,
    limit: Option<f64>,
    dirs: &[PathBuf],
) -> Value {
    let safe_limit = match limit {
        Some(limit) if limit.is_finite() && limit > 0.0 => limit.trunc() as usize,
        _ => DEFAULT_LIMIT,
    };
    let sanitized = sanitize_session_segment(task_id);
    let file_name = format!(
        "model-io-{}.jsonl",
        if sanitized.is_empty() {
            "no-session"
        } else {
            &sanitized
        }
    );
    let mut source_files: Vec<String> = Vec::new();
    let mut raw_records: Vec<Map<String, Value>> = Vec::new();
    let mut input_truncated = false;

    for dir in dirs {
        let Ok(names) = std::fs::read_dir(dir) else {
            // 目录不存在（没跑过对应模式）或不可读，跳过。
            continue;
        };
        let exists = names.flatten().any(|entry| {
            entry.file_name().to_string_lossy() == file_name.as_str()
        });
        if !exists {
            continue;
        }
        let file_path = dir.join(&file_name);
        let Ok((text, truncated)) = read_trajectory_file_tail(&file_path) else {
            continue;
        };
        input_truncated |= truncated;
        let mut matched_in_file = false;
        for line in text.split('\n') {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            if parsed.get("type").and_then(Value::as_str) != Some("model_io") {
                continue;
            }
            if parsed.get("sessionId").and_then(Value::as_str) != Some(task_id) {
                continue;
            }
            raw_records.push(parsed);
            matched_in_file = true;
        }
        if matched_in_file {
            source_files.push(file_path.to_string_lossy().into_owned());
        }
    }

    // 按 startedAt 升序；同毫秒/缺失时间按 requestId（TS localeCompare 对 ASCII 等价）。
    raw_records.sort_by(|left, right| {
        let start_diff = to_time(as_string(left.get("startedAt")))
            - to_time(as_string(right.get("startedAt")));
        if start_diff != 0 {
            return start_diff.cmp(&0);
        }
        as_string(left.get("requestId"))
            .unwrap_or("")
            .cmp(as_string(right.get("requestId")).unwrap_or(""))
    });

    let expanded = expand_delta_records(&raw_records);
    let records: Vec<Value> = expanded.iter().map(map_record).collect();
    let truncated = input_truncated || records.len() > safe_limit;
    let trimmed: Vec<Value> = if truncated {
        records[records.len() - safe_limit..].to_vec()
    } else {
        records
    };

    serde_json::json!({
        "taskId": task_id,
        "available": true,
        "records": trimmed,
        "sourceFiles": source_files,
        "truncated": truncated,
    })
}

fn read_trajectory_file_tail(path: &Path) -> Result<(String, bool), std::io::Error> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(MAX_READ_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = Vec::new();
    file.take(MAX_READ_BYTES).read_to_end(&mut buffer)?;
    let mut text = String::from_utf8_lossy(&buffer).into_owned();
    if start > 0 {
        // 从行中间开始：丢弃不完整残行。
        text = match text.find('\n') {
            Some(index) => text[index + 1..].to_string(),
            None => String::new(),
        };
    }
    Ok((text, start > 0))
}

// ── delta 展开 ──

fn expand_delta_records(records: &[Map<String, Value>]) -> Vec<Map<String, Value>> {
    let mut expanded: Vec<Map<String, Value>> = Vec::with_capacity(records.len());
    let mut previous: Option<Map<String, Value>> = None;
    for record in records {
        let next = expand_record(record, previous.as_ref());
        expanded.push(next.clone());
        previous = Some(next);
    }
    expanded
}

fn expand_record(
    record: &Map<String, Value>,
    previous: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let mut next = record.clone();
    let Some(request) = as_object(record.get("request")) else {
        return next;
    };
    let previous_request = previous.and_then(|record| as_object(record.get("request")));
    let expanded_request = expand_request(request, previous_request);
    next.insert("request".to_string(), Value::Object(expanded_request));
    next
}

fn expand_request(
    request: &Map<String, Value>,
    previous: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let mut next = request.clone();
    // kind/offset 与目标同层（TS metadataSource = next）；先取快照再改集合，避免借用冲突。
    let metadata = next.clone();
    expand_message_collection(
        &mut next,
        previous,
        "messages",
        "messagesKind",
        "messageOffset",
        &metadata,
    );
    let metadata = next.clone();
    expand_message_collection(
        &mut next,
        previous,
        "sdkMessages",
        "sdkMessagesKind",
        "sdkMessageOffset",
        &metadata,
    );
    let metadata = next.clone();
    if let Some(body) = as_object(next.get("body")).cloned() {
        let mut next_body = body;
        let previous_body = previous.and_then(|request| as_object(request.get("body")));
        // body 的 kind/offset 在 request 顶层（metadataSource=next）。
        expand_message_collection(
            &mut next_body,
            previous_body,
            "messages",
            "bodyMessagesKind",
            "bodyMessageOffset",
            &metadata,
        );
        next.insert("body".to_string(), Value::Object(next_body));
    }
    next
}

fn expand_message_collection(
    target: &mut Map<String, Value>,
    previous: Option<&Map<String, Value>>,
    collection_key: &str,
    kind_key: &str,
    offset_key: &str,
    metadata_source: &Map<String, Value>,
) {
    match metadata_source.get(kind_key).and_then(Value::as_str) {
        // tail 是新的展开起点，不能继续拼接更早历史。
        Some("tail") => return,
        Some("delta") => {}
        _ => return,
    }
    let Some(Value::Array(delta)) = target.get(collection_key).cloned() else {
        return;
    };
    let Some(Value::Array(previous_messages)) =
        previous.and_then(|request| request.get(collection_key).cloned())
    else {
        return;
    };
    let Some(offset) = metadata_source
        .get(offset_key)
        .and_then(Value::as_u64)
        .map(|offset| offset as usize)
    else {
        return;
    };
    let mut merged: Vec<Value> = previous_messages
        .iter()
        .take(offset.min(previous_messages.len()))
        .cloned()
        .collect();
    merged.extend(delta);
    target.insert(collection_key.to_string(), Value::Array(merged));
}

// ── 单条记录映射 ──

fn map_record(record: &Map<String, Value>) -> Value {
    let request = as_object(record.get("request"));
    let response = as_object(record.get("response"));
    let model = as_object(record.get("model"));
    let error = as_object(record.get("error"));
    let query_source = as_string(record.get("querySource"))
        .map(str::to_string)
        .or_else(|| infer_query_source_from_request(request));
    let model_role = model.and_then(|model| as_string(model.get("role")));

    let mut mapped = Map::new();
    mapped.insert(
        "requestId".to_string(),
        Value::String(as_string(record.get("requestId")).unwrap_or("").to_string()),
    );
    mapped.insert(
        "attempt".to_string(),
        number_value(as_number(record.get("attempt")).unwrap_or(1.0)),
    );
    mapped.insert(
        "startedAt".to_string(),
        Value::String(as_string(record.get("startedAt")).unwrap_or("").to_string()),
    );
    for key in ["completedAt", "turnId", "traceId"] {
        if let Some(value) = as_string(record.get(key)) {
            mapped.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    if let Some(duration) = as_number(record.get("durationMs")) {
        mapped.insert("durationMs".to_string(), number_value(duration));
    }
    mapped.insert(
        "callSource".to_string(),
        classify_call_source(query_source.as_deref(), model_role),
    );
    let mut model_value = Map::new();
    for (key, source) in [
        ("modelId", "modelId"),
        ("providerId", "providerId"),
        ("role", "role"),
        ("source", "source"),
    ] {
        if let Some(value) = model.and_then(|model| as_string(model.get(source))) {
            model_value.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    mapped.insert("model".to_string(), Value::Object(model_value));

    let mut request_value = Map::new();
    request_value.insert(
        "messages".to_string(),
        Value::Array(map_messages(request.and_then(|request| request.get("messages")))),
    );
    request_value.insert(
        "toolNames".to_string(),
        Value::Array(
            as_string_array(request.and_then(|request| request.get("toolNames")))
                .into_iter()
                .map(Value::String)
                .collect(),
        ),
    );
    mapped.insert("request".to_string(), Value::Object(request_value));

    if let Some(response) = response {
        let mut response_value = Map::new();
        for key in ["finishReason", "text", "reasoningText", "responseId", "modelId"] {
            if let Some(value) = as_string(response.get(key)) {
                response_value.insert(key.to_string(), Value::String(value.to_string()));
            }
        }
        let tool_calls: Vec<Value> = response
            .get("toolCalls")
            .and_then(Value::as_array)
            .map(|calls| calls.iter().map(map_response_tool_call).collect())
            .unwrap_or_default();
        response_value.insert("toolCalls".to_string(), Value::Array(tool_calls));
        if let Some(usage) = map_usage(response.get("usage")) {
            response_value.insert("usage".to_string(), usage);
        }
        mapped.insert("response".to_string(), Value::Object(response_value));
    }

    if let Some(error) = error {
        let name = as_string(error.get("name"));
        let message = as_string(error.get("message"));
        if name.is_some() || message.is_some() {
            let mut error_value = Map::new();
            error_value.insert(
                "name".to_string(),
                Value::String(name.unwrap_or("Error").to_string()),
            );
            error_value.insert(
                "message".to_string(),
                Value::String(message.unwrap_or("").to_string()),
            );
            if let Some(stack) = as_string(error.get("stack")) {
                error_value.insert("stack".to_string(), Value::String(stack.to_string()));
            }
            mapped.insert("error".to_string(), Value::Object(error_value));
        }
    }

    Value::Object(mapped)
}

fn infer_query_source_from_request(request: Option<&Map<String, Value>>) -> Option<String> {
    let messages = request?.get("messages")?.as_array()?;
    let first = messages.first().and_then(Value::as_object)?;
    if first.get("role").and_then(Value::as_str) != Some("system") {
        return None;
    }
    let content = as_string(first.get("content"))?;
    content
        .starts_with(SESSION_TITLE_PROMPT_PREFIX)
        .then(|| "session_title".to_string())
}

fn classify_call_source(query_source: Option<&str>, model_role: Option<&str>) -> Value {
    let (kind, include_source) = match query_source {
        Some("main_turn") => ("main", true),
        Some("subagent") => ("subagent", true),
        Some("compact") => ("compact", true),
        Some(_) => ("sidecar", true),
        None if model_role == Some("compact") => ("compact", false),
        None if model_role == Some("subagent") => ("subagent", false),
        None => ("main", false),
    };
    let mut value = Map::new();
    value.insert("kind".to_string(), Value::String(kind.to_string()));
    if include_source {
        if let Some(source) = query_source {
            value.insert("querySource".to_string(), Value::String(source.to_string()));
        }
    }
    Value::Object(value)
}

fn map_messages(value: Option<&Value>) -> Vec<Value> {
    let Some(Value::Array(entries)) = value else {
        return Vec::new();
    };
    entries
        .iter()
        .map(|entry| {
            let message = entry.as_object();
            let role = message
                .and_then(|message| as_string(message.get("role")))
                .unwrap_or("unknown");
            let tool = message.map(|message| {
                (
                    as_string(message.get("toolCallId"))
                        .or_else(|| as_string(message.get("tool_call_id")))
                        .or_else(|| as_string(message.get("tool_use_id")))
                        .map(str::to_string),
                    as_string(message.get("toolName"))
                        .or_else(|| as_string(message.get("name")))
                        .map(str::to_string),
                    message.get("isError").and_then(Value::as_bool) == Some(true)
                        || message.get("is_error").and_then(Value::as_bool) == Some(true),
                )
            });
            let parts = message
                .and_then(|message| message.get("content"))
                .map(|content| map_content(content, role, tool.as_ref()))
                .unwrap_or_default();
            let mut mapped = Map::new();
            mapped.insert("role".to_string(), Value::String(role.to_string()));
            mapped.insert("parts".to_string(), Value::Array(parts));
            Value::Object(mapped)
        })
        .collect()
}

type MessageToolMeta = (Option<String>, Option<String>, bool);

fn map_content(
    content: &Value,
    role: &str,
    message_tool: Option<&MessageToolMeta>,
) -> Vec<Value> {
    if let Some(text) = content.as_str() {
        if text.is_empty() {
            return Vec::new();
        }
        if role == "tool" {
            // tool 角色字符串内容即工具输出；顶层关联字段必须保留。
            let mut part = Map::new();
            part.insert("kind".to_string(), Value::String("tool-result".to_string()));
            if let Some(tool) = message_tool {
                if let Some(id) = &tool.0 {
                    part.insert("toolCallId".to_string(), Value::String(id.clone()));
                }
                if let Some(name) = &tool.1 {
                    part.insert("toolName".to_string(), Value::String(name.clone()));
                }
            }
            let is_error = message_tool.map(|tool| tool.2).unwrap_or(false);
            part.insert("output".to_string(), try_parse_json(text, is_error));
            return vec![Value::Object(part)];
        }
        let mut part = Map::new();
        part.insert("kind".to_string(), Value::String("text".to_string()));
        part.insert("text".to_string(), Value::String(text.to_string()));
        return vec![Value::Object(part)];
    }
    let Some(content) = content.as_array() else {
        return Vec::new();
    };
    let tool = if role == "tool" { message_tool } else { None };
    content
        .iter()
        .map(|raw_part| map_part(raw_part, tool))
        .collect()
}

fn map_part(raw_part: &Value, message_tool: Option<&MessageToolMeta>) -> Value {
    let Some(part) = raw_part.as_object() else {
        let mut value = Map::new();
        value.insert("kind".to_string(), Value::String("unknown".to_string()));
        value.insert("raw".to_string(), raw_part.clone());
        return Value::Object(value);
    };
    let mut mapped = Map::new();
    match part.get("type").and_then(Value::as_str) {
        Some("text") => {
            mapped.insert("kind".to_string(), Value::String("text".to_string()));
            mapped.insert(
                "text".to_string(),
                Value::String(as_string(part.get("text")).unwrap_or("").to_string()),
            );
        }
        Some("reasoning") => {
            mapped.insert("kind".to_string(), Value::String("reasoning".to_string()));
            mapped.insert(
                "text".to_string(),
                Value::String(as_string(part.get("text")).unwrap_or("").to_string()),
            );
        }
        Some("tool-call") => {
            mapped.insert("kind".to_string(), Value::String("tool-call".to_string()));
            if let Some(id) = as_string(part.get("toolCallId")) {
                mapped.insert("toolCallId".to_string(), Value::String(id.to_string()));
            }
            mapped.insert(
                "toolName".to_string(),
                Value::String(
                    as_string(part.get("toolName"))
                        .unwrap_or("tool")
                        .to_string(),
                ),
            );
            if let Some(input) = first_non_null(part.get("input"), part.get("args")) {
                mapped.insert("input".to_string(), input.clone());
            }
        }
        Some("tool-result") => {
            mapped.insert("kind".to_string(), Value::String("tool-result".to_string()));
            let tool_call_id = as_string(part.get("toolCallId"))
                .map(str::to_string)
                .or_else(|| message_tool.and_then(|tool| tool.0.clone()));
            if let Some(id) = tool_call_id {
                mapped.insert("toolCallId".to_string(), Value::String(id));
            }
            let tool_name = as_string(part.get("toolName"))
                .map(str::to_string)
                .or_else(|| message_tool.and_then(|tool| tool.1.clone()));
            if let Some(name) = tool_name {
                mapped.insert("toolName".to_string(), Value::String(name));
            }
            if let Some(output) = first_non_null(part.get("output"), part.get("result")) {
                mapped.insert("output".to_string(), output.clone());
            }
        }
        Some("image") | Some("file") => {
            mapped.insert("kind".to_string(), Value::String("image".to_string()));
            if let Some(media_type) = as_string(part.get("mediaType")) {
                mapped.insert(
                    "mediaType".to_string(),
                    Value::String(media_type.to_string()),
                );
            }
        }
        _ => {
            mapped.insert("kind".to_string(), Value::String("unknown".to_string()));
            mapped.insert("raw".to_string(), raw_part.clone());
        }
    }
    Value::Object(mapped)
}

fn map_response_tool_call(raw_tool_call: &Value) -> Value {
    let Some(tool_call) = raw_tool_call.as_object() else {
        let mut value = Map::new();
        value.insert("kind".to_string(), Value::String("unknown".to_string()));
        value.insert("raw".to_string(), raw_tool_call.clone());
        return Value::Object(value);
    };
    let mut mapped = Map::new();
    mapped.insert("kind".to_string(), Value::String("tool-call".to_string()));
    if let Some(id) = as_string(tool_call.get("id"))
        .or_else(|| as_string(tool_call.get("toolCallId")))
    {
        mapped.insert("toolCallId".to_string(), Value::String(id.to_string()));
    }
    mapped.insert(
        "toolName".to_string(),
        Value::String(
            as_string(tool_call.get("name"))
                .or_else(|| as_string(tool_call.get("toolName")))
                .unwrap_or("tool")
                .to_string(),
        ),
    );
    if let Some(input) = first_non_null(tool_call.get("input"), tool_call.get("args")) {
        mapped.insert("input".to_string(), input.clone());
    }
    Value::Object(mapped)
}

fn map_usage(value: Option<&Value>) -> Option<Value> {
    let usage = as_object(value)?;
    let mut mapped = Map::new();
    for (key, source) in [
        ("inputTokens", "inputTokens"),
        ("outputTokens", "outputTokens"),
        ("totalTokens", "totalTokens"),
        ("cacheReadTokens", "cacheReadTokens"),
        ("reasoningTokens", "reasoningTokens"),
    ] {
        if let Some(number) = as_number(usage.get(source)) {
            mapped.insert(key.to_string(), number_value(number));
        }
    }
    Some(Value::Object(mapped))
}

/// `a ?? b`：null/缺失时回退 b（TS 语义）。
fn first_non_null<'a>(primary: Option<&'a Value>, fallback: Option<&'a Value>) -> Option<&'a Value> {
    primary
        .filter(|value| !value.is_null())
        .or_else(|| fallback.filter(|value| !value.is_null()))
}

fn try_parse_json(value: &str, is_error: bool) -> Value {
    if is_error {
        return serde_json::json!({ "type": "error-text", "value": value });
    }
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
}

// ── 基础取值 ──

fn as_object(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

fn as_string(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

fn as_number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|number| number.is_finite())
}

fn as_string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn number_value(number: f64) -> Value {
    if number.fract() == 0.0 && number >= i64::MIN as f64 && number <= i64::MAX as f64 {
        Value::Number(serde_json::Number::from(number as i64))
    } else {
        serde_json::Number::from_f64(number)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// `Date.parse` 等价：支持 `YYYY-MM-DD[THH:MM:SS[.fff]][Z|±HH:MM]`。
/// 无法解析返回 0（TS `Number.isNaN → 0`）；缺失时区按 UTC（model-io 写 toISOString）。
fn to_time(value: Option<&str>) -> i64 {
    value.map(parse_time_ms).unwrap_or(0)
}

pub(crate) fn parse_time_ms(value: &str) -> i64 {
    let bytes = value.as_bytes();
    if bytes.len() < 10 {
        return 0;
    }
    let digit = |index: usize| -> Option<i64> {
        let byte = *bytes.get(index)?;
        (byte.is_ascii_digit()).then(|| i64::from(byte - b'0'))
    };
    let two = |index: usize| -> Option<i64> {
        Some(digit(index)? * 10 + digit(index + 1)?)
    };
    let four = |index: usize| -> Option<i64> {
        Some(digit(index)? * 1000 + digit(index + 1)? * 100 + digit(index + 2)? * 10 + digit(index + 3)?)
    };
    let Some(year) = four(0) else { return 0 };
    if bytes.get(4) != Some(&b'-') {
        return 0;
    }
    let Some(month) = two(5) else { return 0 };
    if bytes.get(7) != Some(&b'-') {
        return 0;
    }
    let Some(day) = two(8) else { return 0 };
    let (mut hour, mut minute, mut second, mut millis) = (0_i64, 0_i64, 0_i64, 0_i64);
    let mut index = 10;
    if matches!(bytes.get(index), Some(b'T') | Some(b' ')) {
        index += 1;
        let Some(h) = two(index) else { return 0 };
        hour = h;
        if bytes.get(index + 2) != Some(&b':') {
            return 0;
        }
        let Some(m) = two(index + 3) else { return 0 };
        minute = m;
        index += 5;
        if bytes.get(index) == Some(&b':') {
            let Some(s) = two(index + 1) else { return 0 };
            second = s;
            index += 3;
        }
        if bytes.get(index) == Some(&b'.') {
            index += 1;
            let start = index;
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index += 1;
            }
            let digits = &value[start..index];
            let padded = format!("{:0<3}", &digits[..digits.len().min(3)]);
            millis = padded.parse::<i64>().unwrap_or(0);
        }
    }
    let mut offset_minutes = 0_i64;
    match bytes.get(index) {
        Some(b'Z') | Some(b'z') | None => {}
        Some(b'+') | Some(b'-') => {
            let sign = if bytes[index] == b'-' { -1 } else { 1 };
            let Some(oh) = two(index + 1) else { return 0 };
            let Some(om) = two(index + 4) else { return 0 };
            offset_minutes = sign * (oh * 60 + om);
        }
        _ => {}
    }
    let days = days_from_civil(year, month, day);
    (days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60) * 1_000 + millis
}

/// Howard Hinnant `days_from_civil`（1970-01-01 起的天数）。
fn days_from_civil(mut year: i64, month: i64, day: i64) -> i64 {
    if month <= 2 {
        year -= 1;
    }
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
