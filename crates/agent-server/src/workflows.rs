//! Workflows 会话面（C4 T3）：`.mode/workflows` 文件读写与 `dwf_run` 读面。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §4 与 `fixtures/c4/workflows-*.json`。
//!
//! 文件格式（逐字节观察）：
//!
//! ```text
//! /* mode-workflow
//! description: ...
//! whenToUse: ...
//! args:
//!   target:
//!     type: string
//!     description: ...
//!     required: false
//!     default: x
//! */
//! <script body>
//! ```
//!
//! 目录：project = `<workspace>/.mode/workflows`；global = `<dataRoot>/workflows`。
//! 只认 `.dwf.ts`；其它扩展名完全忽略。执行（`v4/command startSavedWorkflow`）不在本模块：
//! Rust 侧没有 JS 引擎，脚本执行仍由既有 CLI 运行时承担（契约 §4.6 登记）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_protocol::workflow::{
    SavedWorkflowArgDeclaration, SavedWorkflowArgType, SavedWorkflowArgs, SavedWorkflowEntry,
    SavedWorkflowFailure, SavedWorkflowFailureReason, SavedWorkflowInvalidEntry, SavedWorkflowMeta,
    SavedWorkflowRun, SavedWorkflowScope, WorkflowGetOk, WorkflowOkPath, WorkflowRunStatus,
    WorkflowsDeleteResult, WorkflowsGetResult, WorkflowsListResult, WorkflowsMoveFailure,
    WorkflowsMoveFailureReason, WorkflowsMoveResult, WorkflowsRunsResult,
    WorkflowsUpdateMetaResult,
};
use serde_json::{Number, Value};

pub const WORKFLOW_FILE_SUFFIX: &str = ".dwf.ts";
const FRONTMATTER_START: &str = "/* mode-workflow";
const FRONTMATTER_END: &str = "*/";
const MISSING_FRONTMATTER: &str = "file does not start with the `/* mode-workflow` metadata block";

/// 工程/全局两个 workflow 目录。
#[derive(Debug, Clone)]
pub struct WorkflowDirs {
    pub project: PathBuf,
    pub global: PathBuf,
}

impl WorkflowDirs {
    pub fn new(workspace_path: &str, data_root: &Path) -> Self {
        Self {
            project: PathBuf::from(workspace_path)
                .join(".mode")
                .join("workflows"),
            global: data_root.join("workflows"),
        }
    }

