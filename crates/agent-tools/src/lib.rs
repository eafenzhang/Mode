//! `agent-tools`：M2 的**只读工具切片**与工具注册表。
//!
//! 规格：`docs/specs/tauri-agent-runtime.md`（oracle 契约、三层对拍口径、合法差异白名单）。
//! 纪律：`docs/specs/clean-room-policy.md`——本 crate 只取材于**允许输入**（协议与类型的声明面、
//! 测试断言、`docs/specs/**`）与**黑盒观察**（跑旧实现、看它的输入输出），不读旧运行时的实现体。
//!
//! # 注册表里有什么，以及为什么只有这些
//!
//! 阶段计划把只读切片写成 `Read` / `Glob` / `Grep`。**实测不是这样**：那三个名字是从
//! `packages/shared/src/tool-identity.ts` 的 `MODE_KNOWN_TOOL_NAMES` 推出来的，而那张表是
//! **渲染用**的名字表（UI 要渲染历史会话里的工具行），不是运行时的注册表。
//!
//! 黑盒观察（2026-10-16，探针 `packages/desktop/tests/agent-runtime-oracle/probe-tool-surface.mts`
//! 的 `tools` / `absence` 两组）：
//!
//! 1. 运行时对外声明的工具清单是固定的 21 条，其中只读切片里**只有 `Read`**；
//!    `Glob` / `Grep` 都不在里面。
//! 2. 脚本化调用 `Glob` / `Grep` 得到
//!    `{"type":"tool_not_found","message":"Tool not found: Glob","code":"TOOL_NOT_FOUND"}`——
//!    即它们**没有注册**，不是「注册了但没对外声明」。
//! 3. 那 21 条不随 `--surface desktop`、`--mode build|plan`、`--disallowed-tools`、
//!    `nativeSearchEnhancementsEnabled` 变化（正对照：`--disallowed-tools Bash` 下 `Bash`
//!    照样声明）。该偏好的语义由协议声明给出——「新建或冷恢复 Session 是否为 Bash 注入
//!    bfs/ugrep 增强」，它改的是 Bash 的注入，不是工具注册表。
//!
//! 所以本 crate 的 `registry()` 只注册 `Read`；`Glob` / `Grep` 的正确 Rust 行为是
//! **未注册**（调用方按 [`ToolError::not_found`] 答复，形状与观察到的 `tool_not_found` 一致）。
//! 录制的证据在行为夹具 `read-absent-search-tools.json` 里：两条调用、两条 `Tool not found: …`。
//!
//! 这条结论与阶段计划的写法冲突，**不自行改计划**：已按纪律上报（见本任务报告），
//! 由 ledger 裁决是否把只读切片改成 `Read`（或改从哪个协议面取 `Glob`/`Grep` 的行为）。
//!
//! # 与 spec §3 的关系
//!
//! 工具结果的**比较口径**在 spec §3 与 TS 侧 `normalize.mts` 里，本 crate 不实现第二份比较器、
//! 也不扩大白名单：它只产出**线上形状**的工具结果/错误，由两端各自投影后精确相等比较。

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod path_policy;
pub mod read;
/// 测试支撑（投影助手）。放在库里是为了让 `agent-core` 的对拍用例复用**同一份**投影，
/// 而不是各写一份（两份投影必然漂，正是 Ruling 4 要避免的）。见模块注释。
#[doc(hidden)]
pub mod test_support;
pub mod write;

pub use read::{
    estimated_tokens, FileState, READ_DESCRIPTION, READ_INPUT_SCHEMA, TOKEN_LIMIT, WASTED_CALL,
};
pub use write::{write_input_schema, WRITE_DESCRIPTION, WRITE_INPUT_SCHEMA};

/// 工具分档（`packages/shared/src/tool-identity.ts` 的 `ModeToolFamily` 声明面，逐字对齐）。
///
/// 权限引擎（Task 6）按 family 分档比按工具名分档更稳，所以这一档现在就按声明面落下来；
/// 取值用 kebab-case 与 TS 的字面量一致（`file-read` / `ask-user-question` / `node-repl` …）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModeToolFamily {
    FileRead,
    FileWrite,
    Shell,
    Search,
    Todo,
    AskUserQuestion,
    Agent,
    Skill,
    Goal,
    SessionContext,
    Message,
    TaskControl,
    NodeRepl,
    Workflow,
}

/// 一个工具对模型侧的声明（名字 / 分档 / 入参 schema / 是否只读）。
///
/// `schema` 是运行时**实际发出去**的 `input_schema`——`Read` 那份是从 oracle 的请求体里
/// 逐字抄下来的（见 [`read::READ_INPUT_SCHEMA`]），不是照 JSON Schema 规范重写一遍。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub family: ModeToolFamily,
    pub schema: Value,
    pub read_only: bool,
}

/// 工具执行上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolContext {
    /// 会话工作目录：相对 `file_path` 按它解析（实测：工具描述说必须绝对路径，实际不然）。
    pub cwd: String,
}

/// 一次成功的工具结果（`tool.updated/kind:"result"` 的 `result` 对象，取对拍要用的字段）。
///
/// 字段是**线上形状**：`success` 恒为 true（失败走 [`ToolError`]），`content` 是正文，
/// `originalBytes` / `returnedBytes` 在全部观察到的用例里都等于正文的 **UTF-8 字节数**
/// （实测：含中文的三行结果 `originalBytes: 23` = 23 字节，而 JS 侧 `.length` 是 17）。
/// `perf.totalMs` 是计时（spec §3 的时间白名单里），不与对拍契约相关，故不建模。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    /// `result.content`：模型看到的正文。
    pub content: String,
    /// `result.truncated`。在本 crate 实现的路径里恒为 `false`（观察如此）。
    pub truncated: bool,
    /// `result.originalBytes`。
    pub original_bytes: u64,
    /// `result.returnedBytes`。
    pub returned_bytes: u64,
    /// `result.budgetStrategy`（观察到恒为 `"truncate"`）。
    pub budget_strategy: &'static str,
}

