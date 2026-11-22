//! Claude 导入 Stage 2：完整会话解析 + 任务文件/legacy 快照（真源
//! `claudeNativeSessionImportParser.ts`、`buildImportedClaudeTaskFile.ts`、
//! `persistImportedClaudeTask.ts` 与 `claudeNativeSessionImportRepo` 的文件复制）。
//!
//! 编排（复制 → createSession(importedHistory) → 索引回写 → 事件）在 mode-task
//! 频道完成；本模块只做可单测的纯解析与文件 IO。

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};
use sha2::Digest as _;

use crate::claude_import::{
    derive_session_title, extract_head_info, extract_text_field, has_sidechain_marker,
    is_claude_worktree_workspace_path, native_projects_roots, normalize_path_for_comparison,
    read_entry_model, read_entry_timestamp, read_entry_workspace_path, read_json_lines_head,
    sanitize_claude_visible_text,
};

const HEAD_LINES: usize = 16;
const SEARCH_TEXT_MAX_CHARS: usize = 200_000;
const FILTERED_PATHS: [&str; 4] = [
    "meta.mode",
    "meta.model",
    "meta.provider",
    "messages[].model",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedMessage {
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub turn_index: i64,
}

#[derive(Debug, Clone)]
pub struct ImportedSessionSource {
    pub provider: &'static str,
    pub session_id: String,
    pub workspace_path: String,
    pub source_path: PathBuf,
    pub created_at: i64,
    pub updated_at: i64,
    pub title: Option<String>,
    pub model: Option<String>,
    pub messages: Vec<ImportedMessage>,
}

/// 扫描命中：会话归属 workspace、源文件、mtime 与 head 里的 createdAt。
#[derive(Debug, Clone)]
pub struct ImportableSession {
    pub workspace_path: String,
    pub source_path: PathBuf,
    pub updated_at: i64,
    pub created_at: Option<i64>,
}

/// 按进程环境查找单个可导入 session（导入入口；workspace 过滤与扫描同口径）。
pub fn find_importable_session(
    workspace_path: Option<&str>,
    session_id: &str,
) -> Option<ImportableSession> {
    let roots = native_projects_roots();
    find_importable_session_from_roots(&roots, workspace_path, session_id)
}

pub fn find_importable_session_from_roots(
    roots: &[PathBuf],
    workspace_path: Option<&str>,
    session_id: &str,
) -> Option<ImportableSession> {
    let workspace_key = workspace_path.map(normalize_path_for_comparison);
    for root in roots {
        let Some(source_path) = find_session_file(root, session_id) else {
            continue;
        };
        let Ok(metadata) = std::fs::metadata(&source_path) else {
            continue;
        };
        let updated_at = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0);
        let Ok(entries) = read_json_lines_head(&source_path, HEAD_LINES) else {
            continue;
        };
        if has_sidechain_marker(&entries) {
            return None;
        }
        let Some(head) = extract_head_info(&entries) else {
            continue;
        };
        let Some(candidate_workspace) = head.workspace_path else {
            continue;
        };
        // 直接按 sessionId 导入也必须复用扫描边界，防止导入临时 worktree。
        if is_claude_worktree_workspace_path(&candidate_workspace) {
            return None;
        }
        if workspace_key
            .as_deref()
            .is_some_and(|key| normalize_path_for_comparison(&candidate_workspace) != key)
        {
            continue;
        }
        return Some(ImportableSession {
            workspace_path: candidate_workspace,
            source_path,
            updated_at,
            created_at: head.created_at,
        });
    }
    None
}

fn find_session_file(dir: &Path, session_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let target = format!("{session_id}.jsonl");
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if matches!(name.as_str(), "subagents" | "worktree" | "worktrees") {
                continue;
            }
            if let Some(found) = find_session_file(&path, session_id) {
                return Some(found);
            }
            continue;
        }
        if file_type.is_file() && entry.file_name().to_string_lossy() == target {
            return Some(path);
        }
    }
    None
}

