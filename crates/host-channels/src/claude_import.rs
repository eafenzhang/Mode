//! Claude Code 原生历史导入 —— 扫描与 head 解析（Stage 1）。
//!
//! 真源：
//! `packages/services/src/session/claude-native/claudeNativeSessionImportRepo.ts`（扫描）、
//! `claudeNativeSessionHeadParser.ts`（head 信息）与 `sessionTitle.ts`（预览标题）。
//!
//! 扫描 `~/.claude/projects/**/*.jsonl`（另含 HOME/MODE_DATA_BASE_DIR 根），读取每个文件头
//! 16 条记录：有 sidechain 标记 / 无 cwd / worktree 临时工作区 → 跳过；
//! 输出按 mtime 降序的候选（sessionId = 文件名，updatedAt = mtime）。
//!
//! 导入（复制文件、完整解析、createSession(importedHistory)、索引回写）属 Stage 2。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::model_trajectory::parse_time_ms;

const HEAD_LINES: usize = 16;
const IGNORED_TRANSCRIPT_DIRS: [&str; 3] = ["subagents", "worktree", "worktrees"];

pub struct ScanParams {
    pub workspace_path: Option<String>,
    pub modified_since: Option<i64>,
    pub limit: Option<f64>,
}

/// 按进程环境扫描（HOME/USERPROFILE/MODE_DATA_BASE_DIR 下的 `.claude/projects`）。
pub fn scan_importable_sessions(params: &ScanParams) -> Vec<Value> {
    scan_importable_sessions_from_roots(&native_projects_roots(), params)
}

/// 指定根目录扫描（测试注入）。
pub fn scan_importable_sessions_from_roots(
    roots: &[PathBuf],
    params: &ScanParams,
) -> Vec<Value> {
    let workspace_key = params
        .workspace_path
        .as_deref()
        .map(normalize_path_for_comparison);
    let mut files: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in roots {
        collect_jsonl_files(root, &mut files);
    }
    files.retain(|file| seen.insert(file.clone()));

    let mut candidates: Vec<Value> = Vec::new();
    for file in files {
        let Ok(metadata) = std::fs::metadata(&file) else {
            continue;
        };
        let updated_at = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0);
        if params.modified_since.is_some_and(|since| updated_at < since) {
            continue;
        }
        let Ok(entries) = read_json_lines_head(&file, HEAD_LINES) else {
            continue;
        };
        if has_sidechain_marker(&entries) {
            continue;
        }
        let Some(head) = extract_head_info(&entries) else {
            continue;
        };
        let Some(workspace_path) = head.workspace_path else {
            continue;
        };
        // Claude 会把临时执行面放到 ~/.claude/worktrees：导入向导只展示真实 workspace。
        if is_claude_worktree_workspace_path(&workspace_path) {
            continue;
        }
        if workspace_key
            .as_deref()
            .is_some_and(|key| normalize_path_for_comparison(&workspace_path) != key)
        {
            continue;
        }
        let session_id = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        if session_id.is_empty() {
            continue;
        }
        let mut candidate = json!({
            "provider": "claude",
            "sessionId": session_id,
            "workspacePath": workspace_path,
            "sourcePath": file.to_string_lossy(),
            "updatedAt": updated_at,
        });
        if let Some(created_at) = head.created_at {
            candidate["createdAt"] = json!(created_at);
        }
        if let Some(preview_title) = head.preview_title {
            candidate["previewTitle"] = json!(preview_title);
        }
        candidates.push(candidate);
    }

    candidates.sort_by(|left, right| {
        right["updatedAt"]
            .as_i64()
            .unwrap_or(0)
            .cmp(&left["updatedAt"].as_i64().unwrap_or(0))
    });
    match params.limit {
        Some(limit) if limit.is_finite() && limit > 0.0 => {
            candidates.truncate(limit as usize);
        }
        _ => {}
    }
    candidates
}

fn native_projects_roots() -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = Vec::new();
    for key in ["HOME", "USERPROFILE", "MODE_DATA_BASE_DIR"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                continue;
            }
            let home = PathBuf::from(trimmed);
            if !homes.contains(&home) {
                homes.push(home);
            }
        }
    }
    homes
        .into_iter()
        .map(|home| home.join(".claude").join("projects"))
        .collect()
}

