//! 后台任务注册表与 `TaskOutput` / `TaskStop` 工具面（M2 Task 4b）。
//!
//! 规格：`docs/specs/tauri-agent-runtime.md` §3.4 的 `run_in_background` /
//! `TaskOutput` / `TaskStop` 小节（全部来自黑盒观察，探针
//! `probe-bash-background.mts` 与 `probe-task-control.mts`）。
//!
//! # 已实现的范围
//!
//! - 后台任务的登记、状态推进（`running` → `completed` / `failed` / `cancelled`）与输出读取；
//! - `TaskOutput` 的 XML 结果、`display` 字段与两种错误形状；
//! - `TaskStop` 的 JSON 字符串结果、`display` 字段与两种错误形状。
//!
//! # 刻意未实现（依赖 Task 5）
//!
//! **完成通知**：oracle 在任务结束后会**自动开一个新 turn** 并注入 `<task-notification>`。
//! 本 crate 只维护任务状态；通知注入要由会话循环（Task 5）订阅注册表后完成。
//! 因此 `Bash` 的后台结果文案仍照 oracle 逐字输出（「You will be notified…」），
//! 但当前骨架会话**尚不会**真的发通知——这条差异必须在 Task 5 接线时消除。
//!
//! # 观察与推断的边界
//!
//! - 状态取值 `running` / `completed` / `failed` / `cancelled` 来自 spec §3.4；
//! - `running` 状态下的 `<exit_code>` 未观察，这里用 `-1` 显式表示「还没有退出码」；
//! - `TaskStop` 结果 JSON 的**键顺序**按观察逐字拼接（`message` → `task_id` →
//!   `task_type` → `command`），不依赖 `serde_json::Map` 的排序。

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::messages::validation_error;
use crate::{unimplemented_surface, ToolContext, ToolError, ToolOutcome};

/// 任务的线上状态。取值来自 spec §3.4（`session.updated` 与通知里的 `<status>`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    /// `TaskOutput` 结果里的线上 token（逐字）。
    ///
    /// **注意两个面用词不同**：停止后的任务在工具结果里是 `killed`，
    /// 而 `session.updated` 的 `status` 是 `cancelled`（两者都是观察值）。
    pub fn task_output_token(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "killed",
        }
    }
}

/// 一次任务的只读快照（`TaskOutput` 的结果来源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSnapshot {
    pub id: String,
    pub tool_call_id: String,
    pub command: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub output_path: PathBuf,
    /// 输出文件全文（UTF-8 有损解码；文件不存在时为空串）。
    pub output: String,
}

/// 登记成功后的返回（`Bash` 需要 id 与落点来拼结果文案）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRegistration {
    pub id: String,
    pub output_path: PathBuf,
}

#[derive(Debug)]
struct Entry {
    id: String,
    tool_call_id: String,
    command: String,
    description: Option<String>,
    output_path: PathBuf,
    status: TaskStatus,
    exit_code: Option<i32>,
    child: Option<Child>,
}

/// 会话级后台任务表。
///
/// 放在 `Arc` 里由会话持有、经 [`ToolContext::tasks`] 传给工具；`Bash` 登记任务，
/// `TaskOutput` / `TaskStop` 查询与停止。锁的粒度是**单个任务**（map 锁只在查表时持有），
/// 因此 `TaskOutput(block:true)` 等待时不会阻塞别的任务。
#[derive(Debug, Default)]
pub struct TaskRegistry {
    entries: Mutex<HashMap<String, Arc<Mutex<Entry>>>>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个已经 spawn 的进程，并启动回收线程（`try_wait` 轮询，20ms）。
    pub fn register(
        &self,
        child: Child,
        tool_call_id: impl Into<String>,
        command: impl Into<String>,
        description: Option<String>,
        output_path: PathBuf,
    ) -> TaskRegistration {
        let id = next_task_id();
        let entry = Arc::new(Mutex::new(Entry {
            id: id.clone(),
            tool_call_id: tool_call_id.into(),
            command: command.into(),
            description,
            output_path: output_path.clone(),
            status: TaskStatus::Running,
            exit_code: None,
            child: Some(child),
        }));
        self.entries
            .lock()
            .expect("task registry poisoned")
            .insert(id.clone(), Arc::clone(&entry));
        spawn_reaper(entry);
        TaskRegistration { id, output_path }
    }

