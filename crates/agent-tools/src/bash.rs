//! `Bash` 工具（M2 Task 4）。
//!
//! 规格：`docs/specs/tauri-agent-runtime.md` §3.4（Task 4 Step 0 的黑盒观察）。
//! 纪律：`docs/specs/clean-room-policy.md`——本文件只取材于**黑盒观察**与 spec，不读旧实现。
//!
//! # 观察到的行为（每条都有出处）
//!
//! | 场景 | 结果 | 出处 |
//! | --- | --- | --- |
//! | 正常输出 | 成功；正文去掉**尾随空白**（换行与空格都算） | 夹具 `bash-stdout` / `bash-trailing-whitespace` |
//! | 非零退出 | **成功**；正文首行 `Exit code {code}`，其后是输出 | 夹具 `bash-exit-7` / `bash-output-then-exit-7` |
//! | stderr | 与 stdout **按到达顺序交错**合并（不是先 stdout 后 stderr） | 夹具 `bash-mixed-streams` |
//! | 无输出 | `(Bash completed with no output)` | 夹具 `bash-empty-output` |
//! | 命令不存在 | `Exit code 127` + shell 的 `command not found` 行 | 夹具 `bash-command-not-found` |
//! | 超时 | **成功**；`Command timed out after {秒}s` + `<error>Command was aborted before completion</error>` | 夹具 `bash-timeout` / `bash-timeout-fractional` |
//! | 缺 `command` | `tool_execution_failed` / `Tool input failed inputSchema validation` | 夹具 `bash-empty-args` |
//! | 小输出 | `budgetStrategy` 也是 **`"artifact"`**（只是没有 `artifactPath`） | `probe-bash-threshold.mts` |
//! | 输出超过 30,000 字节 | `<persisted-output>` 包装 + 完整输出落盘；`budgetStrategy=artifact` | spec §3.4「大输出」 |
//! | 运行中取消 | 工具错误 `tool_cancelled` / `TOOL_CANCELLED` | spec §3.4「取消」 |
//!
//! # 刻意不做、也不假装做的
//!
//! - **`run_in_background`**：尚未观察其返回与后续事件，属于 Task 4b；这里显式返回
//!   [`crate::UNIMPLEMENTED_ERROR_TYPE`]，不编造线上形状。
//! - **pty**：oracle 的 `[ -t 1 ]` / `[ -t 0 ]` 都是假（pipe），所以用管道，不引 `portable-pty`。
//! - **超时的孙进程清理**：oracle 只 abort 父 shell，子进程仍存活；这里保持一致（只 kill shell）。
//! - **artifact 的真实落点**：oracle 落在 `<dataRoot>/.mode/cli/exec/<sessionId>/<toolCallId>-stdout.log`。
//!   注册表路径按 [`ToolContext::artifact`] 派生；未接线数据根的会话仍落系统临时目录
//!   （夹具不覆盖 >30KB，避免路径不可复现）。
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::messages::validation_error;
use crate::{unimplemented_surface, ToolContext, ToolError, ToolOutcome};

/// `timeout` 缺省值（观察：声明的描述写 `default 120000`）。
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// `timeout` 上限（观察：schema 的 `max 600000`）。
pub const MAX_TIMEOUT_MS: u64 = 600_000;
/// 输出超过这个字节数就落盘（观察：30,000 直出、30,720 起持久化）。
pub const LARGE_OUTPUT_BYTES: usize = 30_000;
/// 持久化包装里的预览长度（观察：标题写 `Preview (first 2KB)`）。
pub const PREVIEW_BYTES: usize = 2048;

/// 无输出的成功正文。
pub const EMPTY_OUTPUT: &str = "(Bash completed with no output)";