fn collect_jsonl_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if IGNORED_TRANSCRIPT_DIRS.contains(&name.as_str()) {
                continue;
            }
            collect_jsonl_files(&path, files);
            continue;
        }
        if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
        {
            files.push(path);
        }
    }
}

fn read_json_lines_head(path: &Path, max_lines: usize) -> Result<Vec<Value>, String> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|error| error.to_string())?;
        let line = line.trim();
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
        if records.len() >= max_lines {
            break;
        }
    }
    Ok(records)
}

struct HeadInfo {
    workspace_path: Option<String>,
    preview_title: Option<String>,
    created_at: Option<i64>,
}

fn extract_head_info(entries: &[Value]) -> Option<HeadInfo> {
    let mut workspace_path: Option<String> = None;
    for entry in entries {
        let user_text = extract_user_text(entry);
        let assistant_text = extract_assistant_text(entry);
        if workspace_path.is_none() && (user_text.is_some() || assistant_text.is_some()) {
            workspace_path = read_entry_workspace_path(entry);
        }
        let Some(user_text) = user_text else {
            continue;
        };
        return Some(HeadInfo {
            workspace_path,
            preview_title: Some(derive_session_title(&user_text)),
            created_at: read_entry_timestamp(entry),
        });
    }
    Some(HeadInfo {
        workspace_path,
        preview_title: None,
        created_at: None,
    })
}

fn has_sidechain_marker(entries: &[Value]) -> bool {
    entries.iter().any(|entry| {
        let message = entry.get("message").and_then(Value::as_object);
        let request = entry.get("request").and_then(Value::as_object);
        entry.get("isSidechain").and_then(Value::as_bool) == Some(true)
            || message
                .and_then(|message| message.get("isSidechain"))
                .and_then(Value::as_bool)
                == Some(true)
            || request
                .and_then(|request| request.get("isSidechain"))
                .and_then(Value::as_bool)
                == Some(true)
    })
}

fn read_entry_workspace_path(entry: &Value) -> Option<String> {
    let message = entry.get("message").and_then(Value::as_object);
    let request = entry.get("request").and_then(Value::as_object);
    read_trimmed_string(entry.get("cwd"))
        .or_else(|| message.and_then(|message| read_trimmed_string(message.get("cwd"))))
        .or_else(|| request.and_then(|request| read_trimmed_string(request.get("cwd"))))
        .map(str::to_string)
}

fn read_entry_model(entry: &Value) -> Option<String> {
    let message = entry.get("message").and_then(Value::as_object);
    read_trimmed_string(entry.get("model"))
        .or_else(|| message.and_then(|message| read_trimmed_string(message.get("model"))))
        .map(str::to_string)
}