    /// 任务是否存在（`TaskOutput` 的启动前检查用）。
    pub fn contains(&self, id: &str) -> bool {
        self.entries
            .lock()
            .expect("task registry poisoned")
            .contains_key(id)
    }

    /// 取快照；`block=true` 时等到任务结束或超时。
    pub fn snapshot(&self, id: &str, block: bool, timeout: Duration) -> Option<TaskSnapshot> {
        let entry = self.lookup(id)?;
        let deadline = Instant::now() + timeout;
        loop {
            {
                let guard = entry.lock().expect("task entry poisoned");
                if guard.status != TaskStatus::Running || !block || Instant::now() >= deadline {
                    return Some(snapshot_from(&guard));
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// 停止任务：杀进程、置 `cancelled`。任务不存在返回 `None`。
    pub fn stop(&self, id: &str) -> Option<TaskSnapshot> {
        let entry = self.lookup(id)?;
        let mut guard = entry.lock().expect("task entry poisoned");
        if guard.status == TaskStatus::Running {
            if let Some(mut child) = guard.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            guard.status = TaskStatus::Cancelled;
            // 观察值：停止后的 TaskOutput 报 `<exit_code>137</exit_code>`（128+SIGKILL）。
            guard.exit_code = Some(137);
        }
        Some(snapshot_from(&guard))
    }

    fn lookup(&self, id: &str) -> Option<Arc<Mutex<Entry>>> {
        self.entries
            .lock()
            .expect("task registry poisoned")
            .get(id)
            .cloned()
    }
}

fn spawn_reaper(entry: Arc<Mutex<Entry>>) {
    thread::spawn(move || loop {
        {
            let mut guard = entry.lock().expect("task entry poisoned");
            if guard.status != TaskStatus::Running {
                return;
            }
            let Some(child) = guard.child.as_mut() else {
                return;
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    guard.exit_code = status.code();
                    guard.status = if status.success() {
                        TaskStatus::Completed
                    } else {
                        TaskStatus::Failed
                    };
                    guard.child = None;
                    return;
                }
                Ok(None) => {}
                Err(_) => {
                    guard.status = TaskStatus::Failed;
                    guard.child = None;
                    return;
                }
            }
        }
        thread::sleep(Duration::from_millis(20));
    });
}

fn snapshot_from(entry: &Entry) -> TaskSnapshot {
    let output = std::fs::read(&entry.output_path)
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .unwrap_or_default();
    TaskSnapshot {
        id: entry.id.clone(),
        tool_call_id: entry.tool_call_id.clone(),
        command: entry.command.clone(),
        description: entry.description.clone(),
        status: entry.status,
        exit_code: entry.exit_code,
        output_path: entry.output_path.clone(),
        output,
    }
}

/// 生成 `exec_<uuid>` 形状的 id：nanos + pid + 单调计数器，不引入新依赖。
fn next_task_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    let tail = ((std::process::id() as u128) << 16 | counter) & 0xffff_ffff_ffff;
    format!(
        "exec_{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (nanos & 0xffff_ffff) as u32,
        ((nanos >> 32) & 0xffff) as u16,
        ((nanos >> 48) & 0xffff) as u16,
        ((nanos >> 64) & 0xffff) as u16,
        tail
    )
}

/// `TaskOutput` 的模型侧描述（逐字抄自请求体）。
pub const TASK_OUTPUT_DESCRIPTION: &str = "DEPRECATED: Background tasks return their output file path in the tool result, and you receive a <task-notification> with the same path when the task completes.\n- For bash tasks: prefer using the Read tool on that output file path — it contains stdout/stderr.\n- For local_agent tasks: use the Agent tool result directly. Do NOT Read the .output file — it is a symlink to the full subagent conversation transcript (JSONL) and will overflow your context window.\n- For remote_agent tasks: prefer using the Read tool on the output file path — it contains the streamed remote session output (same as bash).\n\n- Retrieves output from a running or completed task (background shell, agent, or remote session)\n- Takes a task_id parameter identifying the task\n- Returns the task output along with status information\n- Use block=true (default) to wait for task completion\n- Use block=false for non-blocking check of current status\n- Task IDs can be found using the /tasks command\n- Works with all task types: background shells, async agents, and remote sessions";

/// `TaskOutput` 的 `input_schema`（逐字抄自请求体）。
pub const TASK_OUTPUT_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "task_id": {
      "type": "string",
      "description": "The task ID to get output from"
    },
    "block": {
      "type": "boolean",
      "default": true,
      "description": "Whether to wait for completion"
    },
    "timeout": {
      "type": "number",
      "minimum": 0,
      "maximum": 600000,
      "default": 30000,
      "description": "Max wait time in ms"
    }
  },
  "required": [
    "task_id",
    "block",
    "timeout"
  ],
  "additionalProperties": false,
  "$schema": "https://json-schema.org/draft/2020-12/schema"
}"#;

