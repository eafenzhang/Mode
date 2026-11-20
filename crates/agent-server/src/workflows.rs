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
            "failed" => WorkflowRunStatus::Errored,
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

// ── 脚本执行（C4-T3 裁决：嵌入 JS 引擎）────────────────────────────────────

/// 脚本在嵌入引擎里的执行结果。
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptOutcome {
    /// 脚本正常返回；值已序列化为 JSON 文本。
    Completed(String),
    /// 脚本抛错/引擎失败；`failure_json` 是 `{ "message": … }`。
    Errored(String),
}

/// 执行一段 workflow 脚本：包成函数体（脚本可顶层 `return`），结果 JSON 序列化。
///
/// 同步执行——workflow 脚本应当是短任务；长任务/异步 facade 属后续切片。
pub fn execute_script(script: &str) -> ScriptOutcome {
    use boa_engine::{Context, Source};
    let wrapped = format!("(function(){{\n{script}\n}})()");
    let mut context = Context::default();
    match context.eval(Source::from_bytes(wrapped.as_bytes())) {
        Ok(value) => match value.to_json(&mut context) {
            Ok(json) => ScriptOutcome::Completed(json.to_string()),
            Err(error) => ScriptOutcome::Errored(error.to_string()),
        },
        Err(error) => ScriptOutcome::Errored(error.to_string()),
    }
}

fn script_hash(script: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(script.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// `startSavedWorkflow` 的拒绝形状（ACK `status:"failed"`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRejection {
    pub reason_code: String,
    pub message: String,
}

impl StartRejection {
    pub fn not_found(scope: SavedWorkflowScope, name: &str) -> Self {
        let scope_word = match scope {
            SavedWorkflowScope::Project => "project",
            SavedWorkflowScope::Global => "global",
        };
        Self {
            reason_code: "fault.command.savedWorkflowStartRejected.not_found".to_string(),
            message: format!("No {scope_word} workflow named '{name}'."),
        }
    }

    pub fn invalid_name(name: &str) -> Self {
        Self {
            reason_code: "fault.command.savedWorkflowStartRejected.invalid_name".to_string(),
            message: format!("Invalid workflow name: {name}"),
        }
    }
}

/// 一次成功的启动：ACK `result` 与 `dwf_*` 行都以这些 id 互指。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedWorkflow {
    pub run_id: String,
    pub tool_call_id: String,
    pub input_id: String,
}

/// 启动并同步执行一个 saved workflow：落 `dwf_run` 行 → run-started/launched 事件 →
/// 执行脚本 → run-settled + 结算行。`scope` 缺省按 project → global 定位。
#[allow(clippy::too_many_arguments)]
pub fn start_saved_workflow(
    store: &session_store::SessionStore,
    dirs: &WorkflowDirs,
    workspace_path: &str,
    parent_session_id: &str,
    name: &str,
    scope: Option<SavedWorkflowScope>,
) -> Result<StartedWorkflow, StartRejection> {
    if !is_valid_workflow_name(name) {
        return Err(StartRejection::invalid_name(name));
    }
    let resolved_scope = scope.unwrap_or(SavedWorkflowScope::Project);
    let (path, script) = match get(dirs, name, Some(resolved_scope)) {
        WorkflowsGetResult::Ok(ok) => (ok.path, ok.script),
        WorkflowsGetResult::Failure(_) => {
            // project 缺失时按 global 再找一次，拒绝文案跟随最终查找的 scope。
            let fallback_scope = if resolved_scope == SavedWorkflowScope::Project {
                SavedWorkflowScope::Global
            } else {
                SavedWorkflowScope::Project
            };
            match get(dirs, name, Some(fallback_scope)) {
                WorkflowsGetResult::Ok(ok) => (ok.path, ok.script),
                WorkflowsGetResult::Failure(_) => {
                    return Err(StartRejection::not_found(resolved_scope, name))
                }
            }
        }
    };
    let _ = path;
    let run_id = format!("dwfrun-{}", uuid::Uuid::new_v4());
    let tool_call_id = uuid::Uuid::new_v4().to_string();
    let input_id = uuid::Uuid::new_v4().to_string();
    let launch_tool_call_id = format!("launch-{}", uuid::Uuid::new_v4());
    let now = now_ms();

    store
        .insert_workflow_run(&session_store::WorkflowRunInsert {
            id: run_id.clone(),
            parent_session_id: Some(parent_session_id.to_string()),
            cwd: Some(workspace_path.to_string()),
            name: Some(name.to_string()),
            script_text: Some(script.clone()),
            script_hash: Some(script_hash(&script)),
            args_json: None,
            tool_call_id: Some(tool_call_id.clone()),
            resumed_from: None,
            caps_max_concurrency: Some(10),
            spent_tokens: Some(0),
            status: Some("running".to_string()),
            result_json: None,
            failure_json: None,
            time_created: Some(now),
            time_updated: Some(now),
        })
        .map_err(|error| StartRejection {
            reason_code: "fault.command.savedWorkflowStartRejected.store_failed".to_string(),
            message: error.to_string(),
        })?;

    let started_payload = serde_json::json!({
        "type": "run-started",
        "runId": run_id,
        "caps": { "maxConcurrency": 10 },
    });
    let launched_payload = serde_json::json!({
        "type": "run-launched",
        "inputId": input_id,
        "toolCallId": launch_tool_call_id,
        "parentSessionId": parent_session_id,
        "scriptPath": path,
    });
    for (sequence, (kind, payload)) in [
        ("run-started", started_payload),
        ("run-launched", launched_payload),
    ]
    .into_iter()
    .enumerate()
    {
        store
            .insert_workflow_event(&session_store::WorkflowEventInsert {
                run_id: run_id.clone(),
                sequence: sequence as i64,
                kind: kind.to_string(),
                payload_json: payload.to_string(),
                time_created: now,
            })
            .map_err(|error| StartRejection {
                reason_code: "fault.command.savedWorkflowStartRejected.store_failed".to_string(),
                message: error.to_string(),
            })?;
    }

    // DB 的 CHECK 只认 pending/running/completed/failed/cancelled；协议的
    // errored 在库里落 `failed`（runs 投影映射回 Errored）。
    let (status, result_json, failure_json) = match execute_script(&script) {
        ScriptOutcome::Completed(json) => ("completed", Some(json), None),
        ScriptOutcome::Errored(message) => (
            "failed",
            None,
            Some(serde_json::json!({ "message": message }).to_string()),
        ),
    };
    store
        .settle_workflow_run(
            &run_id,
            status,
            result_json.as_deref(),
            failure_json.as_deref(),
            0,
            now,
        )
        .map_err(|error| StartRejection {
            reason_code: "fault.command.savedWorkflowStartRejected.store_failed".to_string(),
            message: error.to_string(),
        })?;
    let settled_payload = serde_json::json!({ "type": "run-settled", "status": status });
    store
        .insert_workflow_event(&session_store::WorkflowEventInsert {
            run_id: run_id.clone(),
            sequence: 2,
            kind: "run-settled".to_string(),
            payload_json: settled_payload.to_string(),
            time_created: now,
        })
        .map_err(|error| StartRejection {
            reason_code: "fault.command.savedWorkflowStartRejected.store_failed".to_string(),
            message: error.to_string(),
        })?;

    Ok(StartedWorkflow {
        run_id,
        tool_call_id,
        input_id,
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