fn read_entry_timestamp(entry: &Value) -> Option<i64> {
    let message = entry.get("message").and_then(Value::as_object);
    let request = entry.get("request").and_then(Value::as_object);
    let candidates = [
        entry.get("timestamp"),
        entry.get("createdAt"),
        entry.get("updatedAt"),
        entry.get("created_at"),
        entry.get("updated_at"),
        entry.get("time"),
        message.and_then(|message| message.get("timestamp")),
        message.and_then(|message| message.get("createdAt")),
        message.and_then(|message| message.get("updatedAt")),
        request.and_then(|request| request.get("timestamp")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find_map(to_timestamp_ms)
}

/// Claude 时间戳：毫秒数 / 秒数 / 数字字符串 / ISO 字符串（Date.parse 等价）。
fn to_timestamp_ms(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_f64() {
        if !number.is_finite() {
            return None;
        }
        if number > 1_000_000_000_000.0 {
            return Some(number.trunc() as i64);
        }
        if number > 1_000_000_000.0 {
            return Some((number * 1000.0).trunc() as i64);
        }
        return None;
    }
    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(number) = text.parse::<f64>() {
        if number.is_finite() {
            return to_timestamp_ms(&json!(number));
        }
    }
    let parsed = parse_time_ms(text);
    (parsed != 0).then_some(parsed)
}

fn extract_user_text(entry: &Value) -> Option<String> {
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

fn extract_assistant_text(entry: &Value) -> Option<String> {
    if entry.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = entry.get("message").and_then(Value::as_object);
    let is_api_error = entry.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true)
        || message
            .and_then(|message| message.get("isApiErrorMessage"))
            .and_then(Value::as_bool)
            == Some(true);
    if is_api_error || read_entry_model(entry).as_deref() == Some("<synthetic>") {
        return None;
    }
    let content = message.and_then(|message| message.get("content"))?;
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
        if object.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let normalized = sanitize_claude_visible_text(&extract_text_field(item));
        if !normalized.is_empty() {
            visible.push(normalized);
        }
    }
    let text = visible.concat();
    (!text.is_empty()).then_some(text)
}

fn extract_text_field(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    let Some(object) = value.as_object() else {
        return String::new();
    };
    read_trimmed_string(object.get("text"))
        .or_else(|| read_trimmed_string(object.get("content")))
        .unwrap_or("")
        .to_string()
}

fn sanitize_claude_visible_text(text: &str) -> String {
    let text = remove_tag_blocks(text, "<ide_opened_file", "</ide_opened_file>");
    let text = remove_tag_blocks(&text, "<local-command-", "</local-command");
    let text = remove_tag_blocks(&text, "<command-", "</command>");
    text.replace("\r\n", "\n").trim().to_string()
}

/// 大小写不敏感的标签块删除（JS `/<open...>[\s\S]*?<\/close>/gi` 等价）。
fn remove_tag_blocks(text: &str, open: &str, close: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let open_lower = open.to_ascii_lowercase();
    let close_lower = close.to_ascii_lowercase();
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find(open_lower.as_str()) {
        let start = cursor + offset;
        output.push_str(&text[cursor..start]);
        let search_from = start + open.len();
        match lower[search_from..].find(close_lower.as_str()) {
            Some(relative) => {
                output.push(' ');
                cursor = search_from + relative + close.len();
            }
            None => {
                // 没有闭合标签：保留原文（避免吞掉后续全部内容）。
                cursor = start;
                break;
            }
        }
    }
    output.push_str(&text[cursor..]);
    output
}

/// JS `deriveSessionTitle(content, [])`：UTF-16 长度 50 截断 + "..."。
fn derive_session_title(content: &str) -> String {
    let units: Vec<u16> = content.encode_utf16().collect();
    if units.len() <= 50 {
        return content.to_string();
    }
    let truncated = String::from_utf16_lossy(&units[..50]);
    format!("{truncated}...")
}

fn read_trimmed_string(value: Option<&Value>) -> Option<&str> {
    let text = value?.as_str()?.trim();
    (!text.is_empty()).then_some(text)
}

fn normalize_path_for_comparison(path: &str) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path));
    let text = absolute.to_string_lossy().replace('\\', "/");
    // TS `resolve()` 会去掉尾部分隔符；比较键保持一致。
    let text = text.trim_end_matches('/').to_string();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