/// `Bash` 的模型侧工具描述——从 oracle 的请求体里逐字抄下（探针 `probe-bash.mts`）。
pub const BASH_DESCRIPTION: &str = "Executes a bash command and returns its output.\n\n- Working directory persists between calls, but prefer absolute paths — `cd` in a compound command can trigger a permission prompt. Shell state (env vars, functions) does not persist; the shell is initialized from the user's profile.\n- IMPORTANT: Avoid using this tool to run `cat`, `head`, `tail`, `sed`, `awk`, or `echo` commands, unless explicitly instructed or after you have verified that a dedicated tool cannot accomplish your task. Instead, use the appropriate dedicated tool as this will provide a much better experience for the user.\n- `timeout` is in milliseconds: default 120000, max 600000.\n- `run_in_background` runs the command detached: it keeps running across turns and re-invokes you when it exits. No `&` needed.\n\n# Git\n- Interactive flags (`-i`, e.g. `git rebase -i`, `git add -i`) are not supported in this environment.\n- Use the `gh` CLI for GitHub operations (PRs, issues, API).\n- Commit or push only when the user asks. If on the default branch, branch first.";

/// `Bash` 的模型侧 `input_schema`——逐字抄自 oracle 的请求体。
pub const BASH_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "command": {
      "type": "string",
      "description": "The command to execute"
    },
    "timeout": {
      "type": "number",
      "description": "Optional timeout in milliseconds (max 600000)"
    },
    "description": {
      "type": "string",
      "description": "Clear, concise description of what this command does in active voice. Never use words like \"complex\" or \"risk\" in the description - just describe what it does.\n\nFor simple commands (git, npm, standard CLI tools), keep it brief (5-10 words):\n- ls → \"List files in current directory\"\n- git status → \"Show working tree status\"\n- npm install → \"Install package dependencies\"\n\nFor commands that are harder to parse at a glance (pipes, obscure flags, etc.), add enough context to clarify what it does:\n- find . -name \"*.tmp\" -exec rm {} \\; → \"Find and delete all .tmp files recursively\"\n- git reset --hard origin/main → \"Discard all local changes and match remote main\"\n- curl -s url | jq '.data[]' → \"Fetch JSON from URL and extract data array elements\""
    },
    "run_in_background": {
      "type": "boolean",
      "description": "Set this to true to run this command in the background."
    },
    "dangerouslyDisableSandbox": {
      "type": "boolean",
      "description": "Set this to true to dangerously override sandbox mode and run commands without sandboxing."
    }
  },
  "required": [
    "command"
  ],
  "additionalProperties": false,
  "$schema": "https://json-schema.org/draft/2020-12/schema"
}"#;

/// 解析 [`BASH_INPUT_SCHEMA`]（同一份常量，供注册表用）。
pub fn bash_input_schema() -> Value {
    serde_json::from_str(BASH_INPUT_SCHEMA).expect("BASH_INPUT_SCHEMA 是合法 JSON（有单测钉住）")
}

/// 取消令牌：会话侧在 `session/stop` 时置位，工具在轮询循环里响应。
#[derive(Debug, Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

