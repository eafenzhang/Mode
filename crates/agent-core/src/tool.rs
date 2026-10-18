//! 工具调用入口：注册表投影 + 结果/错误到事件载荷的投影。
//!
//! 本模块**不含**任何工具语义——语义在 `crates/agent-tools`（那里每条规则都标了观察出处）。
//! 这里只负责两件事：
//!
//! 1. 给出 spec 要求的调用入口 [`invoke`]（`ToolSpec` + 入参 + 上下文 → 结果或错误）；
//! 2. 把结果/错误投影成 `tool.updated` 载荷里**实际观察到**的那两个对象
//!    （`result` / `error`），供 [`crate::session`] 发事件用。
//!
//! 观察出处（行为夹具 `read-single` / `read-absent-search-tools` 的 `expect.events`）：
//!
//! ```text
//! result: { success: true, content, perf: { totalMs }, truncated, originalBytes, returnedBytes, budgetStrategy }
//! error:  { type, message, code, stack? , detail? }
//! ```

use serde_json::{json, Value};

use agent_protocol::session_event::{ProtocolErrorDetail, ToolResultObject};
use agent_protocol::ExtraFields;

pub use agent_tools::{
    invoke_by_name, lookup, registry, FileState, ModeToolFamily, ToolContext, ToolError,
    ToolOutcome, ToolSpec,
};

/// 执行一次工具调用。
///
/// `Ok` → `tool.updated` 的 `kind:"result"`；`Err` → `kind:"error"`。
/// 未注册的工具名会得到与 oracle 同形的 `tool_not_found`（见 `agent_tools` 的 crate 文档）。
///
/// `files` 是会话内的文件记忆：`Write` 的「先读再写」前置条件要读它（见 `agent_tools::FileState`）。
pub fn invoke(
    spec: &ToolSpec,
    args: &Value,
    ctx: &ToolContext,
    files: &FileState,
) -> Result<ToolOutcome, ToolError> {
    invoke_by_name(spec.name, args, ctx, files)
}

/// `ToolOutcome` → `kind:"result"` 的 `result` 对象。
///
/// `perf.totalMs` 与 `duration` 都是计时字段（spec §3 的时间白名单），由调用方测量后传进来；
/// 其余字段逐条对应观察到的形状。
pub fn result_object(outcome: &ToolOutcome, elapsed_ms: u64) -> ToolResultObject {
    let mut extra = ExtraFields::new();
    extra.insert("perf".to_string(), json!({ "totalMs": elapsed_ms }));
    extra.insert("truncated".to_string(), Value::Bool(outcome.truncated));
    extra.insert("originalBytes".to_string(), json!(outcome.original_bytes));
    extra.insert("returnedBytes".to_string(), json!(outcome.returned_bytes));
    extra.insert("budgetStrategy".to_string(), json!(outcome.budget_strategy));
    // 观察（spec §3.4）：后台 Bash 的结果多一个 `artifactPath`；TaskOutput / TaskStop
    // 多一个 `display` 对象。前台的普通结果没有这两个键。
    if let Some(path) = &outcome.artifact_path {
        extra.insert("artifactPath".to_string(), json!(path));
    }
    if let Some(display) = &outcome.display_json {
        let value = serde_json::from_str(display).unwrap_or_else(|_| json!(display));
        extra.insert("display".to_string(), value);
    }
    ToolResultObject {
        success: Some(true),
        content: Some(Value::String(outcome.content.clone())),
        extra,
    }
}

/// `ToolError` → `kind:"error"` 的 `error` 对象。
///
/// `stack` 恒为 `None`：那是 JS 调用栈，Rust 侧没有对应物。它**不在** spec §3 的白名单里，
/// 所以只读切片的第三层对拍暂时不能比 `stack`（Task 5/7 处理；本任务只比第一、二层）。
pub fn error_object(error: &ToolError) -> ProtocolErrorDetail {
    ProtocolErrorDetail {
        error_type: error.error_type.to_string(),
        message: error.message.clone(),
        stack: None,
        code: error.code.map(str::to_string),
        detail: error.detail.clone(),
        underlying_error_message: None,
        underlying_error_detail: None,
        attribution: None,
        retryable: None,
        data: None,
        extra: ExtraFields::new(),
    }
}