fn is_claude_worktree_workspace_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let segments: Vec<String> = normalized
        .split('/')
        .map(|segment| segment.to_lowercase())
        .collect();
    segments.iter().enumerate().any(|(index, segment)| {
        segment == ".claude"
            && matches!(
                segments.get(index + 1).map(String::as_str),
                Some("worktree") | Some("worktrees")
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "claude-import-{label}-{}-{}",
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

    fn write_jsonl(dir: &Path, project: &str, session_id: &str, lines: &[Value]) -> PathBuf {
        let project_dir = dir.join(".claude").join("projects").join(project);
        std::fs::create_dir_all(&project_dir).expect("project dir");
        let path = project_dir.join(format!("{session_id}.jsonl"));
        let text = lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, format!("{text}\n")).expect("write");
        path
    }

    fn user_line(cwd: &str, text: &str, timestamp: &str) -> Value {
        json!({
            "type": "user",
            "cwd": cwd,
            "timestamp": timestamp,
            "message": { "role": "user", "content": text }
        })
    }

    fn assistant_line(text: &str) -> Value {
        json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{ "type": "text", "text": text }]
            }
        })
    }

    fn scan(root: &Path, params: ScanParams) -> Vec<Value> {
        let root = root.to_path_buf();
        scan_importable_sessions_from_roots(&[root], &params)
    }

    fn default_params() -> ScanParams {
        ScanParams {
            workspace_path: None,
            modified_since: None,
            limit: None,
        }
    }

    #[test]
    fn scans_visible_sessions_with_preview_title() {
        let dir = TempDir::new("scan");
        write_jsonl(
            &dir.0,
            "proj-a",
            "session-a",
            &[
                user_line(
                    "D:/work/app",
                    "<ide_opened_file>x</ide_opened_file>Fix the bug please",
                    "2026-01-01T00:00:00Z",
                ),
                assistant_line("Looking into it"),
            ],
        );
        let candidates = scan(&dir.0, default_params());
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        let candidate = &candidates[0];
        assert_eq!(candidate["provider"], "claude");
        assert_eq!(candidate["sessionId"], "session-a");
        assert_eq!(candidate["workspacePath"], "D:/work/app");
        assert_eq!(candidate["previewTitle"], "Fix the bug please");
        assert!(candidate["createdAt"].as_i64().expect("createdAt") > 0);
        assert!(candidate["sourcePath"]
            .as_str()
            .expect("sourcePath")
            .ends_with("session-a.jsonl"));
    }

    #[test]
    fn skips_sidechain_worktree_and_workspace_mismatch() {
        let dir = TempDir::new("skip");
        write_jsonl(
            &dir.0,
            "proj-a",
            "sidechain",
            &[
                json!({ "type": "user", "cwd": "D:/work/app", "isSidechain": true,
                        "message": { "content": "hidden" } }),
            ],
        );
        write_jsonl(
            &dir.0,
            "proj-b",
            "worktree",
            &[user_line("D:/work/.claude/worktrees/x", "temp", "2026-01-01T00:00:00Z")],
        );
        write_jsonl(
            &dir.0,
            "proj-c",
            "other-workspace",
            &[user_line("D:/work/other", "other", "2026-01-01T00:00:00Z")],
        );
        write_jsonl(
            &dir.0,
            "proj-d",
            "match",
            &[user_line("D:/work/app", "match", "2026-01-01T00:00:00Z")],
        );

        assert_eq!(
            normalize_path_for_comparison("d:/work/app/"),
            normalize_path_for_comparison("D:/work/app"),
            "normalize mismatch"
        );
        let candidates = scan(
            &dir.0,
            ScanParams {
                workspace_path: Some("d:/work/app/".to_string()),
                ..default_params()
            },
        );
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(candidates[0]["sessionId"], "match");
    }

    #[test]
    fn modified_since_and_limit_filter() {
        let dir = TempDir::new("filter");
        for session in ["s1", "s2", "s3"] {
            write_jsonl(
                &dir.0,
                "proj",
                session,
                &[user_line("D:/work/app", "hello", "2026-01-01T00:00:00Z")],
            );
        }
        assert_eq!(scan(&dir.0, default_params()).len(), 3);
        let limited = scan(
            &dir.0,
            ScanParams {
                limit: Some(1.0),
                ..default_params()
            },
        );
        assert_eq!(limited.len(), 1, "{limited:?}");
        // 未来时间 → 全部过滤。
        let filtered = scan(
            &dir.0,
            ScanParams {
                modified_since: Some(i64::MAX / 2),
                ..default_params()
            },
        );
        assert!(filtered.is_empty(), "{filtered:?}");
    }

    #[test]
    fn head_parser_handles_string_and_array_content() {
        let entries = vec![
            json!({ "type": "assistant", "message": { "content": [
                { "type": "text", "text": "prelude" }
            ] } }),
            json!({ "type": "user", "cwd": "D:/work/app", "message": { "content": [
                { "type": "text", "text": "first line" },
                { "type": "tool_result", "content": "ignored" },
                { "type": "text", "text": "second line" }
            ] } }),
        ];
        let head = extract_head_info(&entries).expect("head");
        assert_eq!(head.workspace_path.as_deref(), Some("D:/work/app"));
        assert_eq!(head.preview_title.as_deref(), Some("first line\n\nsecond line"));
        // assistant 的非 text part 不参与预览。
        assert!(!extract_assistant_text(&entries[0]).unwrap().contains("tool_result"));
    }

    #[test]
    fn title_truncates_by_utf16_units() {
        let long = "a".repeat(60);
        assert_eq!(derive_session_title(&long), format!("{}...", "a".repeat(50)));
        assert_eq!(derive_session_title("short"), "short");
    }
}