impl PartialEq for Cancellation {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Cancellation {}

/// `Bash` 的执行控制面：取消令牌 + artifact 落点。
///
/// 缺省值用于 [`invoke_with_control`] 的直接调用方：不取消、artifact 落系统临时目录。
/// 注册表路径（[`invoke`]）从 [`ToolContext::artifact`] 派生落点。
#[derive(Debug, Clone, Default)]
pub struct BashControl {
    pub cancel: Cancellation,
    /// artifact 目录；`None` = 落系统临时目录。
    pub artifact_dir: Option<PathBuf>,
    /// artifact 文件名；`None` = 用进程 id + 时间戳生成。会话路径下是
    /// `<toolCallId>-stdout.log`（观察值）。
    pub artifact_name: Option<String>,
}

impl BashControl {
    /// 从会话上下文派生落点：已接线数据根时落
    /// `{data_root}/cli/exec/{session_id}/{tool_call_id}-stdout.log`，否则保持临时目录。
    fn from_context(ctx: &ToolContext) -> Self {
        let artifact = ctx.artifact.as_ref();
        Self {
            cancel: Cancellation::new(),
            artifact_dir: artifact.map(crate::ArtifactLocation::dir),
            artifact_name: artifact.map(crate::ArtifactLocation::file_name),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BashInput {
    command: String,
    #[serde(default)]
    timeout: Option<f64>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "run_in_background")]
    run_in_background: Option<bool>,
    #[serde(default, rename = "dangerouslyDisableSandbox")]
    dangerously_disable_sandbox: Option<bool>,
}

/// 执行一次 `Bash`（注册表路径：无取消；artifact 落点由 [`ToolContext::artifact`] 决定）。
pub fn invoke(args: &Value, ctx: &ToolContext) -> Result<ToolOutcome, ToolError> {
    invoke_with_control(args, ctx, &BashControl::from_context(ctx))
}

/// 执行一次 `Bash`，允许注入取消令牌与 artifact 落点（会话接线与单测用）。
pub fn invoke_with_control(
    args: &Value,
    ctx: &ToolContext,
    control: &BashControl,
) -> Result<ToolOutcome, ToolError> {
    let input: BashInput = serde_json::from_value(args.clone()).map_err(|_| validation_error())?;
    let timeout_ms = match input.timeout {
        Some(value) if value.is_finite() && (0.0..=MAX_TIMEOUT_MS as f64).contains(&value) => {
            value.round() as u64
        }
        Some(_) => return Err(validation_error()),
        None => DEFAULT_TIMEOUT_MS,
    };
    if input.run_in_background == Some(true) {
        return start_background(&input.command, input.description, ctx, control);
    }
    // 未观察到 `dangerouslyDisableSandbox` 的行为差异；接受但无沙箱可关，保持透明。
    let _ = input.dangerously_disable_sandbox;
    let _ = input.description;

    let mut command = shell_command(&input.command, ctx);
    let mut child = command
        .spawn()
        .map_err(|error| unimplemented_surface("bash_spawn_failed", error.to_string()))?;
    // 必须在等待子进程期间持续读管道：只 `try_wait` 不读会让写满 64KB 管道缓冲的
    // 命令永远退不出去（表现为「健康命令必超时」）。两条管道各一条读线程，
    // stdout/stderr 由 `exec 2>&1` 合流，读线程退出后再按到达顺序拼接。
    let stdout_reader = spawn_pipe_reader(child.stdout.take());
    let stderr_reader = spawn_pipe_reader(child.stderr.take());
    // 前台命令同样挂进 kill-on-close 作业对象：函数内任一提前返回（取消/超时）
    // 都会先 kill 子进程再析构句柄，句柄在 agent 进程被强杀时由 OS 关闭。
    #[cfg(windows)]
    let _child_job = crate::win_job::JobHandle::assign(&child);

    let started = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    let status = loop {
        if control.cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = join_reader(stdout_reader);
            let _ = join_reader(stderr_reader);
            return Err(cancelled_error());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = join_reader(stdout_reader);
                let _ = join_reader(stderr_reader);
                return Err(unimplemented_surface("bash_wait_failed", error.to_string()));
            }
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = join_reader(stdout_reader);
            let _ = join_reader(stderr_reader);
            // 观察：超时同样是 `budgetStrategy:"artifact"`（Bash 的通用预算策略）。
            // `perf.detail`（夹具 `bash-timeout`）：`exitCode` 是 143（SIGTERM 的线上取值）、
            // `timedOut:true`、`status:"timed_out"`；管道没读，`outputBytes` 按观察值 0。
            return Ok(
                ToolOutcome::artifact(timeout_content(timeout_ms)).with_detail(command_detail(
                    &input.command,
                    CommandDetail {
                        run_ms: started.elapsed().as_millis() as u64,
                        no_output_ms: started.elapsed().as_millis() as u64,
                        exit_code: Some(143),
                        timed_out: true,
                        output_bytes: 0,
                    },
                )),
            );
        }
        thread::sleep(Duration::from_millis(20));
    };