/// 读取全量 JSONL；任一行损坏即整体失败（与 TS `readJsonLinesFile` 一致）。
fn read_json_lines_file(path: &Path) -> Result<Vec<Value>, String> {
    let raw = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut records = Vec::new();
    for (index, line) in raw.split('\n').enumerate() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).map_err(|error| {
            format!(
                "[claude-native] 解析 JSONL 失败 {}:{} {error}",
                path.display(),
                index + 1
            )
        })?;
        records.push(value);
    }
    Ok(records)
}

/// 完整解析入口（TS `parseClaudeNativeSessionFile`）。
pub fn parse_session_file(
    file_path: &Path,
    workspace_path: &str,
    session_id: &str,
    source_path: Option<&Path>,
    fallback_created_at: Option<i64>,
    fallback_updated_at: Option<i64>,
) -> Result<ImportedSessionSource, String> {
    let records = read_json_lines_file(file_path)?;
    let metadata = std::fs::metadata(file_path)
        .map_err(|error| format!("[claude-native] 读取 session 元数据失败: {error}"))?;
    let birthtime = metadata
        .created()
        .ok()
        .and_then(|created| created.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64);
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as i64);
    parse_session_records(
        &records,
        workspace_path,
        session_id,
        source_path.unwrap_or(file_path),
        fallback_created_at.or(birthtime),
        fallback_updated_at.or(mtime),
    )
}

pub fn parse_session_records(
    records: &[Value],
    workspace_path: &str,
    session_id: &str,
    source_path: &Path,
    fallback_created_at: Option<i64>,
    fallback_updated_at: Option<i64>,
) -> Result<ImportedSessionSource, String> {
    let mut messages: Vec<ImportedMessage> = Vec::new();
    let mut detected_workspace_path: Option<String> = None;
    let mut first_visible_user_timestamp: Option<i64> = None;
    let mut updated_at = fallback_updated_at;
    let mut current_turn_index: i64 = -1;
    let mut model: Option<String> = None;
    let mut pending_assistant_content = String::new();
    let mut pending_assistant_timestamp: Option<i64> = None;

    for entry in records {
        let user_text = extract_import_user_text(entry);
        if detected_workspace_path.is_none() && user_text.is_some() {
            detected_workspace_path = read_entry_workspace_path(entry);
        }
        if let Some(user_text) = user_text {
            flush_pending_assistant(
                &mut messages,
                &mut pending_assistant_content,
                &mut pending_assistant_timestamp,
                current_turn_index,
                &model,
                &mut updated_at,
            );
            current_turn_index += 1;
            let timestamp = read_entry_timestamp(entry)
                .or(updated_at)
                .unwrap_or_else(now_ms);
            if first_visible_user_timestamp.is_none() {
                first_visible_user_timestamp = Some(timestamp);
            }
            updated_at = Some(timestamp);
            messages.push(ImportedMessage {
                role: "user".to_string(),
                content: user_text,
                timestamp,
                model: None,
                turn_index: current_turn_index,
            });
            continue;
        }

        let assistant_text = extract_import_assistant_text(entry);
        if detected_workspace_path.is_none() && assistant_text.is_some() {
            detected_workspace_path = read_entry_workspace_path(entry);
        }
        let Some(assistant_text) = assistant_text else {
            continue;
        };
        if current_turn_index < 0 {
            continue;
        }
        // 导入 snapshot 的模型只来自可见模型回复，避免 synthetic 占位污染续聊。
        if model.is_none() {
            model = read_entry_model(entry);
        }
        pending_assistant_content.push_str(&assistant_text);
        pending_assistant_timestamp = read_entry_timestamp(entry)
            .or(pending_assistant_timestamp)
            .or(updated_at);
        updated_at = pending_assistant_timestamp.or(updated_at);
    }

    flush_pending_assistant(
        &mut messages,
        &mut pending_assistant_content,
        &mut pending_assistant_timestamp,
        current_turn_index,
        &model,
        &mut updated_at,
    );

    if messages.is_empty() {
        return Err(format!(
            "[claude-native] Claude 原生 session {session_id} 没有可导入的可见消息"
        ));
    }

    let now = now_ms();
    let resolved_workspace = detected_workspace_path.unwrap_or_else(|| workspace_path.to_string());
    let created_at = first_visible_user_timestamp
        .or(fallback_created_at)
        .or(updated_at)
        .unwrap_or(now);
    let updated_at = updated_at
        .or(first_visible_user_timestamp)
        .or(fallback_created_at)
        .unwrap_or(now);
    let first_user_content = messages
        .iter()
        .find(|message| message.role == "user")
        .map(|message| message.content.as_str())
        .unwrap_or("");
    Ok(ImportedSessionSource {
        provider: "claude",
        session_id: session_id.to_string(),
        workspace_path: resolved_workspace,
        source_path: source_path.to_path_buf(),
        created_at,
        updated_at,
        title: Some(derive_session_title(first_user_content)),
        model,
        messages,
    })
}

