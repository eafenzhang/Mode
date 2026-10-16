//! `Edit` 工具（M2 Task 3 Task B）。
//!
//! 规格：`docs/specs/tauri-agent-runtime.md` §3.3.1（Task B 开工前的黑盒观察）。
//! 纪律：`docs/specs/clean-room-policy.md`——只取材于黑盒观察与 spec，不读旧实现。
//!
//! # 检查顺序（**探出来的，不是推的**）
//!
//! 探针 `probe-edit.mts` 里有一组专门让**两条判据同时成立**的样本，因为每组只触发一条错误时
//! 看不出先后。决定性样本与结论见 §3.3.1 的「先后的决定性样本」。
//!
//! 1. `old_string == new_string` → [`SAME_STRING_MESSAGE`]（`code=1`）
//! 2. 文件不存在：`old_string` 为空 → **新建**（成功）；否则 → [`MISSING_FILE_MESSAGE`]（`code=4`）
//! 3. 文件存在但本会话没读过 → [`NOT_READ_MESSAGE`]（`code=6`）
//! 4. `old_string` 为空且文件存在 → [`FILE_EXISTS_MESSAGE`]（`code=3`）
//! 5. 子串出现 0 次 → [`NOT_FOUND_TEMPLATE`]（`code=8`）
//! 6. 出现 ≥2 次且 `replace_all` 非 true → [`AMBIGUOUS_TEMPLATE`]（`code=9`）
//! 7. 否则成功（按 `replace_all` 决定替换一处或全部；**成功文案只看这个开关**，不看命中次数）
//!
//! # 没观察到、因而**不假装**的
//!
//! 读文件失败（IO 错误 / 目标是目录）的形状没有观察，走 [`crate::unimplemented_surface`]。

use std::fs;

use serde_json::Value;

use crate::messages::{validation_error, NOT_READ_MESSAGE, UPDATED_TEMPLATE};
use crate::path_policy::{empty_path_error, resolve};
use crate::{unimplemented_surface, FileState, ToolContext, ToolError, ToolOutcome};

/// `Edit` 的模型侧工具描述——从 oracle 的请求体里逐字抄下（探针 `probe-write.mts surface`）。
pub const EDIT_DESCRIPTION: &str = "Performs exact string replacement in a file.\n\n- You must Read the file in this conversation before editing, or the call will fail.\n- `old_string` must match the file exactly, including indentation, and be unique — the edit fails otherwise. Strip the Read line prefix (line number + tab) before matching.\n- `replace_all: true` replaces every occurrence instead.";

/// `Edit` 的模型侧 `input_schema`——同样逐字抄自 oracle 的请求体。
pub const EDIT_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "file_path": {
      "type": "string",
      "description": "The absolute path to the file to modify"
    },
    "old_string": {
      "type": "string",
      "description": "The text to replace"
    },
    "new_string": {
      "type": "string",
      "description": "The text to replace it with (must be different from old_string)"
    },
    "replace_all": {
      "type": "boolean",
      "default": false,
      "description": "Replace all occurrences of old_string (default false)"
    }
  },
  "required": [
    "file_path",
    "old_string",
    "new_string"
  ],
  "additionalProperties": false,
  "$schema": "https://json-schema.org/draft/2020-12/schema"
}"#;

/// `old_string == new_string`（判据 1，`code=1`）。
pub const SAME_STRING_MESSAGE: &str =
    "No changes to make: old_string and new_string are exactly the same.";
/// 目标文件不存在且 `old_string` 非空（判据 2，`code=4`）。`{cwd}` 处填会话工作目录。
pub const MISSING_FILE_TEMPLATE: &str =
    "File does not exist. Note: your current working directory is {cwd}.";
/// `old_string` 为空但文件已存在（判据 4，`code=3`）。
pub const FILE_EXISTS_MESSAGE: &str = "Cannot create new file - file already exists.";
/// 子串一次都没出现（判据 5，`code=8`）。`{old}` 处填 `old_string`。
pub const NOT_FOUND_TEMPLATE: &str = "String to replace not found in file. String: {old}";
/// 子串出现多次且没开 `replace_all`（判据 6，`code=9`）。
pub const AMBIGUOUS_TEMPLATE: &str = "Found {count} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, please provide more context to uniquely identify the instance. String: {old}";
/// `replace_all: true` 时的成功文案（**只看开关，不看命中次数**）。
pub const REPLACED_ALL_TEMPLATE: &str = "The file {path} has been updated. All occurrences were successfully replaced. (file state is current in your context — no need to Read it back)";