    let stdout = join_reader(stdout_reader);
    let stderr = join_reader(stderr_reader);
    let run_ms = started.elapsed().as_millis() as u64;
    let (content, original_bytes) = normalize_output(&stdout, &stderr, status.code());
    // `perf.detail.command.outputBytes`：**原始**合并输出的字节数（trim 前；夹具
    // `bash-stdout`：原始 12 = "hello\nworld\n"，正文 trim 后是 11）。
    let output_bytes = (stdout.len() + stderr.len()) as u64;
    if content.len() > LARGE_OUTPUT_BYTES {
        let detail = command_detail(
            &input.command,
            CommandDetail {
                run_ms,
                no_output_ms: run_ms,
                exit_code: status.code(),
                timed_out: false,
                output_bytes,
            },
        );
        return persist_large_output(content, control).map(|wrapped| {
            let mut outcome = ToolOutcome::artifact(wrapped);
            // 大输出包装的 `originalBytes` 仍是包装后正文的字节数（未观察到别的口径）。
            outcome.original_bytes = outcome.content.len() as u64;
            outcome.with_detail(detail)
        });
    }
    // 观察（probe-bash-threshold）：小输出的 `budgetStrategy` 也是 `"artifact"`，
    // 只是没有 `artifactPath`。别把它当成 `"truncate"`（那是 TaskStop 等工具的取值）。
    let mut outcome = ToolOutcome::artifact(content);
    // `bash-empty-output`：`originalBytes` 是 trim 后真实输出的字节数（空 → 0），
    // `returnedBytes` 是占位文案的长度——占位文案不算原始输出。
    outcome.original_bytes = original_bytes;
    Ok(outcome.with_detail(command_detail(
        &input.command,
        CommandDetail {
            run_ms,
            no_output_ms: run_ms,
            exit_code: status.code(),
            timed_out: false,
            output_bytes,
        },
    )))
}

/// `perf.detail.command` 的已测量事实（C1.1-#6 对齐；形状来自行为夹具 bash-* 的录制）。
struct CommandDetail {
    run_ms: u64,
    no_output_ms: u64,
    exit_code: Option<i32>,
    timed_out: bool,
    output_bytes: u64,
}

/// 组装 `perf.detail`：`{kind:"command", command:{…}}`。
///
/// - `category` 恒为 `"other"`（全部已观察命令都如此）；空命令走观察到的**缺省形态**
///   （探针实测：`category:"empty"` / `count:0` / `name:"empty"` / `status:"completed"`，
///   且没有 `runMs` / `noOutputMs` / `exitCode` / `timedOut` / `outputBytes` 键）。
/// - `count` 是 `;` 分段数（夹具 `bash-mixed-streams`：3 段；`bash-output-then-exit-7`：2 段）；
///   `name` 按观察值映射：多段 `"compound"`、单段 `"other"`、空 `"empty"`。
/// - `status`：退出 0 且未超时 `"completed"`；超时 `"timed_out"`；其余 `"failed"`。
/// - **`hash` 不产出**：oracle 录的是命令内容指纹，其算法无法从黑盒观察推导
///   （探针实测它与 cwd/描述/输出/耗时无关、只随命令文本变化，但 sha256/sha1/md5/blake/
///   xxh64/fnv/murmur/crc64 及带盐变体均不复现）。按 C1 纪律缺省而不是编一个数，
///   差异登记在 spec §C1。
fn command_detail(command: &str, detail: CommandDetail) -> String {
    let empty = command.trim().is_empty();
    let count = if empty { 0 } else { command.split(';').count() };
    let name = if empty {
        "empty"
    } else if count > 1 {
        "compound"
    } else {
        "other"
    };
    let status = if empty {
        "completed"
    } else if detail.timed_out {
        "timed_out"
    } else if detail.exit_code == Some(0) {
        "completed"
    } else {
        "failed"
    };
    let mut inner = Map::new();
    if !empty {
        inner.insert("runMs".to_string(), json!(detail.run_ms));
        inner.insert("noOutputMs".to_string(), json!(detail.no_output_ms));
        if let Some(code) = detail.exit_code {
            inner.insert("exitCode".to_string(), json!(code));
        }
        inner.insert("timedOut".to_string(), json!(detail.timed_out));
        inner.insert("outputBytes".to_string(), json!(detail.output_bytes));
    }
    inner.insert(
        "category".to_string(),
        json!(if empty { "empty" } else { "other" }),
    );
    inner.insert("count".to_string(), json!(count));
    inner.insert("name".to_string(), json!(name));
    inner.insert("status".to_string(), json!(status));
    json!({ "kind": "command", "command": Value::Object(inner) }).to_string()
}