impl ToolOutcome {
    /// 按观察到的形状构造一个成功结果：`truncated:false`、`budgetStrategy:"truncate"`、
    /// 两个字节计数都取正文的 UTF-8 字节数。
    pub fn text(content: impl Into<String>) -> Self {
        let content = content.into();
        let bytes = content.len() as u64;
        Self {
            content,
            truncated: false,
            original_bytes: bytes,
            returned_bytes: bytes,
            budget_strategy: "truncate",
        }
    }
}

/// 一次工具失败（`tool.updated/kind:"error"` 的 `error` 对象，取对拍要用的字段）。
///
/// 一个结构体而不是枚举，因为线上的判别键就是 `type` 这个**字符串**，而它的取值集合本身是
/// 观察出来的（`tool_execution_failed` / `FileSystemPortError` / `ZodError`），不是一个封闭枚举。
/// `code` / `detail` 的取值同样逐条来自观察（见 `read.rs` 的注释）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    /// 线上的 `error.type`。
    pub error_type: &'static str,
    /// 线上的 `error.message`——**对拍第一层的「工具输出」取的就是它**（spec §2.1）。
    pub message: String,
    /// 线上的 `error.code`（观察到的分支都有，唯 `ZodError` 没有）。
    pub code: Option<&'static str>,
    /// 线上的 `error.detail`。
    pub detail: Option<String>,
}

/// 未注册的工具名——形状逐字对齐观察到的 `tool_not_found`。
///
/// 观察（2026-10-16）：脚本化调用未注册的 `Glob` / `Grep` 时，运行时的答复是
/// `{"type":"tool_not_found","message":"Tool not found: Glob","code":"TOOL_NOT_FOUND"}`（无 `detail`）。
pub fn not_found(tool_name: &str) -> ToolError {
    ToolError {
        error_type: "tool_not_found",
        message: format!("Tool not found: {tool_name}"),
        code: Some("TOOL_NOT_FOUND"),
        detail: None,
    }
}

/// 本 crate 刻意**没有**实现、因而绝不能与 oracle 结果混同的面的标记。
///
/// 用途：`Read` 的两条路径在 M2 里无法忠实实现（见 `read.rs` 的文件注释），返回这个 `type`。
/// 它**不是**线上取值，所以任何一个把它当成 oracle 答复的地方都是缺陷——`agent-core` 的
/// 对拍用例显式断言「实测结果里不出现它」，把这件事从「靠人记得」变成「会红」。
pub const UNIMPLEMENTED_ERROR_TYPE: &str = "m2_unimplemented_surface";

/// 构造一个「M2 未实现的面」结果。
pub fn unimplemented_surface(surface: &'static str, message: impl Into<String>) -> ToolError {
    ToolError {
        error_type: UNIMPLEMENTED_ERROR_TYPE,
        message: message.into(),
        code: None,
        detail: Some(format!("surface={surface}")),
    }
}

/// M2 的工具注册表。
///
/// `Read` 与 `Write`：两者都由黑盒观察确认**已注册**（`Read` 见 crate 文档；`Write` 见
/// `docs/specs/tauri-agent-runtime.md` §3.3 与夹具 `write-empty-args` 的正对照）。
/// 计划里同列的 `ApplyPatch` **未声明也未注册**（调用得 `tool_not_found`），故不在表里。
pub fn registry() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "Read",
            family: ModeToolFamily::FileRead,
            schema: read_input_schema(),
            read_only: true,
        },
        ToolSpec {
            name: "Write",
            family: ModeToolFamily::FileWrite,
            schema: write_input_schema(),
            read_only: false,
        },
    ]
}

/// 解析 [`READ_INPUT_SCHEMA`]（那条从 oracle 请求体里逐字抄下来的声明）。
///
/// 常量本身已由单元测试钉住「能解析且形状与声明一致」，所以这里的 `expect` 不会在生产路径上
/// 被触发；不用「解析失败就回 `Null`」的兜底——那会把一个常量写错伪装成「工具没有 schema」。
pub fn read_input_schema() -> Value {
    serde_json::from_str(READ_INPUT_SCHEMA).expect("READ_INPUT_SCHEMA 是合法 JSON（有单测钉住）")
}

/// 按名字取工具声明。
pub fn lookup(tool_name: &str) -> Option<ToolSpec> {
    registry().into_iter().find(|spec| spec.name == tool_name)
}

/// 按名字执行一次工具调用；未注册的名字得到 [`not_found`]（与 oracle 的答复同形）。
///
/// `files` 是会话内的文件记忆（[`FileState`]）——`Write` 的「先读再写」前置条件要读它，
/// `Read` 不用（它的去重由会话在调用前后处理，见 [`FileState`] 的注释）。
pub fn invoke_by_name(
    tool_name: &str,
    args: &Value,
    ctx: &ToolContext,
    files: &FileState,
) -> Result<ToolOutcome, ToolError> {
    match tool_name {
        "Read" => read::invoke(args, ctx),
        "Write" => write::invoke(args, ctx, files),
        other => Err(not_found(other)),
    }
}