fn flush_pending_assistant(
    messages: &mut Vec<ImportedMessage>,
    pending_content: &mut String,
    pending_timestamp: &mut Option<i64>,
    current_turn_index: i64,
    model: &Option<String>,
    updated_at: &mut Option<i64>,
) {
    if pending_content.is_empty() || current_turn_index < 0 {
        pending_content.clear();
        *pending_timestamp = None;
        return;
    }
    // Claude 一轮回答可能拆成多条事件：遇到下一条可见 user 或文件结束时聚合。
    messages.push(ImportedMessage {
        role: "assistant".to_string(),
        content: std::mem::take(pending_content),
        timestamp: pending_timestamp
            .take()
            .or(*updated_at)
            .unwrap_or_else(now_ms),
        model: model.clone(),
        turn_index: current_turn_index,
    });
}

fn extract_import_user_text(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let message = entry.get("message").and_then(Value::as_object);
    if entry.get("isMeta").and_then(Value::as_bool) == Some(true)
        || message
            .and_then(|message| message.get("isMeta"))
            .and_then(Value::as_bool)
            == Some(true)
    {
        return None;
    }
    let content = message
        .and_then(|message| message.get("content"))
        .or_else(|| {
            entry
                .get("request")
                .and_then(Value::as_object)
                .and_then(|request| request.get("prompt"))
        })?;
    if let Some(text) = content.as_str() {
        let normalized = sanitize_claude_visible_text(text);
        return (!normalized.is_empty()).then_some(normalized);
    }
    let parts = content.as_array()?;
    let mut visible: Vec<String> = Vec::new();
    for item in parts {
        if let Some(text) = item.as_str() {
            let normalized = sanitize_claude_visible_text(text);
            if !normalized.is_empty() {
                visible.push(normalized);
            }
            continue;
        }
        let Some(object) = item.as_object() else {
            continue;
        };
        if object.get("type").and_then(Value::as_str) == Some("tool_result") {
            continue;
        }
        let normalized = sanitize_claude_visible_text(&extract_text_field(item));
        if !normalized.is_empty() {
            visible.push(normalized);
        }
    }
    (!visible.is_empty()).then(|| visible.join("\n\n"))
}