/// 后台执行：立即登记任务并返回（不等待命令结束）。
///
/// 观察（spec §3.4）：结果是 `success=true` 的一段固定文案，`budgetStrategy:"artifact"`，
/// 并带 `artifactPath`；输出文件在命令结束前也可读。**完成通知**（新 turn）不在这里做，
/// 由注册表投递、`agent-server` worker 合并成一个通知 turn（C3 T3）——本函数只把任务
/// 写进注册表。
fn start_background(
    command: &str,
    description: Option<String>,
    ctx: &ToolContext,
    control: &BashControl,
) -> Result<ToolOutcome, ToolError> {
    let registry = ctx.tasks.as_ref().ok_or_else(|| {
        unimplemented_surface(
            "bash_run_in_background_unwired",
            "会话未接线任务注册表，无法登记后台任务",
        )
    })?;
    let dir = control
        .artifact_dir
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("mode-bash-artifacts"));
    std::fs::create_dir_all(&dir)
        .map_err(|error| unimplemented_surface("bash_artifact_dir_failed", error.to_string()))?;
    // 文件名沿用 artifact 落点规则：会话接线时是观察到的 `<toolCallId>-stdout.log`。
    let name = control
        .artifact_name
        .clone()
        .unwrap_or_else(|| format!("bash-background-{}.log", std::process::id()));
    let path = dir.join(&name);

    let file = std::fs::File::create(&path)
        .map_err(|error| unimplemented_surface("bash_artifact_write_failed", error.to_string()))?;
    let mut command_builder = shell_command(command, ctx);
    command_builder
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        // `exec 2>&1` 已把 shell 的 fd2 指向 stdout（文件）；这里不发散管道，避免无人读取阻塞。
        .stderr(Stdio::null());
    let child = command_builder
        .spawn()
        .map_err(|error| unimplemented_surface("bash_spawn_failed", error.to_string()))?;

    let tool_call_id = control
        .artifact_name
        .as_deref()
        .and_then(|name| name.strip_suffix("-stdout.log"))
        .unwrap_or("background")
        .to_string();
    let registration = registry.register(child, tool_call_id, command, description, path);
    let content = format!(
        "Command running in background with ID: {}. Output is being written to: {}. You will be notified when it completes. To check interim output, use Read on that file path.",
        registration.id,
        registration.output_path.display()
    );
    Ok(ToolOutcome::background(
        content,
        registration.output_path.display().to_string(),
    ))
}

/// 在独立线程里读完一条管道，避免子进程因管道缓冲写满而卡死。
fn spawn_pipe_reader<R>(pipe: Option<R>) -> Option<thread::JoinHandle<std::io::Result<Vec<u8>>>>
where
    R: Read + Send + 'static,
{
    pipe.map(|mut pipe| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
    })
}

/// 等读线程收尾；读/线程失败时按空输出处理（进程退出码仍可反映失败）。
fn join_reader(handle: Option<thread::JoinHandle<std::io::Result<Vec<u8>>>>) -> Vec<u8> {
    handle
        .and_then(|handle| handle.join().ok())
        .and_then(Result::ok)
        .unwrap_or_default()
}

