//! 工具结果的**共用文案**：`Write` 与 `Edit` 逐字相同的那几条。
//!
//! 抽出来的理由：这些字符串是**对拍契约的一部分**（第一层比的就是它们），两份必然漂。
//! 各错误**码**不同（`Write` 的「未读覆盖」是 `TOOL_EXECUTION_FAILED`/`code=write_file_not_read`，
//! `Edit` 是 `6`/`code=6`），所以错误构造器仍留在各自的模块里，这里只放**逐字相同**的文本。
//!
//! 观察出处：spec §3.3 与 §3.3.1；夹具 `write-*` / `edit-*`。

/// 「本会话没读过这个文件」的文案（`Write` 与 `Edit` 逐字相同，只有 `code`/`detail` 不同）。
pub const NOT_READ_MESSAGE: &str =
    "File has not been read yet. Read it first before writing to it.";

/// 新建成功的正文模板（`{path}` 处填**原样入参**，不是解析后的绝对路径）。
pub const CREATED_TEMPLATE: &str = "File created successfully at: {path} (file state is current in your context — no need to Read it back)";

/// 覆盖 / 替换成功的正文模板（`Write` 的覆盖与 `Edit` 的单处替换**逐字相同**）。
pub const UPDATED_TEMPLATE: &str = "The file {path} has been updated successfully. (file state is current in your context — no need to Read it back)";

/// 入参校验失败的文案（`Write` 与 `Edit` 逐字相同）。
pub const VALIDATION_MESSAGE: &str = "Tool input failed inputSchema validation";

/// 入参校验失败的答复（观察到的形状：有 `code`、**无 `detail`**）。
pub fn validation_error() -> crate::ToolError {
    crate::ToolError {
        error_type: "tool_execution_failed",
        message: VALIDATION_MESSAGE.to_string(),
        code: Some("TOOL_EXECUTION_FAILED"),
        detail: None,
    }
}