fn extract_import_assistant_text(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = entry.get("message").and_then(Value::as_object);
    let is_api_error = entry.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true)
        || message
            .and_then(|message| message.get("isApiErrorMessage"))
            .and_then(Value::as_bool)
            == Some(true);
    // synthetic 控制占位（"No response requested."）不可见；API Error 是用户可见气泡。
    if read_entry_model(entry).as_deref() == Some("<synthetic>") && !is_api_error {
        return None;
    }
    let content = message.and_then(|message| message.get("content"))?;
    let text = if let Some(text) = content.as_str() {
        sanitize_claude_visible_text(text)
    } else {
        let parts = content.as_array()?;
        let mut visible: Vec<String> = Vec::new();
        for item in parts {
            if let Some(text) = item.as_str() {
                let normalized = sanitize_claude_visible_text(text);
                if !normalized.is_empty() {
                    visible.push(normalized);
                }
                continue;
            }
            let Some(object) = item.as_object() else {
                continue;
            };
            if object.get("type").and_then(Value::as_str) != Some("text") {
                continue;
            }
            let normalized = sanitize_claude_visible_text(&extract_text_field(item));
            if !normalized.is_empty() {
                visible.push(normalized);
            }
        }
        visible.concat()
    };
    let stripped = strip_synthetic_no_response_placeholder(&text);
    (!stripped.is_empty()).then_some(stripped)
}

/// TS `stripClaudeNativeSyntheticNoResponsePlaceholderText`。
fn strip_synthetic_no_response_placeholder(text: &str) -> String {
    let trimmed = text.trim_end();
    match trimmed.strip_suffix("No response requested.") {
        Some(prefix) => prefix.trim_end().to_string(),
        None => trimmed.to_string(),
    }
}

/// TS `buildImportedClaudeTaskId`：sha256("claude:{path}:{sid}") 前 24 位。
pub fn build_imported_task_id(workspace_path: &str, session_id: &str) -> String {
    let digest = sha2::Sha256::digest(format!("claude:{workspace_path}:{session_id}").as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("claude-import-{}", &hex[..24])
}

/// 任务文件（legacy `.mode-session` 快照）：meta + messages，按 filter 路径清洗来源运行态。
pub fn build_task_file(
    source: &ImportedSessionSource,
    task_id_override: Option<&str>,
) -> Result<Value, String> {
    if source.messages.is_empty() {
        return Err(format!(
            "[claude-native] external session {} 没有可导入的可见消息",
            source.session_id
        ));
    }
    let task_id =
        task_id_override.map(str::to_string).unwrap_or_else(|| {
            build_imported_task_id(&source.workspace_path, &source.session_id)
        });
    let trace_id = uuid::Uuid::new_v4().to_string();
    let created_at = source.created_at;
    let updated_at = source.updated_at.max(created_at);
    let title = derive_imported_title(source.title.as_deref(), &source.messages);
    let mut meta = json!({
        "taskId": task_id,
        "traceId": trace_id,
        "title": title,
        "workspacePath": source.workspace_path,
        "createdAt": created_at,
        "updatedAt": updated_at,
        "mode": "build",
        "migrationSource": "claudeCode",
        "status": "completed",
    });
    if let Some(model) = &source.model {
        meta["model"] = json!(model);
    }
    let messages: Vec<Value> = source
        .messages
        .iter()
        .filter_map(|message| serde_json::to_value(message).ok())
        .collect();
    let mut file = json!({ "meta": meta, "messages": messages });
    filter_imported_claude_paths(&mut file, &FILTERED_PATHS);
    Ok(file)
}

fn derive_imported_title(fallback_title: Option<&str>, messages: &[ImportedMessage]) -> String {
    if let Some(title) = fallback_title.map(str::trim).filter(|title| !title.is_empty()) {
        return title.to_string();
    }
    messages
        .iter()
        .find(|message| message.role == "user" && !message.content.trim().is_empty())
        .map(|message| derive_session_title(&message.content))
        .unwrap_or_else(|| "Imported session".to_string())
}

/// 按 TS filter 路径删除来源运行态（meta.mode/model/provider、messages[].model）。
fn filter_imported_claude_paths(file: &mut Value, paths: &[&str]) {
    for path in paths {
        let segments: Vec<&str> = path.split('.').collect();
        delete_path(file, &segments);
    }
}

fn delete_path(value: &mut Value, segments: &[&str]) {
    let Some((segment, rest)) = segments.split_first() else {
        return;
    };
    if let Some(key) = segment.strip_suffix("[]") {
        let Some(array) = value.get_mut(key).and_then(Value::as_array_mut) else {
            return;
        };
        for item in array {
            delete_path(item, rest);
        }
        return;
    }
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if rest.is_empty() {
        object.remove(*segment);
        return;
    }
    if let Some(child) = object.get_mut(*segment) {
        delete_path(child, rest);
    }
}

/// TS `buildSearchableTextFromMessages`：逐条 trim，跳过空内容，`\n` 连接，UTF-16 上限 20 万。
pub fn build_searchable_text(messages: &[ImportedMessage]) -> String {
    let mut output = String::new();
    let mut used_units = 0_usize;
    for message in messages {
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }
        let separator = if used_units > 0 { "\n" } else { "" };
        let next_units = separator.encode_utf16().count() + content.encode_utf16().count();
        if used_units + next_units > SEARCH_TEXT_MAX_CHARS {
            let remaining = SEARCH_TEXT_MAX_CHARS.saturating_sub(used_units);
            output.push_str(&take_utf16_units(&format!("{separator}{content}"), remaining));
            break;
        }
        output.push_str(separator);
        output.push_str(content);
        used_units += next_units;
    }
    output
}