    fn dir_for(&self, scope: SavedWorkflowScope) -> &Path {
        match scope {
            SavedWorkflowScope::Project => &self.project,
            SavedWorkflowScope::Global => &self.global,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedWorkflow {
    pub meta: SavedWorkflowMeta,
    pub script: String,
}

/// 解析块注释元数据 + 脚本正文。错误文案按夹具。
pub fn parse_workflow(text: &str) -> Result<ParsedWorkflow, SavedWorkflowFailure> {
    let trimmed = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = trimmed.strip_prefix(FRONTMATTER_START) else {
        return Err(failure(
            SavedWorkflowFailureReason::ParseError,
            MISSING_FRONTMATTER,
        ));
    };
    let Some(end) = rest.find(FRONTMATTER_END) else {
        return Err(failure(
            SavedWorkflowFailureReason::ParseError,
            MISSING_FRONTMATTER,
        ));
    };
    let header = &rest[..end];
    let script = rest[end + FRONTMATTER_END.len()..].trim_start_matches(['\r', '\n']);
    let mut description: Option<String> = None;
    let mut when_to_use: Option<String> = None;
    let mut args: Option<SavedWorkflowArgs> = None;
    let lines: Vec<&str> = header.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() {
            index += 1;
            continue;
        }
        if let Some(value) = trimmed_line.strip_prefix("description:") {
            description = Some(value.trim().to_string());
            index += 1;
            continue;
        }
        if let Some(value) = trimmed_line.strip_prefix("whenToUse:") {
            let value = value.trim();
            if !value.is_empty() {
                when_to_use = Some(value.to_string());
            }
            index += 1;
            continue;
        }
        if trimmed_line == "args:" {
            let (parsed, consumed) = parse_args(&lines[index + 1..]);
            args = parsed;
            index += 1 + consumed;
            continue;
        }
        index += 1;
    }
    let Some(description) = description else {
        return Err(failure(
            SavedWorkflowFailureReason::ParseError,
            "metadata block is missing `description`",
        ));
    };
    Ok(ParsedWorkflow {
        meta: SavedWorkflowMeta {
            description,
            when_to_use,
            args,
        },
        script: script.to_string(),
    })
}

/// 读 `args:` 之后的缩进块；返回（解析结果, 消费行数）。
fn parse_args(lines: &[&str]) -> (Option<SavedWorkflowArgs>, usize) {
    let mut args: SavedWorkflowArgs = BTreeMap::new();
    let mut consumed = 0;
    let mut current: Option<(String, SavedWorkflowArgDeclaration)> = None;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            consumed += 1;
            continue;
        }
        // args 块结束：回到顶格的非空行。
        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        consumed += 1;
        let indent = line.len() - line.trim_start().len();
        if indent <= 2 {
            // 新参数：`  name:`
            if let Some((name, declaration)) = current.take() {
                args.insert(name, declaration);
            }
            if let Some(name) = trimmed.strip_suffix(':') {
                current = Some((
                    name.trim().to_string(),
                    SavedWorkflowArgDeclaration {
                        arg_type: SavedWorkflowArgType::String,
                        description: None,
                        required: None,
                        default: None,
                    },
                ));
            }
            continue;
        }
        let Some((_, declaration)) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "type" => {
                declaration.arg_type = match value {
                    "number" => SavedWorkflowArgType::Number,
                    "boolean" => SavedWorkflowArgType::Boolean,
                    "json" => SavedWorkflowArgType::Json,
                    _ => SavedWorkflowArgType::String,
                }
            }
            "description" => {
                if !value.is_empty() {
                    declaration.description = Some(unquote(value));
                }
            }
            "required" => declaration.required = Some(value == "true"),
            "default" => {
                declaration.default = Some(Some(parse_scalar(value)));
            }
            _ => {}
        }
    }
    if let Some((name, declaration)) = current {
        args.insert(name, declaration);
    }
    (if args.is_empty() { None } else { Some(args) }, consumed)
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(trimmed)
        .to_string()
}

fn parse_scalar(value: &str) -> Value {
    match value {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ => match value.parse::<i64>() {
            Ok(number) => Value::Number(number.into()),
            Err(_) => match value.parse::<f64>() {
                Ok(number) => Number::from_f64(number)
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::String(unquote(value))),
                Err(_) => Value::String(unquote(value)),
            },
        },
    }
}

/// 生成文件正文：块注释元数据 + 脚本。
pub fn render_workflow(meta: &SavedWorkflowMeta, script: &str) -> String {
    let mut header = String::from("/* mode-workflow\n");
    header.push_str(&format!("description: {}\n", meta.description));
    if let Some(when_to_use) = &meta.when_to_use {
        header.push_str(&format!("whenToUse: {when_to_use}\n"));
    }
    if let Some(args) = &meta.args {
        header.push_str("args:\n");
        for (name, declaration) in args {
            header.push_str(&format!("  {name}:\n"));
            header.push_str(&format!(
                "    type: {}\n",
                arg_type_str(declaration.arg_type)
            ));
            if let Some(description) = &declaration.description {
                header.push_str(&format!("    description: {description}\n"));
            }
            if let Some(required) = declaration.required {
                header.push_str(&format!("    required: {required}\n"));
            }
            if let Some(Some(value)) = &declaration.default {
                header.push_str(&format!("    default: {}\n", scalar_str(value)));
            }
        }
    }
    header.push_str("*/\n");
    format!("{header}{script}")
}

fn arg_type_str(arg_type: SavedWorkflowArgType) -> &'static str {
    match arg_type {
        SavedWorkflowArgType::String => "string",
        SavedWorkflowArgType::Number => "number",
        SavedWorkflowArgType::Boolean => "boolean",
        SavedWorkflowArgType::Json => "json",
    }
}

fn scalar_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn failure(reason: SavedWorkflowFailureReason, detail: &str) -> SavedWorkflowFailure {
    SavedWorkflowFailure {
        ok: agent_protocol::workflow::LiteralFalse,
        reason,
        detail: Some(detail.to_string()),
    }
}