/// `TaskStop` 的模型侧描述（逐字抄自请求体，含首尾换行）。
pub const TASK_STOP_DESCRIPTION: &str = "\n- Stops a running background task by its ID\n- Takes a task_id parameter identifying the task to stop\n- Returns a success or failure status\n- Use this tool when you need to terminate a long-running task\n";

/// `TaskStop` 的 `input_schema`（逐字抄自请求体）。
pub const TASK_STOP_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "task_id": {
      "type": "string",
      "description": "The ID of the background task to stop"
    },
    "shell_id": {
      "type": "string",
      "description": "Deprecated: use task_id instead"
    }
  },
  "additionalProperties": false,
  "$schema": "https://json-schema.org/draft/2020-12/schema"
}"#;

pub fn task_output_input_schema() -> Value {
    serde_json::from_str(TASK_OUTPUT_INPUT_SCHEMA)
        .expect("TASK_OUTPUT_INPUT_SCHEMA 是合法 JSON（有单测钉住）")
}

pub fn task_stop_input_schema() -> Value {
    serde_json::from_str(TASK_STOP_INPUT_SCHEMA)
        .expect("TASK_STOP_INPUT_SCHEMA 是合法 JSON（有单测钉住）")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskOutputInput {
    task_id: String,
    #[serde(default)]
    block: Option<bool>,
    #[serde(default)]
    timeout: Option<f64>,
}

/// `TaskOutput` 的启动前检查（观察：找不到任务时**没有 `started`** 帧）。
///
/// 返回 `Some(error)` 表示应在 `started` 之前直接答复错误；`None` 表示继续正常流程。
pub fn preflight_error(name: &str, args: &Value, ctx: &ToolContext) -> Option<ToolError> {
    if name != "TaskOutput" {
        return None;
    }
    let input: TaskOutputInput = match serde_json::from_value(args.clone()) {
        Ok(input) => input,
        Err(_) => return Some(validation_error()),
    };
    if !valid_timeout(input.timeout) {
        return Some(validation_error());
    }
    let registry = ctx.tasks.as_ref()?;
    (!registry.contains(&input.task_id)).then(|| not_found_output(&input.task_id))
}

pub fn task_output(args: &Value, ctx: &ToolContext) -> Result<ToolOutcome, ToolError> {
    let input: TaskOutputInput =
        serde_json::from_value(args.clone()).map_err(|_| validation_error())?;
    if !valid_timeout(input.timeout) {
        return Err(validation_error());
    }
    let registry = ctx
        .tasks
        .as_ref()
        .ok_or_else(|| unimplemented_surface("task_output_unwired", "会话未接线任务注册表"))?;
    let block = input.block.unwrap_or(true);
    let timeout = Duration::from_millis(input.timeout.unwrap_or(30_000.0).round() as u64);
    let snapshot = registry
        .snapshot(&input.task_id, block, timeout)
        .ok_or_else(|| not_found_output(&input.task_id))?;

    // 逐字形状见 spec §3.4。三种已观察的分支：
    // - running：`not_ready`，**没有** exit_code / output；
    // - killed：`success` + 137，**没有** output；
    // - completed / failed：`success` + 退出码 + output。
    let task_block = format!(
        "<task_id>{}</task_id>\n\n<task_type>local_bash</task_type>",
        snapshot.id
    );
    let (content, display) = match snapshot.status {
        TaskStatus::Running => (
            format!(
                "<retrieval_status>not_ready</retrieval_status>\n\n{task_block}\n\n<status>running</status>"
            ),
            json!({
                "kind": "task_output",
                "retrievalStatus": "not_ready",
                "taskStatus": "running",
            }),
        ),
        TaskStatus::Cancelled => (
            format!(
                "<retrieval_status>success</retrieval_status>\n\n{task_block}\n\n<status>killed</status>\n\n<exit_code>137</exit_code>"
            ),
            json!({
                "kind": "task_output",
                "retrievalStatus": "success",
                "taskStatus": "killed",
            }),
        ),
        status => {
            let head = format!(
                "<retrieval_status>success</retrieval_status>\n\n{task_block}\n\n<status>{}</status>\n\n<exit_code>{}</exit_code>",
                status.task_output_token(),
                snapshot.exit_code.unwrap_or(-1),
            );
            (
                format!("{head}\n\n<output>\n{}</output>", snapshot.output),
                json!({
                    "kind": "task_output",
                    "retrievalStatus": "success",
                    "taskStatus": status.task_output_token(),
                    // 观察值：display 的 output 不带文件末尾换行。
                    "output": snapshot.output.trim_end(),
                }),
            )
        }
    };
    Ok(ToolOutcome::artifact_with_display(
        content,
        display.to_string(),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskStopInput {
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    shell_id: Option<String>,
}

pub fn task_stop(args: &Value, ctx: &ToolContext) -> Result<ToolOutcome, ToolError> {
    let input: TaskStopInput =
        serde_json::from_value(args.clone()).map_err(|_| validation_error())?;
    let id = input
        .task_id
        .or(input.shell_id)
        .ok_or_else(missing_task_id)?;
    let registry = ctx
        .tasks
        .as_ref()
        .ok_or_else(|| unimplemented_surface("task_stop_unwired", "会话未接线任务注册表"))?;
    let snapshot = registry.stop(&id).ok_or_else(|| not_found_stop(&id))?;

    let message = format!(
        "Successfully stopped task: {} ({})",
        snapshot.id, snapshot.command
    );
    let content = format!(
        "{{\"message\":{},\"task_id\":{},\"task_type\":\"local_bash\",\"command\":{}}}",
        serde_json::to_string(&message).expect("字符串可序列化"),
        serde_json::to_string(&snapshot.id).expect("字符串可序列化"),
        serde_json::to_string(&snapshot.command).expect("字符串可序列化"),
    );
    let display = json!({
        "kind": "task_stop",
        "taskId": snapshot.id,
        "taskType": "local_bash",
        "command": snapshot.command,
        "message": message,
    });
    Ok(ToolOutcome::text_with_display(content, display.to_string()))
}

fn valid_timeout(timeout: Option<f64>) -> bool {
    match timeout {
        Some(value) => value.is_finite() && (0.0..=600_000.0).contains(&value),
        None => true,
    }
}

fn not_found_output(id: &str) -> ToolError {
    ToolError {
        error_type: "tool_execution_failed",
        message: format!("No task found with ID: {id}"),
        code: Some("2"),
        detail: Some("code=2".to_string()),
    }
}

fn not_found_stop(id: &str) -> ToolError {
    ToolError {
        error_type: "tool_execution_failed",
        message: format!("No task found with ID: {id}"),
        code: Some("TOOL_EXECUTION_FAILED"),
        detail: Some("code=1 reason=background_task_not_found".to_string()),
    }
}

fn missing_task_id() -> ToolError {
    ToolError {
        error_type: "tool_execution_failed",
        message: "Missing required parameter: task_id".to_string(),
        code: Some("TOOL_EXECUTION_FAILED"),
        detail: Some("code=1".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bash::bash_program;
    use std::process::{Command, Stdio};

    fn registry_context(registry: Arc<TaskRegistry>) -> ToolContext {
        ToolContext {
            cwd: std::env::temp_dir().to_string_lossy().to_string(),
            artifact: None,
            tasks: Some(registry),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mode-task-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn spawn_script(script: &str) -> (Child, PathBuf) {
        let dir = temp_dir("script");
        let path = dir.join("out.log");
        let file = std::fs::File::create(&path).expect("建输出文件");
        let child = Command::new(bash_program())
            .arg("-lc")
            .arg(format!("exec 2>&1\n{script}"))
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn 测试进程");
        (child, path)
    }

    #[test]
    fn task_output_reports_completed_task_in_the_observed_shape() {
        let registry = Arc::new(TaskRegistry::new());
        let (child, path) = spawn_script("printf 'quick\\n'; printf 'quick-done\\n'");
        let registration = registry.register(child, "call_bg", "quick", Some("Quick".into()), path);
        let ctx = registry_context(Arc::clone(&registry));

        let outcome = task_output(
            &json!({ "task_id": registration.id, "block": true, "timeout": 5000 }),
            &ctx,
        )
        .expect("任务应能读到结果");
        assert_eq!(outcome.budget_strategy, "artifact");
        assert!(outcome
            .content
            .starts_with("<retrieval_status>success</retrieval_status>"));
        assert!(outcome.content.contains("<status>completed</status>"));
        assert!(outcome.content.contains("<exit_code>0</exit_code>"));
        assert!(outcome
            .content
            .contains("<output>\nquick\nquick-done\n</output>"));
        let display: Value =
            serde_json::from_str(outcome.display_json.as_deref().expect("有 display"))
                .expect("合法 JSON");
        assert_eq!(display["kind"], "task_output");
        assert_eq!(display["taskStatus"], "completed");
        assert_eq!(display["output"], "quick\nquick-done");
    }

    #[test]
    fn task_output_missing_task_uses_the_observed_error_codes() {
        let registry = Arc::new(TaskRegistry::new());
        let ctx = registry_context(registry);
        let error = task_output(&json!({ "task_id": "exec_missing" }), &ctx).expect_err("应失败");
        assert_eq!(error.error_type, "tool_execution_failed");
        assert_eq!(error.message, "No task found with ID: exec_missing");
        assert_eq!(error.code, Some("2"));
        assert_eq!(error.detail.as_deref(), Some("code=2"));
    }

    #[test]
    fn task_output_missing_input_is_a_validation_error() {
        let registry = Arc::new(TaskRegistry::new());
        let ctx = registry_context(registry);
        let error = task_output(&json!({}), &ctx).expect_err("缺 task_id 应失败");
        assert_eq!(error.message, "Tool input failed inputSchema validation");
        assert_eq!(error.code, Some("TOOL_EXECUTION_FAILED"));
    }

    #[test]
    fn task_stop_cancels_and_reports_the_observed_json() {
        let registry = Arc::new(TaskRegistry::new());
        let (child, path) = spawn_script("sleep 30");
        let registration = registry.register(child, "call_bg", "sleep 30", None, path);
        let ctx = registry_context(Arc::clone(&registry));

        let outcome = task_stop(&json!({ "task_id": registration.id }), &ctx).expect("应能停止");
        assert_eq!(outcome.budget_strategy, "truncate");
        let parsed: Value = serde_json::from_str(&outcome.content).expect("正文是 JSON 字符串");
        assert_eq!(parsed["task_id"], registration.id);
        assert_eq!(parsed["task_type"], "local_bash");
        assert_eq!(parsed["command"], "sleep 30");
        assert_eq!(
            parsed["message"],
            format!("Successfully stopped task: {} (sleep 30)", registration.id)
        );
        let display: Value =
            serde_json::from_str(outcome.display_json.as_deref().expect("有 display"))
                .expect("合法 JSON");
        assert_eq!(display["kind"], "task_stop");
        assert_eq!(display["taskId"], registration.id);

        // 观察值：停止后 TaskOutput 是 `killed` + `137`，且没有 `<output>` 段。
        let stopped =
            task_output(&json!({ "task_id": registration.id }), &ctx).expect("停止后仍能查输出");
        assert!(stopped.content.contains("<status>killed</status>"));
        assert!(stopped.content.contains("<exit_code>137</exit_code>"));
        assert!(!stopped.content.contains("<output>"));
        let stopped_display: Value =
            serde_json::from_str(stopped.display_json.as_deref().expect("有 display"))
                .expect("合法 JSON");
        assert_eq!(stopped_display["taskStatus"], "killed");
        assert!(stopped_display.get("output").is_none());
    }

    #[test]
    fn task_output_reports_failed_task_with_output() {
        let registry = Arc::new(TaskRegistry::new());
        let (child, path) = spawn_script("printf 'bad\\n'; exit 7");
        let registration = registry.register(child, "call_bg", "bad", None, path);
        let ctx = registry_context(Arc::clone(&registry));

        let outcome = task_output(
            &json!({ "task_id": registration.id, "block": true, "timeout": 5000 }),
            &ctx,
        )
        .expect("失败任务也能读输出");
        assert!(outcome.content.contains("<status>failed</status>"));
        assert!(outcome.content.contains("<exit_code>7</exit_code>"));
        assert!(outcome.content.contains("<output>\nbad\n</output>"));
        let display: Value =
            serde_json::from_str(outcome.display_json.as_deref().expect("有 display"))
                .expect("合法 JSON");
        assert_eq!(display["taskStatus"], "failed");
        assert_eq!(display["output"], "bad");
    }

    #[test]
    fn task_output_running_uses_not_ready_without_output() {
        let registry = Arc::new(TaskRegistry::new());
        let (child, path) = spawn_script("sleep 5");
        let registration = registry.register(child, "call_bg", "sleep 5", None, path);
        let ctx = registry_context(Arc::clone(&registry));

        let outcome = task_output(
            &json!({ "task_id": registration.id, "block": false, "timeout": 5000 }),
            &ctx,
        )
        .expect("运行中的任务也可查询");
        assert!(outcome
            .content
            .contains("<retrieval_status>not_ready</retrieval_status>"));
        assert!(outcome.content.contains("<status>running</status>"));
        assert!(!outcome.content.contains("<exit_code>"));
        assert!(!outcome.content.contains("<output>"));
        let display: Value =
            serde_json::from_str(outcome.display_json.as_deref().expect("有 display"))
                .expect("合法 JSON");
        assert_eq!(display["retrievalStatus"], "not_ready");
        assert_eq!(display["taskStatus"], "running");
        assert!(display.get("output").is_none());

        registry.stop(&registration.id).expect("清理运行中的任务");
    }

    #[test]
    fn task_stop_missing_and_unknown_ids_use_the_observed_errors() {
        let registry = Arc::new(TaskRegistry::new());
        let ctx = registry_context(registry);
        let missing = task_stop(&json!({}), &ctx).expect_err("缺 task_id 应失败");
        assert_eq!(missing.message, "Missing required parameter: task_id");
        assert_eq!(missing.detail.as_deref(), Some("code=1"));

        let unknown =
            task_stop(&json!({ "task_id": "exec_missing" }), &ctx).expect_err("未知 id 应失败");
        assert_eq!(unknown.message, "No task found with ID: exec_missing");
        assert_eq!(unknown.code, Some("TOOL_EXECUTION_FAILED"));
        assert_eq!(
            unknown.detail.as_deref(),
            Some("code=1 reason=background_task_not_found")
        );
    }

    #[test]
    fn preflight_only_applies_to_task_output() {
        let registry = Arc::new(TaskRegistry::new());
        let ctx = registry_context(registry);
        assert!(
            preflight_error("TaskOutput", &json!({ "task_id": "exec_missing" }), &ctx).is_some()
        );
        assert!(preflight_error("TaskOutput", &json!({}), &ctx).is_some());
        assert!(preflight_error("TaskStop", &json!({}), &ctx).is_none());
    }

    #[test]
    fn schemas_are_valid_json() {
        assert_eq!(task_output_input_schema()["required"][0], "task_id");
        assert_eq!(task_stop_input_schema()["additionalProperties"], false);
    }
}