fn take_utf16_units(text: &str, max_units: usize) -> String {
    let mut units = 0_usize;
    for (index, ch) in text.char_indices() {
        let width = ch.len_utf16();
        if units + width > max_units {
            return text[..index].to_string();
        }
        units += width;
    }
    text.to_string()
}

/// 导入副本：`{data_root}/v2/agent-config/claude/{workspaceHash}/projects/{relative}`。
pub fn copy_session_file(
    data_root: &Path,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    source_path: &Path,
) -> Result<(PathBuf, bool), String> {
    let source_text = source_path.to_string_lossy();
    // 两种分隔符都试；marker 长度必须含末尾分隔符（否则相对路径变成 rooted path，
    // PathBuf::join 会丢掉 data_root 前缀）。
    const BACK_MARKER: &str = "\\projects\\";
    const FORWARD_MARKER: &str = "/projects/";
    let marker = match (
        source_text.rfind(BACK_MARKER),
        source_text.rfind(FORWARD_MARKER),
    ) {
        (Some(back), Some(_fwd)) => Some((back, BACK_MARKER.len())),
        (Some(back), None) => Some((back, BACK_MARKER.len())),
        (None, Some(fwd)) => Some((fwd, FORWARD_MARKER.len())),
        (None, None) => None,
    };
    let Some((index, marker_len)) = marker else {
        return Err(format!(
            "[claude-native] Claude 原生 session 路径非法: {}",
            source_text
        ));
    };
    let relative = &source_text[index + marker_len..];
    let hash = workspace_identity::workspace_hash_for(workspace_path, workspace_identity);
    let destination = data_root
        .join("v2")
        .join("agent-config")
        .join("claude")
        .join(hash)
        .join("projects")
        .join(relative.replace('\\', std::path::MAIN_SEPARATOR_STR));
    let existed_before = destination.exists();
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp = destination.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::copy(source_path, &temp).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, &destination).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        error.to_string()
    })?;
    Ok((destination, !existed_before))
}