fn invalid_name() -> SavedWorkflowFailure {
    failure(
        SavedWorkflowFailureReason::InvalidName,
        "workflow names may only contain letters, digits, '.', '-' and '_', and must be 1-64 characters",
    )
}

/// 名字校验：`[A-Za-z0-9._-]{1,64}`。
pub fn is_valid_workflow_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
}

/// `workflows/list`。
pub fn list(dirs: &WorkflowDirs, scope: Option<SavedWorkflowScope>) -> WorkflowsListResult {
    let scopes: Vec<SavedWorkflowScope> = match scope {
        Some(scope) => vec![scope],
        None => vec![SavedWorkflowScope::Project, SavedWorkflowScope::Global],
    };
    let mut workflows = Vec::new();
    let mut invalid = Vec::new();
    for scope in &scopes {
        let dir = dirs.dir_for(*scope);
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if !file_name.ends_with(WORKFLOW_FILE_SUFFIX) {
                continue;
            }
            let name = file_name.trim_end_matches(WORKFLOW_FILE_SUFFIX).to_string();
            let display = path.display().to_string();
            match std::fs::read_to_string(&path) {
                Ok(text) => match parse_workflow(&text) {
                    Ok(parsed) => workflows.push(SavedWorkflowEntry {
                        name,
                        description: parsed.meta.description,
                        when_to_use: parsed.meta.when_to_use,
                        args: parsed.meta.args,
                        scope: *scope,
                        path: display,
                    }),
                    Err(error) => invalid.push(SavedWorkflowInvalidEntry {
                        path: display,
                        reason: invalid_reason(&error),
                    }),
                },
                Err(error) => invalid.push(SavedWorkflowInvalidEntry {
                    path: display,
                    reason: format!("read_error: {error}"),
                }),
            }
        }
    }
    workflows.sort_by(|left, right| plugin_discovery::js::locale_compare(&left.name, &right.name));
    let dir = scopes
        .first()
        .map(|scope| dirs.dir_for(*scope).display().to_string())
        .unwrap_or_else(|| dirs.project.display().to_string());
    WorkflowsListResult {
        workflows,
        invalid,
        dir,
    }
}

fn invalid_reason(error: &SavedWorkflowFailure) -> String {
    match error.reason {
        // 观察：缺块与「看起来像 frontmatter 但不是块注释」都归 missing_frontmatter。
        SavedWorkflowFailureReason::ParseError
            if error.detail.as_deref() == Some(MISSING_FRONTMATTER) =>
        {
            format!("missing_frontmatter: {MISSING_FRONTMATTER}")
        }
        other => format!("{}: {}", other, error.detail.as_deref().unwrap_or_default()),
    }
}

/// 定位文件：显式 scope 优先；缺省 project → global。
fn locate(
    dirs: &WorkflowDirs,
    name: &str,
    scope: Option<SavedWorkflowScope>,
) -> Option<(PathBuf, SavedWorkflowScope)> {
    let scopes = match scope {
        Some(scope) => vec![scope],
        None => vec![SavedWorkflowScope::Project, SavedWorkflowScope::Global],
    };
    for scope in scopes {
        let path = dirs
            .dir_for(scope)
            .join(format!("{name}{WORKFLOW_FILE_SUFFIX}"));
        if path.is_file() {
            return Some((path, scope));
        }
    }
    None
}

/// `workflows/get`。
pub fn get(
    dirs: &WorkflowDirs,
    name: &str,
    scope: Option<SavedWorkflowScope>,
) -> WorkflowsGetResult {
    if !is_valid_workflow_name(name) {
        return WorkflowsGetResult::Failure(invalid_name());
    }
    let Some((path, scope)) = locate(dirs, name, scope) else {
        return WorkflowsGetResult::Failure(failure(
            SavedWorkflowFailureReason::NotFound,
            "workflow not found",
        ));
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return WorkflowsGetResult::Failure(failure(
                SavedWorkflowFailureReason::ReadError,
                &error.to_string(),
            ))
        }
    };
    match parse_workflow(&text) {
        Ok(parsed) => WorkflowsGetResult::Ok(WorkflowGetOk {
            ok: agent_protocol::workflow::LiteralTrue,
            name: name.to_string(),
            path: path.display().to_string(),
            scope,
            meta: parsed.meta,
            script: parsed.script,
        }),
        Err(error) => WorkflowsGetResult::Failure(error),
    }
}