/// 解析 [`EDIT_INPUT_SCHEMA`]（同一份常量，供注册表用）。
pub fn edit_input_schema() -> Value {
    serde_json::from_str(EDIT_INPUT_SCHEMA).expect("EDIT_INPUT_SCHEMA 是合法 JSON（有单测钉住）")
}

/// `code` / `detail` 都是那个十进制数的错误（观察值：`code:"6"` + `detail:"code=6"`）。
///
/// 这一族在 `Edit` 里出现了六档（1/3/4/6/8/9），形状完全同构，所以用一个构造器而不是写六份；
/// 码取 `&'static str`（取值集合固定），不用 `Box::leak` 之类的把戏。
fn coded_error(code: &'static str, message: String) -> ToolError {
    ToolError {
        error_type: "tool_execution_failed",
        message,
        code: Some(code),
        detail: Some(format!("code={code}")),
    }
}

/// 执行一次 `Edit`。
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
    let Some(old_string) = args.get("old_string").and_then(Value::as_str) else {
        return Err(validation_error());
    };
    let Some(new_string) = args.get("new_string").and_then(Value::as_str) else {
        return Err(validation_error());
    };
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // 判据 1：先看入参本身，与文件状态无关（决定性样本：文件不存在 + old==new → code=1，不是 4）。
    if old_string == new_string {
        return Err(coded_error("1", SAME_STRING_MESSAGE.to_string()));
    }

    let target = resolve(file_path, ctx);
    if !target.exists() {
        // 判据 2'：空 `old_string` 在文件不存在时 = 新建（观察：真的建出来了）。
        // 文案是 **UPDATED 那条**，不是 `Write` 的「File created successfully at: …」——
        // 夹具 `edit-empty-old-creates` 录到的就是前者，别照 `Write` 抄。
        if old_string.is_empty() {
            if let Err(error) = fs::write(&target, new_string) {
                return Err(unimplemented_surface(
                    "edit_io_error",
                    format!("{file_path}：{error}"),
                ));
            }
            return Ok(ToolOutcome::text(
                UPDATED_TEMPLATE.replace("{path}", file_path),
            ));
        }
        // 判据 2：决定性样本是「文件不存在 + 未读」→ code=4（不是 6），所以存在性**先于**未读检查。
        return Err(coded_error(
            "4",
            MISSING_FILE_TEMPLATE.replace("{cwd}", &ctx.cwd),
        ));
    }

    // 判据 3：决定性样本是「文件存在 + 未读 + old==new」→ code=1（已在上面返回），
    // 「未读 + 串不存在」→ code=6（不是 8），所以未读检查先于匹配。
    if !files.knows_path(file_path, ctx) {
        return Err(coded_error("6", NOT_READ_MESSAGE.to_string()));
    }

    // 判据 4：空 `old_string` 在文件存在时是「新建」语义，而文件已经在 → 报这条。
    if old_string.is_empty() {
        return Err(coded_error("3", FILE_EXISTS_MESSAGE.to_string()));
    }

    let Ok(content) = fs::read_to_string(&target) else {
        return Err(unimplemented_surface(
            "edit_read_error",
            format!("{file_path}：不是可读的 UTF-8 文本"),
        ));
    };
    let matches = content.matches(old_string).count();
    if matches == 0 {
        return Err(coded_error(
            "8",
            NOT_FOUND_TEMPLATE.replace("{old}", old_string),
        ));
    }
    // 匹配口径是**纯子串**：`old_string: "indented"` 在 `"  indented\nnot indented\n"` 上算 2 次。
    if matches > 1 && !replace_all {
        return Err(coded_error(
            "9",
            AMBIGUOUS_TEMPLATE
                .replace("{count}", &matches.to_string())
                .replace("{old}", old_string),
        ));
    }

    let updated = if replace_all {
        content.replace(old_string, new_string)
    } else {
        content.replacen(old_string, new_string, 1)
    };
    if let Err(error) = fs::write(&target, updated) {
        return Err(unimplemented_surface(
            "edit_io_error",
            format!("{file_path}：{error}"),
        ));
    }
    // 成功文案**只看 `replace_all` 这个开关**，不看实际命中几次（观察：只命中 1 次时仍是
    // 「All occurrences were successfully replaced.」）。
    let template = if replace_all {
        REPLACED_ALL_TEMPLATE
    } else {
        UPDATED_TEMPLATE
    };
    Ok(ToolOutcome::text(template.replace("{path}", file_path)))
}