/// legacy 快照：`{data_root}/v2/sessions/{hash}/{taskId}.json`（2 空格缩进 + 换行）。
pub fn write_legacy_snapshot(
    data_root: &Path,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    task_id: &str,
    task_file: &Value,
) -> Result<PathBuf, String> {
    let hash = workspace_identity::workspace_hash_for(workspace_path, workspace_identity);
    let directory = data_root.join("v2").join("sessions").join(hash);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("{task_id}.json"));
    let serialized = serde_json::to_string_pretty(task_file).map_err(|error| error.to_string())?;
    let temp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&temp, format!("{serialized}\n")).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        error.to_string()
    })?;
    Ok(path)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "claude-import-persist-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos() as u64)
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn entry(value: Value) -> Value {
        value
    }

    fn user(text: &str, timestamp: i64) -> Value {
        entry(json!({
            "type": "user",
            "cwd": "D:/work/app",
            "timestamp": timestamp,
            "message": { "role": "user", "content": text }
        }))
    }

    fn assistant(text: &str, timestamp: i64, model: &str) -> Value {
        entry(json!({
            "type": "assistant",
            "timestamp": timestamp,
            "message": { "role": "assistant", "model": model,
                         "content": [{ "type": "text", "text": text }] }
        }))
    }

    #[test]
    fn parses_turns_merges_assistant_and_skips_synthetic() {
        // 真实 Claude 时间戳是 epoch 毫秒（>1e12）或 ISO 字符串；<1e9 的裸数字在
        // TS `toTimestampMs` 里也会被丢弃并回退到 updatedAt。
        let records = vec![
            user("First question", 1_700_000_000_000),
            assistant("Part one.", 1_700_000_000_100, "claude-3"),
            assistant("Part two.", 1_700_000_000_200, "claude-3"),
            // synthetic 占位：不可见。
            entry(json!({
                "type": "assistant",
                "timestamp": 1_700_000_000_250_i64,
                "message": { "model": "<synthetic>",
                             "content": [{ "type": "text", "text": "No response requested." }] }
            })),
            user("Second question", 1_700_000_000_300),
            // API error assistant 是用户可见气泡，必须保留。
            entry(json!({
                "type": "assistant",
                "timestamp": 1_700_000_000_400_i64,
                "isApiErrorMessage": true,
                "message": { "model": "claude-3",
                             "content": [{ "type": "text", "text": "API error occurred" }] }
            })),
        ];
        let source = parse_session_records(
            &records,
            "D:/work/app",
            "session-1",
            Path::new("D:/claude/session-1.jsonl"),
            Some(1_699_999_999_000),
            Some(1_700_000_001_000),
        )
        .expect("parse");
        assert_eq!(source.workspace_path, "D:/work/app");
        assert_eq!(source.created_at, 1_700_000_000_000);
        assert_eq!(source.updated_at, 1_700_000_000_400);
        assert_eq!(source.model.as_deref(), Some("claude-3"));
        assert_eq!(source.messages.len(), 4, "{:?}", source.messages);
        assert_eq!(source.messages[0].role, "user");
        assert_eq!(source.messages[0].turn_index, 0);
        assert_eq!(source.messages[1].role, "assistant");
        // 每个 text part 先各自 trim，再按无分隔符拼接（TS 同）。
        assert_eq!(source.messages[1].content, "Part one.Part two.");
        assert_eq!(source.messages[1].turn_index, 0);
        assert_eq!(source.messages[1].model.as_deref(), Some("claude-3"));
        assert_eq!(source.messages[2].turn_index, 1);
        assert_eq!(source.messages[3].content, "API error occurred");
        assert_eq!(source.title.as_deref(), Some("First question"));
    }

    #[test]
    fn task_file_filters_source_runtime_paths() {
        let source = ImportedSessionSource {
            provider: "claude",
            session_id: "session-1".to_string(),
            workspace_path: "D:/work/app".to_string(),
            source_path: PathBuf::from("D:/claude/session-1.jsonl"),
            created_at: 1_000,
            updated_at: 1_200,
            title: Some("标题".to_string()),
            model: Some("claude-3".to_string()),
            messages: vec![
                ImportedMessage {
                    role: "user".to_string(),
                    content: "hello".to_string(),
                    timestamp: 1_000,
                    model: None,
                    turn_index: 0,
                },
                ImportedMessage {
                    role: "assistant".to_string(),
                    content: "hi".to_string(),
                    timestamp: 1_100,
                    model: Some("claude-3".to_string()),
                    turn_index: 0,
                },
            ],
        };
        let file = build_task_file(&source, Some("task-1")).expect("task file");
        assert_eq!(file["meta"]["taskId"], "task-1");
        assert_eq!(file["meta"]["title"], "标题");
        assert_eq!(file["meta"]["status"], "completed");
        assert_eq!(file["meta"]["migrationSource"], "claudeCode");
        // filter：meta.mode/model/provider 与 messages[].model 被清除。
        assert!(file["meta"].get("mode").is_none(), "{file}");
        assert!(file["meta"].get("model").is_none(), "{file}");
        assert!(file["meta"].get("provider").is_none(), "{file}");
        assert!(file["messages"][1].get("model").is_none(), "{file}");
        // 内容与 turnIndex 保留。
        assert_eq!(file["messages"][1]["content"], "hi");
        assert_eq!(file["messages"][1]["turnIndex"], 0);
        // taskId 生成：sha256 前 24 位 + 前缀。
        let generated = build_imported_task_id("D:/work/app", "session-1");
        assert!(generated.starts_with("claude-import-"));
        assert_eq!(generated.len(), "claude-import-".len() + 24);
    }

    #[test]
    fn searchable_text_joins_non_empty_with_newlines() {
        let messages = vec![
            ImportedMessage {
                role: "user".to_string(),
                content: "  first  ".to_string(),
                timestamp: 1,
                model: None,
                turn_index: 0,
            },
            ImportedMessage {
                role: "assistant".to_string(),
                content: "   ".to_string(),
                timestamp: 2,
                model: None,
                turn_index: 0,
            },
            ImportedMessage {
                role: "user".to_string(),
                content: "second".to_string(),
                timestamp: 3,
                model: None,
                turn_index: 1,
            },
        ];
        assert_eq!(build_searchable_text(&messages), "first\nsecond");
        let long = "a".repeat(SEARCH_TEXT_MAX_CHARS + 10);
        let messages = vec![ImportedMessage {
            role: "user".to_string(),
            content: long,
            timestamp: 1,
            model: None,
            turn_index: 0,
        }];
        assert_eq!(
            build_searchable_text(&messages).encode_utf16().count(),
            SEARCH_TEXT_MAX_CHARS
        );
    }

    #[test]
    fn copies_session_file_and_writes_snapshot() {
        let root = TempDir::new("persist");
        let source_dir = root.0.join("home/.claude/projects/proj-a");
        std::fs::create_dir_all(&source_dir).expect("source dir");
        let source_path = source_dir.join("session-1.jsonl");
        std::fs::write(&source_path, "{}\n").expect("source");
        let data_root = root.0.join("mode-data");

        let (copied, created) =
            copy_session_file(&data_root, "D:/work/app", None, &source_path).expect("copy");
        assert!(created);
        assert!(copied.starts_with(
            data_root
                .join("v2")
                .join("agent-config")
                .join("claude")
        ));
        let hash = workspace_identity::workspace_hash_for("D:/work/app", None);
        let copied_text = copied.to_string_lossy().replace('\\', "/");
        assert!(
            copied_text.contains(&format!("claude/{hash}/projects/proj-a/session-1.jsonl")),
            "{copied_text}"
        );
        // 重复复制不再算新建。
        let (_copied, created_again) =
            copy_session_file(&data_root, "D:/work/app", None, &source_path).expect("copy");
        assert!(!created_again);

        let file = json!({ "meta": { "taskId": "task-1" }, "messages": [] });
        let snapshot =
            write_legacy_snapshot(&data_root, "D:/work/app", None, "task-1", &file).expect("snapshot");
        let snapshot_text = snapshot.to_string_lossy().replace('\\', "/");
        assert!(
            snapshot_text.contains(&format!("v2/sessions/{hash}/task-1.json")),
            "{snapshot_text}"
        );
        let text = std::fs::read_to_string(&snapshot).expect("read snapshot");
        assert!(text.ends_with('\n'));
        assert!(text.contains("\"taskId\": \"task-1\""));
    }
}