/// `workflows/updateMeta`。
pub fn update_meta(
    dirs: &WorkflowDirs,
    name: &str,
    meta: &SavedWorkflowMeta,
    scope: Option<SavedWorkflowScope>,
) -> WorkflowsUpdateMetaResult {
    if !is_valid_workflow_name(name) {
        return WorkflowsUpdateMetaResult::Failure(invalid_name());
    }
    let Some((path, _)) = locate(dirs, name, scope) else {
        return WorkflowsUpdateMetaResult::Failure(failure(
            SavedWorkflowFailureReason::NotFound,
            "workflow not found",
        ));
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return WorkflowsUpdateMetaResult::Failure(failure(
                SavedWorkflowFailureReason::ReadError,
                &error.to_string(),
            ))
        }
    };
    let parsed = match parse_workflow(&text) {
        Ok(parsed) => parsed,
        Err(error) => return WorkflowsUpdateMetaResult::Failure(error),
    };
    let rendered = render_workflow(meta, &parsed.script);
    if let Err(error) = std::fs::write(&path, rendered) {
        return WorkflowsUpdateMetaResult::Failure(failure(
            SavedWorkflowFailureReason::ReadError,
            &error.to_string(),
        ));
    }
    WorkflowsUpdateMetaResult::Ok(WorkflowOkPath {
        ok: agent_protocol::workflow::LiteralTrue,
        path: path.display().to_string(),
    })
}

/// `workflows/delete`。
pub fn delete(
    dirs: &WorkflowDirs,
    name: &str,
    scope: Option<SavedWorkflowScope>,
) -> WorkflowsDeleteResult {
    if !is_valid_workflow_name(name) {
        return WorkflowsDeleteResult::Failure(invalid_name());
    }
    let Some((path, _)) = locate(dirs, name, scope) else {
        return WorkflowsDeleteResult::Failure(failure(
            SavedWorkflowFailureReason::NotFound,
            "workflow not found",
        ));
    };
    match std::fs::remove_file(&path) {
        Ok(()) => WorkflowsDeleteResult::Ok(WorkflowOkPath {
            ok: agent_protocol::workflow::LiteralTrue,
            path: path.display().to_string(),
        }),
        Err(error) => WorkflowsDeleteResult::Failure(failure(
            SavedWorkflowFailureReason::ReadError,
            &error.to_string(),
        )),
    }
}

/// `workflows/move`：global ↔ project 同名文件。
pub fn move_workflow(dirs: &WorkflowDirs, name: &str) -> WorkflowsMoveResult {
    if !is_valid_workflow_name(name) {
        return WorkflowsMoveResult::Failure(WorkflowsMoveFailure {
            ok: agent_protocol::workflow::LiteralFalse,
            reason: WorkflowsMoveFailureReason::InvalidName,
            path: None,
            detail: None,
        });
    }
    let global = dirs.global.join(format!("{name}{WORKFLOW_FILE_SUFFIX}"));
    let project = dirs.project.join(format!("{name}{WORKFLOW_FILE_SUFFIX}"));
    let (from, to) = if global.is_file() && !project.is_file() {
        (global, project)
    } else if project.is_file() && !global.is_file() {
        (project, global)
    } else if global.is_file() && project.is_file() {
        return WorkflowsMoveResult::Failure(WorkflowsMoveFailure {
            ok: agent_protocol::workflow::LiteralFalse,
            reason: WorkflowsMoveFailureReason::TargetExists,
            path: Some(project.display().to_string()),
            detail: None,
        });
    } else {
        return WorkflowsMoveResult::Failure(WorkflowsMoveFailure {
            ok: agent_protocol::workflow::LiteralFalse,
            reason: WorkflowsMoveFailureReason::NotFound,
            path: None,
            detail: None,
        });
    };
    if let Some(parent) = to.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return WorkflowsMoveResult::Failure(WorkflowsMoveFailure {
                ok: agent_protocol::workflow::LiteralFalse,
                reason: WorkflowsMoveFailureReason::WriteError,
                path: None,
                detail: Some(error.to_string()),
            });
        }
    }
    match std::fs::rename(&from, &to) {
        Ok(()) => WorkflowsMoveResult::Ok(agent_protocol::workflow::WorkflowsMoveOk {
            ok: agent_protocol::workflow::LiteralTrue,
            from: from.display().to_string(),
            to: to.display().to_string(),
        }),
        Err(error) => WorkflowsMoveResult::Failure(WorkflowsMoveFailure {
            ok: agent_protocol::workflow::LiteralFalse,
            reason: WorkflowsMoveFailureReason::WriteError,
            path: Some(from.display().to_string()),
            detail: Some(error.to_string()),
        }),
    }
}