fn shell_command(command: &str, ctx: &ToolContext) -> Command {
    let mut cmd = Command::new(bash_program());
    // `exec 2>&1` 让**整段脚本**的 stderr 与 stdout 在同一个管道里按到达顺序交错。
    // 不能把 `2>&1` 追加在命令末尾：那只作用于最后一条命令（实测会得到 `out1\nout2\nerr1`）。
    cmd.arg("-lc").arg(format!("exec 2>&1\n{command}"));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !ctx.cwd.is_empty() {
        cmd.current_dir(&ctx.cwd);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 与 sidecar-host 的既有做法一致：不要在桌面弹控制台窗口。
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// 找 bash：优先 PATH，其次 Git for Windows 的常见安装位置。
///
/// 找不到时交给 `Command::new("bash")` 报 spawn 失败（显式标记，不静默降级到 `sh`——
/// `sh` 的语法与 `bash` 不同，静默替换会把行为差异伪装成工具行为）。
pub(crate) fn bash_program() -> PathBuf {
    #[cfg(windows)]
    {
        for candidate in [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files\Git\usr\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ] {
            let path = PathBuf::from(candidate);
            if path.exists() {
                return path;
            }
        }
    }
    #[cfg(not(windows))]
    {
        let path = PathBuf::from("/bin/bash");
        if path.exists() {
            return path;
        }
    }
    PathBuf::from("bash")
}

fn cancelled_error() -> ToolError {
    ToolError {
        error_type: "tool_cancelled",
        message: "Bash was cancelled and the child process was asked to stop".to_string(),
        code: Some("TOOL_CANCELLED"),
        detail: None,
    }
}

/// 超时正文：秒数保留一位小数、去掉尾随 `.0`（观察：1000 → `1s`、1500 → `1.5s`）。
fn timeout_content(timeout_ms: u64) -> String {
    let seconds = format!("{:.1}", timeout_ms as f64 / 1000.0);
    let seconds = seconds.trim_end_matches(".0");
    format!(
        "Command timed out after {seconds}s\n<error>Command was aborted before completion</error>"
    )
}

/// 输出规范化：合并 stderr、`trim_end`、非零退出前缀、空输出文案。
///
/// 观察（spec §3.4）：`Exit code N` 是**首行**（有输出时在输出之前）；空输出只在退出码为 0 时
/// 才用 `(Bash completed with no output)`（`exit 7` 只有 `Exit code 7`）。
///
/// 返回值里的第二个分量是 `result.originalBytes`：夹具 `bash-empty-output` 的
/// `originalBytes` 是 **0**（trim 后的真实输出为空），而 `returnedBytes` 是占位文案的长度
/// （31）——占位文案不算「原始输出」。其余分支两个字节数都等于正文长度。
fn normalize_output(stdout: &[u8], stderr: &[u8], code: Option<i32>) -> (String, u64) {
    let mut combined = String::from_utf8_lossy(stdout).to_string();
    if !stderr.is_empty() {
        combined.push_str(&String::from_utf8_lossy(stderr));
    }
    let trimmed = combined.trim_end();
    let content = match code {
        Some(0) if trimmed.is_empty() => EMPTY_OUTPUT.to_string(),
        Some(0) => trimmed.to_string(),
        Some(code) if trimmed.is_empty() => format!("Exit code {code}"),
        Some(code) => format!("Exit code {code}\n{trimmed}"),
        // 被信号终止（Unix）：oracle 未观察；用 -1 显式暴露，不编一个 shell 风格的数字。
        None if trimmed.is_empty() => "Exit code -1".to_string(),
        None => format!("Exit code -1\n{trimmed}"),
    };
    let original_bytes = if code == Some(0) && trimmed.is_empty() {
        0
    } else {
        content.len() as u64
    };
    (content, original_bytes)
}

fn persist_large_output(content: String, control: &BashControl) -> Result<String, ToolError> {
    let dir = control
        .artifact_dir
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("mode-bash-artifacts"));
    std::fs::create_dir_all(&dir)
        .map_err(|error| unimplemented_surface("bash_artifact_dir_failed", error.to_string()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    // 已接线会话时用观察到的 `<toolCallId>-stdout.log`；未接线时保留临时目录下的可读名字。
    let name = control
        .artifact_name
        .clone()
        .unwrap_or_else(|| format!("bash-{}-{stamp}.log", std::process::id()));
    let path = dir.join(name);
    std::fs::write(&path, content.as_bytes())
        .map_err(|error| unimplemented_surface("bash_artifact_write_failed", error.to_string()))?;

    let bytes = content.as_bytes();
    let preview_len = bytes.len().min(PREVIEW_BYTES);
    let preview = String::from_utf8_lossy(&bytes[..preview_len]);
    Ok(format!(
        "<persisted-output>\nOutput too large ({}). Full output saved to: {}\n\nPreview (first 2KB):\n{}\n...\n</persisted-output>",
        format_kb(bytes.len()),
        path.display(),
        preview
    ))
}

/// 人类可读的 KB 标签：一位小数、去掉尾随 `.0`（观察：30KB / 195.3KB / 293KB / 976.6KB）。
fn format_kb(bytes: usize) -> String {
    let text = format!("{:.1}", bytes as f64 / 1024.0);
    format!("{}KB", text.trim_end_matches(".0"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn context() -> ToolContext {
        ToolContext {
            cwd: std::env::temp_dir().to_string_lossy().to_string(),
            artifact: None,
            tasks: None,
        }
    }

    fn run(command: &str) -> ToolOutcome {
        invoke(&json!({ "command": command }), &context()).expect("Bash 调用应当成功")
    }

    #[test]
    fn stdout_is_trailing_trimmed() {
        assert_eq!(run("printf 'hello\\nworld\\n'").content, "hello\nworld");
        assert_eq!(run("printf 'a   \\n'").content, "a");
        assert_eq!(run("printf 'a\\n\\n\\n'").content, "a");
    }

    #[test]
    fn non_zero_exit_is_success_with_exit_code_first() {
        let exit = run("exit 7");
        assert_eq!(exit.content, "Exit code 7");
        assert_eq!(exit.budget_strategy, "artifact");
        assert_eq!(
            run("printf 'before\\n'; exit 7").content,
            "Exit code 7\nbefore"
        );
    }

    #[test]
    fn stderr_is_merged_in_arrival_order() {
        assert_eq!(run("printf 'oops\\n' >&2").content, "oops");
        assert_eq!(
            run("printf 'out1\\n'; printf 'err1\\n' >&2; printf 'out2\\n'").content,
            "out1\nerr1\nout2"
        );
    }

    #[test]
    fn empty_output_has_a_stable_message() {
        assert_eq!(run("true").content, EMPTY_OUTPUT);
    }

    /// 200KB 远超管道缓冲：若等待期间不读管道，子进程会卡在 write 上直到超时。
    #[test]
    fn large_output_is_drained_while_waiting() {
        let outcome = invoke(
            &json!({
                "command": "for i in $(seq 1 20000); do printf '0123456789\n'; done",
                "timeout": 15000
            }),
            &context(),
        )
        .expect("large output");
        assert!(
            outcome.content.contains("Output too large"),
            "{}",
            outcome.content.chars().take(160).collect::<String>()
        );
    }

    #[test]
    fn missing_command_is_reported_by_the_shell() {
        let content = run("definitely_not_a_command_xyz").content;
        assert!(content.starts_with("Exit code 127\n"), "{content}");
        assert!(
            content.contains("definitely_not_a_command_xyz: command not found"),
            "{content}"
        );
    }

    #[test]
    fn timeout_content_matches_the_observed_format() {
        let one = invoke(
            &json!({ "command": "sleep 5", "timeout": 1000 }),
            &context(),
        )
        .expect("超时仍是成功结果");
        assert_eq!(
            one.content,
            "Command timed out after 1s\n<error>Command was aborted before completion</error>"
        );

        let fractional = invoke(
            &json!({ "command": "sleep 3", "timeout": 1500 }),
            &context(),
        )
        .expect("超时仍是成功结果");
        assert!(fractional
            .content
            .starts_with("Command timed out after 1.5s\n"));
    }

    #[test]
    fn invalid_input_is_a_validation_error() {
        let missing = invoke(&json!({}), &context()).expect_err("缺 command 必须失败");
        assert_eq!(missing.message, "Tool input failed inputSchema validation");
        assert_eq!(missing.code, Some("TOOL_EXECUTION_FAILED"));

        let unknown = invoke(&json!({ "command": "true", "nope": 1 }), &context())
            .expect_err("未知键必须失败");
        assert_eq!(unknown.message, "Tool input failed inputSchema validation");
    }

    #[test]
    fn run_in_background_is_an_explicit_unimplemented_surface() {
        let error = invoke(
            &json!({ "command": "true", "run_in_background": true }),
            &context(),
        )
        .expect_err("后台任务尚未实现");
        assert_eq!(error.error_type, crate::UNIMPLEMENTED_ERROR_TYPE);
        assert!(error
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("bash_run_in_background"));
    }

    #[test]
    fn cancellation_returns_tool_cancelled() {
        let control = BashControl::default();
        let cancel = control.cancel.clone();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(120));
            cancel.cancel();
        });
        let error = invoke_with_control(
            &json!({ "command": "sleep 5", "timeout": 60000 }),
            &context(),
            &control,
        )
        .expect_err("取消必须是工具错误");
        worker.join().expect("取消线程不应 panic");
        assert_eq!(error.error_type, "tool_cancelled");
        assert_eq!(error.code, Some("TOOL_CANCELLED"));
        assert_eq!(
            error.message,
            "Bash was cancelled and the child process was asked to stop"
        );
    }

    #[test]
    fn large_output_is_persisted_with_the_observed_wrapper() {
        let dir = std::env::temp_dir().join(format!(
            "mode-bash-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        let control = BashControl {
            cancel: Cancellation::new(),
            artifact_dir: Some(dir.clone()),
            artifact_name: None,
        };
        let outcome = invoke_with_control(
            &json!({ "command": "head -c 31000 /dev/zero | tr '\\0' 'x'" }),
            &context(),
            &control,
        )
        .expect("大输出仍是成功结果");

        assert_eq!(outcome.budget_strategy, "artifact");
        assert!(!outcome.truncated);
        assert!(outcome
            .content
            .starts_with("<persisted-output>\nOutput too large (30.3KB)."));
        assert!(outcome.content.contains("Preview (first 2KB):"));
        assert!(outcome.content.ends_with("\n...\n</persisted-output>"));

        let log = std::fs::read_dir(&dir)
            .expect("artifact 目录应存在")
            .next()
            .expect("应产出一个 artifact")
            .expect("读 artifact 目录项")
            .path();
        assert_eq!(std::fs::read(&log).expect("读 artifact").len(), 31_000);
        assert!(Path::new(&log).exists(), "artifact 文件应存在");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn artifact_location_from_context_uses_the_observed_path() {
        let base = std::env::temp_dir().join(format!(
            "mode-bash-artifact-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        let data_root = base.join(".mode");
        let context = ToolContext {
            cwd: std::env::temp_dir().to_string_lossy().to_string(),
            artifact: Some(crate::ArtifactLocation {
                data_root: data_root.clone(),
                session_id: "sess_art".to_string(),
                tool_call_id: "call_art".to_string(),
            }),
            tasks: None,
        };
        let outcome = invoke(
            &json!({ "command": "head -c 31000 /dev/zero | tr '\\0' 'x'" }),
            &context,
        )
        .expect("大输出仍是成功结果");

        let expected = data_root
            .join("cli")
            .join("exec")
            .join("sess_art")
            .join("call_art-stdout.log");
        assert!(
            expected.exists(),
            "artifact 应落在 {expected:?}（观察值 <dataRoot>/.mode/cli/exec/<sessionId>/<toolCallId>-stdout.log）"
        );
        assert_eq!(std::fs::read(&expected).expect("读 artifact").len(), 31_000);
        assert!(
            outcome.content.contains(&expected.display().to_string()),
            "包装里应写出真实落点：{}",
            outcome.content
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn background_registers_a_task_and_returns_the_observed_message() {
        let base = std::env::temp_dir().join(format!(
            "mode-bash-bg-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or(0)
        ));
        let data_root = base.join(".mode");
        let registry = std::sync::Arc::new(crate::TaskRegistry::new());
        let context = ToolContext {
            cwd: base.to_string_lossy().to_string(),
            artifact: Some(crate::ArtifactLocation {
                data_root: data_root.clone(),
                session_id: "sess_bg".to_string(),
                tool_call_id: "call_bg".to_string(),
            }),
            tasks: Some(std::sync::Arc::clone(&registry)),
        };

        let outcome = invoke(
            &json!({
                "command": "printf 'bg\\n'; sleep 0.2; printf 'done\\n'",
                "run_in_background": true,
                "description": "Background"
            }),
            &context,
        )
        .expect("后台启动应成功");

        assert_eq!(outcome.budget_strategy, "artifact");
        let expected = data_root
            .join("cli")
            .join("exec")
            .join("sess_bg")
            .join("call_bg-stdout.log");
        assert_eq!(
            outcome.artifact_path.as_deref(),
            Some(expected.display().to_string().as_str())
        );
        assert!(outcome
            .content
            .starts_with("Command running in background with ID: exec_"));
        assert!(outcome.content.contains(&expected.display().to_string()));

        let id = outcome
            .content
            .split("ID: ")
            .nth(1)
            .and_then(|rest| rest.split('.').next())
            .expect("正文里应有 task id")
            .to_string();
        let snapshot = registry
            .snapshot(&id, true, std::time::Duration::from_secs(5))
            .expect("任务应已登记");
        assert_eq!(snapshot.status, crate::TaskStatus::Completed);
        assert_eq!(snapshot.exit_code, Some(0));
        assert_eq!(snapshot.output, "bg\ndone\n");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn input_schema_is_valid_json() {
        let schema = bash_input_schema();
        assert_eq!(schema["required"][0], "command");
        assert_eq!(schema["additionalProperties"], false);
    }
}
