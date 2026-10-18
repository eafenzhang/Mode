//! `Write` 工具（M2 Task 3）。
//!
//! 规格：`docs/specs/tauri-agent-runtime.md` §3.3（Task 3 开工前的黑盒观察）。
//! 纪律：`docs/specs/clean-room-policy.md`——本文件只取材于**黑盒观察**与 spec，不读旧实现。
//!
//! # 观察到的行为（每条都有出处）
//!
//! | 场景 | 结果 | 出处 |
//! | --- | --- | --- |
//! | 新建文件 | 成功；正文 `File created successfully at: {原样入参路径} (file state is current in your context — no need to Read it back)` | 夹具 `write-new` |
//! | 覆盖**本会话未读**的已存在文件 | 失败 `tool_execution_failed` / `File has not been read yet. Read it first before writing to it.` / `code=TOOL_EXECUTION_FAILED` / `detail=code=write_file_not_read` | 夹具 `write-overwrite-unread` |
//! | 覆盖**本会话已读**的已存在文件 | 成功；正文 `The file {原样入参路径} has been updated successfully. (file state is current in your context — no need to Read it back)` | 夹具 `write-overwrite-read` |
//! | 缺必填字段 | 失败 `tool_execution_failed` / `Tool input failed inputSchema validation` / `code=TOOL_EXECUTION_FAILED`（**无 `detail`**） | 夹具 `write-empty-args` / `write-missing-content` |
//! | `file_path` 是空串 | 失败 `Tool path must not be empty`（与 `Read` 同一条，见 [`crate::path_policy`]） | 夹具 `write-empty-path` |
//! | 相对路径 | 写入按 cwd 解析，但**成功文案里回显的是原样入参** | 夹具 `write-relative-path` |
//!
//! # 没观察到、因而**不假装**的
//!
//! 写失败（IO 错误、父目录不存在、目标是目录）的答复形状**没有观察**，所以走
//! [`crate::unimplemented_surface`] 的显式标记，而不是编一个线上不存在的错误类型。
//! 与 `Read` 的图片路径同一条纪律：任何把它当成 oracle 答复的地方都是缺陷。

use std::fs;

use serde_json::Value;

use crate::messages::{validation_error, CREATED_TEMPLATE, NOT_READ_MESSAGE, UPDATED_TEMPLATE};
use crate::path_policy::{empty_path_error, resolve};
use crate::{file_diff, unimplemented_surface, FileState, ToolContext, ToolError, ToolOutcome};

/// `Write` 的模型侧工具描述——从 oracle 的请求体里逐字抄下（探针 `probe-write.mts surface`）。
pub const WRITE_DESCRIPTION: &str = "Writes a file to the local filesystem, overwriting if one exists.\n\nWhen to use: creating a new file, or fully replacing one you've already Read. Overwriting an existing file you haven't Read will fail. For partial changes, use Edit instead.";

/// `Write` 的模型侧 `input_schema`——同样逐字抄自 oracle 的请求体。
pub const WRITE_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "file_path": {
      "type": "string",
      "description": "The absolute path to the file to write (must be absolute, not relative)"
    },
    "content": {
      "type": "string",
      "description": "The content to write to the file"
    }
  },
  "required": [
    "file_path",
    "content"
  ],
  "additionalProperties": false,
  "$schema": "https://json-schema.org/draft/2020-12/schema"
}"#;

// 成功文案与「未读」文案由 `crate::messages` 持有——`Edit` 用的是**逐字相同**的那几条，
// 两份必然漂。这里只留 `Write` 独有的错误码。

/// 解析 [`WRITE_INPUT_SCHEMA`]（同一份常量，供注册表用）。
pub fn write_input_schema() -> Value {
    serde_json::from_str(WRITE_INPUT_SCHEMA).expect("WRITE_INPUT_SCHEMA 是合法 JSON（有单测钉住）")
}

/// 覆盖未读文件的答复——`detail` 是 `code=write_file_not_read`（与 `Edit` 的**不同**，
/// `Edit` 那条观察到的是 `code=6`；两者各自的夹具分别钉住）。
pub fn not_read_error() -> ToolError {
    ToolError {
        error_type: "tool_execution_failed",
        message: NOT_READ_MESSAGE.to_string(),
        code: Some("TOOL_EXECUTION_FAILED"),
        detail: Some("code=write_file_not_read".to_string()),
    }
}

/// 执行一次 `Write`。
///
/// `files` 是会话内的文件记忆：**本会话读过或写过**这个路径，才允许覆盖已存在的文件
/// （观察：`write-overwrite-unread` 失败、`write-overwrite-read` 成功）。
pub fn invoke(
    args: &Value,
    ctx: &ToolContext,
    files: &FileState,
) -> Result<ToolOutcome, ToolError> {
    let file_path = match args.get("file_path").and_then(Value::as_str) {
        None => return Err(validation_error()),
        Some("") => return Err(empty_path_error()),
        Some(value) => value,
    };
    let Some(content) = args.get("content").and_then(Value::as_str) else {
        return Err(validation_error());
    };

    let target = resolve(file_path, ctx);
    let existed = target.exists();
    if existed && !files.knows_path(file_path, ctx) {
        return Err(not_read_error());
    }

    // 覆盖写要在写前留下旧正文，才能产出 `display.file_diff`（观察：新建 Write **没有** display，
    // 覆盖有；见 `file_diff` 模块）。读不出来（非 UTF-8 等）时宁可不给 display，也不编一份。
    let previous = if existed {
        fs::read_to_string(&target).ok()
    } else {
        None
    };

    // 写失败（IO 错误 / 父目录不存在 / 目标是目录）的形状**没有观察**——不假装，
    // 走显式标记让它在对拍里报红（见文件头的说明）。
    if let Err(error) = fs::write(&target, content) {
        return Err(unimplemented_surface(
            "write_io_error",
            format!("{file_path}：{error}"),
        ));
    }

    let template = if existed {
        UPDATED_TEMPLATE
    } else {
        CREATED_TEMPLATE
    };
    let text = template.replace("{path}", file_path);

    let display = previous.and_then(|before| {
        file_diff::file_diff_display(&target.to_string_lossy(), &before, content)
    });
    Ok(match display {
        Some(display) => ToolOutcome::text_with_display(text, display),
        None => ToolOutcome::text(text),
    })
}