/// `workflows/runs`：读 `dwf_run`（唯一事实源是 session-store 的库文件，只读连接）。
pub fn runs(
    data_root: &Path,
    workspace_path: &str,
    name: Option<&str>,
    limit: u64,
) -> WorkflowsRunsResult {
    let db_path = data_root.join("cli").join("db").join("db.sqlite");
    let Ok(connection) =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return WorkflowsRunsResult {
            runs: Vec::new(),
            truncated: None,
        };
    };
    let normalized_cwd = workspace_path.replace('\\', "/");
    let mut statement = match connection.prepare(
        "select id, name, status, time_created, time_updated, spent_tokens, \
         parent_session_id, tool_call_id, args_json, cwd \
         from dwf_run where cwd = ?1 order by time_created desc",
    ) {
        Ok(statement) => statement,
        Err(_) => {
            return WorkflowsRunsResult {
                runs: Vec::new(),
                truncated: None,
            };
        }
    };
    let rows = statement
        .query_map([workspace_path.to_string()], |row| {
            Ok(WorkflowRunRow {
                id: row.get(0)?,
                name: row.get(1)?,
                status: row.get(2)?,
                time_created: row.get(3)?,
                time_updated: row.get(4)?,
                spent_tokens: row.get(5)?,
                parent_session_id: row.get(6)?,
                tool_call_id: row.get(7)?,
                args_json: row.get(8)?,
                cwd: row.get(9)?,
            })
        })
        .ok();
    let mut runs = Vec::new();
    if let Some(rows) = rows {
        for row in rows.flatten() {
            if let Some(name) = name {
                if row.name.as_deref() != Some(name) {
                    continue;
                }
            }
            if let Some(cwd) = &row.cwd {
                if cwd.replace('\\', "/") != normalized_cwd {
                    continue;
                }
            }
            runs.push(row.into_protocol());
        }
    }
    let truncated = runs.len() > limit as usize;
    runs.truncate(limit as usize);
    WorkflowsRunsResult {
        runs,
        truncated: truncated.then_some(agent_protocol::workflow::LiteralTrue),
    }
}

struct WorkflowRunRow {
    id: String,
    name: Option<String>,
    status: String,
    time_created: i64,
    time_updated: i64,
    spent_tokens: i64,
    parent_session_id: Option<String>,
    tool_call_id: Option<String>,
    args_json: Option<String>,
    cwd: Option<String>,
}

impl WorkflowRunRow {
    fn into_protocol(self) -> SavedWorkflowRun {
        let status = match self.status.as_str() {
            "pending" => WorkflowRunStatus::Pending,
            "running" => WorkflowRunStatus::Running,
            "errored" => WorkflowRunStatus::Errored,
            "stopped" => WorkflowRunStatus::Stopped,
            _ => WorkflowRunStatus::Completed,
        };
        SavedWorkflowRun {
            run_id: self.id,
            name: self.name,
            status,
            stop_reason: None,
            created_at: Number::from(self.time_created),
            updated_at: Number::from(self.time_updated),
            spent_tokens: Number::from(self.spent_tokens),
            parent_session_id: self.parent_session_id,
            tool_call_id: self.tool_call_id,
            args: self
                .args_json
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .and_then(|value| value.as_object().cloned()),
            cwd: self.cwd,
            artifacts: None,
        }
    }
}
