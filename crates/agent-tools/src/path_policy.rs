//! 路径策略：`file_path` 的解析与「空路径」错误——**读工具与写工具共用一份**。
//!
//! 抽出来的理由（Task 3）：`Write` 与 `Read` 的观察值里，空路径给的是**同一条**错误
//! （`Tool path must not be empty` / `TOOL_EXECUTION_FAILED`），相对路径也按**同一个**规则解析。
//! 两份实现必然漂（Ruling 4 的那条理由），所以放一处。
//!
//! 观察出处：`packages/desktop/tests/agent-runtime-oracle/fixtures/read-error-paths.json` 与
//! `write-empty-path.json`；消息里的分隔符形态见 [`resolve`] 的注释。

use std::path::{Path, PathBuf};

use crate::{ToolContext, ToolError};

/// 相对路径按会话工作目录解析（实测成功；工具描述说必须绝对路径，实测不然）。
///
/// 末尾按**组件**重拼一次：观察到的消息里分隔符是原生形态（`Cannot read directory as text file:
/// <cwd>\dir`），而 `PathBuf` 会原样保留输入里的分隔符——脚本给的 `file_path` 是正斜杠形式
/// （夹具录到的入参就是 `<cwd>/dir`），不归一会让消息变成 `<cwd>/dir`，与观察值不符。
pub fn resolve(file_path: &str, ctx: &ToolContext) -> PathBuf {
    let candidate = Path::new(file_path);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        Path::new(&ctx.cwd).join(candidate)
    };
    joined.components().collect()
}

/// 空字符串 `file_path` 的答复（`Read` 与 `Write` 观察值一致）。
///
/// 线上形状：`{"type":"tool_execution_failed","message":"Tool path must not be empty",
/// "code":"TOOL_EXECUTION_FAILED"}`（无 `detail`）。
pub fn empty_path_error() -> ToolError {
    ToolError {
        error_type: "tool_execution_failed",
        message: "Tool path must not be empty".to_string(),
        code: Some("TOOL_EXECUTION_FAILED"),
        detail: None,
    }
}
