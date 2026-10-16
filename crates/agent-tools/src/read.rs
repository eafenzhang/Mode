//! `Read`：读一个文本文件并按 `cat -n` 形式带行号返回。
//!
//! **每一条语义都来自黑盒观察**（2026-10-16，探针
//! `packages/desktop/tests/agent-runtime-oracle/probe-read-only.mts` 的 `read` / `trunc` /
//! `limit-edge` 三组；行为夹具 `fixtures/read-offset-edges.json`、`read-error-paths.json`
//! 把其中最要紧的那些逐字钉住）。本文件不读旧实现的实现体。
//!
//! # 观察到的规则（逐条给出处）
//!
//! ## 1. 声明面
//!
//! `READ_DESCRIPTION` / `READ_INPUT_SCHEMA` 是从 oracle 的模型请求体里**逐字抄下来**的
//! （探针 `tools` 组打印的那条 `Read` 声明）。两条实测与声明不符，照实记录、**按实测实现**：
//!
//! - 声明说 `additionalProperties: false`，实测多一个未知键（`nope: 1`）**照样成功**
//!   （探针 `read` #14；夹具 `read-offset-edges` 里没有它，见下条说明）。
//! - 声明说 `file_path` 必须是绝对路径，实测相对路径按会话工作目录解析并成功
//!   （探针 `read` #13；夹具 `read-offset-edges` 的工具调用 #5 就是这条）。
//! - 声明说「Reads up to 2000 lines by default」，实测 2500 行的文件一次读到 2500 行
//!   （探针 `trunc` #1），即**没有** 2000 行默认上限——默认上限是 token 预算。
//!
//! ## 2. 分行、行尾 `\r`、行号
//!
//! - 按 `\n` 切段，**保留**末尾空段：`"u\n"` → 两行（`"1\tu\n2\t"`）；`""` → 一行（空）。
//! - 每段**去掉一个行尾 `\r`**，行内 `\r` 保留：
//!   `"x\r\ny\r\n"` → `"1\tx\n2\ty\n3\t"`；`"a\rb\nc\n"` → `"1\ta\rb\n2\tc\n3\t"`。
//! - 返回正文 = 各行 `"{number}\t{text}"` 用 `\n` 连接；
//!   `number = offset + 返回序号`（`offset` 缺省为 1）。所以 `offset: 1` 从 1 编号，
//!   而 `offset: 0` 会从 **0** 开始编号、且从第 1 行开始取（起点 = `max(offset-1, 0)`）——
//!   `offset: 0` 的这条怪癖是实测（探针 `read` #4：`"1\n2\n3\n"` → `"0\t1\n1\t2\n2\t3\n3\t"`）。
//! - `limit` 是**行数**不是结束行号（`offset: 2, limit: 1` → 只回第 2 行）。
//!
//! ## 3. `offset` 越过最后一行：**成功**，正文是一条 system-reminder
//!
//! 判据是 `offset >= 行数`（行数含末尾空段：`"1\n2\n3\n"` 是 4 行、`""` 是 1 行）：
//!
//! ```text
//! <system-reminder>Warning: the file exists but is shorter than the provided offset ({offset}). The file has {行数} lines.</system-reminder>
//! ```
//!
//! 实测三点：`offset: 3`（4 行）正常返回；`offset: 4`（4 行）走警告；空文件缺省 `offset: 1`
//! 也走警告（「The file has 1 lines」）。
//!
//! ## 4. 截断：token 预算，估算 = `ceil(字节 / 3)`
//!
//! 估算口径由三个点钉死（探针 `trunc`）：文件 74999 字节 → 全文；75000 字节 → 全文；
//! 75001 字节 → 25001 tokens 超限。即 **`ceil(字节数 / 3)`**，上限 **25000 tokens**，
//! 触发条件是**严格大于**。整数除法下 `floor` 会与 75001 这一点矛盾，所以是上取整。
//!
//! 上限作用在**被选中的范围**上，不是整个文件——三种入参各有实测：
//!
//! | 入参 | 选中范围字节 | 实测 |
//! | --- | --- | --- |
//! | `Read` 2500×60（152499 B） | 152499 → 50833 tokens | 成功，但**返回部分视图**（带提醒头） |
//! | 同上 `offset: 1, limit: 2000` | 121999 → 40667 tokens | **错误** `read_output_too_many_tokens` |
//! | 同上 `offset: 2` | 152438 → 50813 tokens | **错误** 同上（所以「显式」的判据是 `offset` 或 `limit` 任一出现） |
//! | 同上 `limit: 3000`（夹到 2500 行） | 152499 → 50833 tokens | **错误** |
//! | 同上 `limit: 1000` | 60999 → 20333 tokens | 成功，整段返回 |
//! | 304999 B 文件 `limit: 5000` | 304999 → 101667 tokens | **错误**（token 错误，不是大小错误） |
//! | 2500×120（302499 B）`limit: 2000` | 241999 → 80667 tokens | **错误**（同上） |
//!
//! 「选中范围字节」的口径也由实测确定：是**源文本**里这些行用 `\n` 连接的字节数
//! （`60×2000 + 1999 = 121999`），不是带 `"{n}\t"` 前缀后的长度（那会是 125999 → 42000）。
//!
//! ## 5. 文件大小上限 256KB，只作用于**没有** `offset`/`limit` 的路径
//!
//! `Read` 302499 B（2500×120）→ 错误
//! `File content (295.4KB) exceeds maximum allowed size (256KB). …`（295.4 = 302499/1024，一位小数）。
//! 而 304999 B 的文件加 `limit: 5000` → **不是**大小错误，是 token 错误（见上表）。
//! 所以显式给范围时不做大小检查。
//!
//! ## 6. 错误形状（`error.type` / `code` / `detail` 都是观察值）
//!
//! | 触发 | `type` | `code` | 说明 |
//! | --- | --- | --- | --- |
//! | 文件不存在 | `tool_execution_failed` | `TOOL_EXECUTION_FAILED` | `message` = `File does not exist. Note: your current working directory is {cwd}.`；`detail` 里带 `read_file_not_found` |
//! | 目标是目录 | `FileSystemPortError` | `is_directory` | `message` = `Cannot read directory as text file: {path}` |
//! | 扩展名 `.bin` | `tool_execution_failed` | `TOOL_EXECUTION_FAILED` | `message` 是 `<tool_use_error>…binary {ext} file…</tool_use_error>`；`detail` 带 `read_input_preflight_failed`。**与文件内容无关**（那个 `.bin` 里其实是普通文本） |
//! | 含 NUL 字节 | `FileSystemPortError` | `unsupported` | `message` = `Unsupported or binary text encoding: {path}`（`.txt` 扩展名也照样触发） |
//! | `limit <= 0` | `ZodError` | （无） | `message` 是序列化后的 zod issue 数组，逐字见 [`LIMIT_TOO_SMALL_MESSAGE`] |
//! | `offset` 非整数 / `offset < 0` / `limit` 非整数 | `tool_execution_failed` | `TOOL_EXECUTION_FAILED` | `message` = `Tool input failed inputSchema validation` |
//! | `file_path` 为空串 | `tool_execution_failed` | `TOOL_EXECUTION_FAILED` | `message` = `Tool path must not be empty` |
//! | 选中范围超出 token 预算 | `tool_execution_failed` | `TOOL_EXECUTION_FAILED` | `detail` 带 `read_output_too_many_tokens` |
//! | 文件超出 256KB（隐式路径） | `tool_execution_failed` | `TOOL_EXECUTION_FAILED` | `detail` 带 `read_file_too_large` |
//!
//! 校验顺序在观察里没有两条同时触发的样本，故本实现的顺序是**选定**的（先 JSON Schema 的
//! 类型/范围闸门、再 zod 精化、最后路径非空），并在下面的注释里逐处标出。
//!
//! # 本实现**刻意不做**的两件事（不是遗漏，是 M2 无法忠实实现）
//!
//! 1. **隐式路径的部分视图行数**：文件超 token 预算又没有 `offset`/`limit` 时，运行时返回
//!    一条提醒头 + 前若干行（2500×60 的文件给的是「lines 1-1045 of 2500」）。这条提醒头的
//!    **文案**是逐字观察到的（[`PARTIAL_VIEW_HEADER_TEMPLATE`]），但**截取多少行**的规则
//!    只从一个样本反推不出来（不是按字节预算：1045 行格式化后 66818 字节 + 头 267 字节，
//!    远小于 25000×3；也不是按行数比例：1045/2500 与 25000/50833 不等）。按纪律**不猜**：
//!    这条路径返回 [`crate::unimplemented_surface`]，它的 `error.type` 是 M2 内部标记，
//!    不可能被误当成 oracle 答复（`agent-core` 的对拍用例显式断言它不出现）。
//! 2. **图片/视频扩展名**：实测扩展名 `.PNG`（大小写不敏感）会被导到图片读取路径
//!    （内容不是真图片时答复 `Unable to decode image data` / `read_image_processing_failed`）。
//!    图片与视频面在 M2 范围之外（要模态内容与解码），本实现对**观察到的** `png`
//!    返回 [`crate::unimplemented_surface`]；其余扩展名集合没有观察，不猜。
//!    **残留风险**：`.jpg` 等未观察到的图片扩展名会被当文本处理。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{unimplemented_surface, ToolContext, ToolError, ToolOutcome};

/// `Read` 的模型侧工具描述——从 oracle 的请求体里逐字抄下（探针 `tools` 组）。
pub const READ_DESCRIPTION: &str = "Reads a file from the local filesystem.\n\n- `file_path` must be an absolute path.\n- Reads up to 2000 lines by default.\n- You can optionally specify a line offset and limit (especially handy for long files), but it's recommended to read the whole file by not providing these parameters\n- Results are returned using cat -n format, with line numbers starting at 1\n- Reads images (PNG, JPG, …) and presents them visually.\n- Reads videos (MP4, MOV, WEBM, …) and presents them as video input (subject to Mode's video input limit).\n- Reading a directory, a missing file, or an empty file returns an error or system reminder rather than content.\n- Do NOT re-read a file you just edited to verify — Edit/Write would have errored if the change failed, and the harness tracks file state for you.";

/// `Read` 的 `input_schema`——从 oracle 的请求体里逐字抄下（探针 `tools` 组）。
///
/// 与实测不符的三处（`additionalProperties`、绝对路径、2000 行默认上限）见文件头第 1 节：
/// 保留声明原样（它是模型真正看到的东西），行为按实测实现。
pub const READ_INPUT_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "file_path": {
      "type": "string",
      "description": "The absolute path to the file to read"
    },
    "offset": {
      "type": "integer",
      "minimum": 0,
      "maximum": 9007199254740991,
      "description": "The line number to start reading from. Only provide if the file is too large to read at once"
    },
    "limit": {
      "type": "integer",
      "exclusiveMinimum": 0,
      "maximum": 9007199254740991,
      "description": "The number of lines to read. Only provide if the file is too large to read at once."
    }
  },
  "required": [
    "file_path"
  ],
  "additionalProperties": false,
  "$schema": "https://json-schema.org/draft/2020-12/schema"
}"#;

/// token 预算（观察值：提醒头里的 `limit 25000`，以及 75000/75001 字节这两个边界）。
pub const TOKEN_LIMIT: u64 = 25_000;

/// 隐式路径的文件大小上限：256KB = 262144 字节（`error.message` 里写的 `maximum allowed size (256KB)`）。
///
/// 边界本身（`>` 还是 `>=`）没有样本落在 262144 上，取 `>`：实测里 302499 触发、152499 不触发。
pub const FILE_SIZE_LIMIT_BYTES: u64 = 262_144;

/// 观察值：`offset` 越界时那条 system-reminder 的模板。
pub const OFFSET_WARNING_TEMPLATE: &str = "<system-reminder>Warning: the file exists but is shorter than the provided offset ({offset}). The file has {lines} lines.</system-reminder>";

/// 观察值：超 token 预算时的错误正文（`detail` 里带 `read_output_too_many_tokens`）。
pub const TOO_MANY_TOKENS_TEMPLATE: &str = "File content ({tokens} tokens) exceeds maximum allowed tokens (25000). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.";

/// 观察值：隐式路径超 256KB 时的错误正文（`detail` 里带 `read_file_too_large`）。
pub const TOO_LARGE_TEMPLATE: &str = "File content ({kb}KB) exceeds maximum allowed size (256KB). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.";

/// 观察值：隐式路径的部分视图提醒头模板（**本实现不产出它**，见文件头「刻意不做」第 1 条）。
///
/// 留着是为了让下一个人一眼看到「文案已经录到手了，缺的只是行数规则」。
pub const PARTIAL_VIEW_HEADER_TEMPLATE: &str = "<system-reminder>The file is too large to display in full ({tokens} estimated tokens, limit 25000). Showing a partial view of lines 1-{shown} of {total}. Use Read with offset {next} and limit 2000 to continue, or use a search tool to find a specific section.</system-reminder>";

/// 观察值：`limit <= 0` 时 `error.message` 的全部内容（zod 的序列化 issue 数组，逐字）。
pub const LIMIT_TOO_SMALL_MESSAGE: &str = r#"[ { "code": "too_small", "minimum": 0, "type": "number", "inclusive": false, "exact": false, "message": "Number must be greater than 0", "path": [ "limit" ] } ]"#;

/// 观察到的二进制扩展名（探针 `read` #10 / #12：`.bin` 不论内容都被读前预检拒绝）。
///
/// 这只是**观察到的**那一档；完整表没有观察，不猜（见文件头「刻意不做」第 2 条）。
const BINARY_EXTENSIONS: &[&str] = &["bin"];

/// 观察到的图片扩展名（探针 `limit-edge` #4：`.PNG` 大小写不敏感地走图片路径）。
const IMAGE_EXTENSIONS: &[&str] = &["png"];

/// token 估算：`ceil(字节 / 3)`（三个边界点实测，见文件头第 4 节）。
pub fn estimated_tokens(bytes: u64) -> u64 {
    bytes.div_ceil(3)
}

/// 执行一次 `Read`。
pub fn invoke(args: &Value, ctx: &ToolContext) -> Result<ToolOutcome, ToolError> {
    let file_path = validate(args)?;
    let absolute = resolve(&file_path, ctx);

    let metadata = match fs::metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(file_not_found(&absolute, ctx));
        }
        Err(error) => {
            // 未观察到的 stat 失败（权限等）：fail loud，不编一个 oracle 形状出来。
            return Err(unimplemented_surface(
                "read_stat_error",
                format!("stat {} failed: {error}", absolute.display()),
            ));
        }
    };
    if metadata.is_dir() {
        return Err(ToolError {
            error_type: "FileSystemPortError",
            message: format!("Cannot read directory as text file: {}", absolute.display()),
            code: Some("is_directory"),
            detail: None,
        });
    }

    match extension_of(&absolute).as_deref() {
        Some(extension) if BINARY_EXTENSIONS.contains(&extension) => {
            let message = binary_preflight_message(extension);
            return Err(ToolError {
                error_type: "tool_execution_failed",
                message: message.clone(),
                code: Some("TOOL_EXECUTION_FAILED"),
                detail: Some(format!(
                    "code=read_input_preflight_failed\n[ {{ \"code\": \"custom\", \"path\": [ \"file_path\" ], \"message\": \"{}\" }} ]",
                    message.trim_start_matches("<tool_use_error>").trim_end_matches("</tool_use_error>")
                )),
            });
        }
        Some(extension) if IMAGE_EXTENSIONS.contains(&extension) => {
            return Err(unimplemented_surface(
                "read_image",
                format!("图片扩展名 .{extension} 属 M2 范围外（模态内容），见 read.rs 文件头"),
            ));
        }
        _ => {}
    }

    let bytes = match fs::read(&absolute) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Err(unimplemented_surface(
                "read_io_error",
                format!("read {} failed: {error}", absolute.display()),
            ));
        }
    };
    let content = match std::str::from_utf8(&bytes) {
        // NUL 字节实测触发；非法 UTF-8 没有样本，按同一条处理（都是「不是可读文本」）。
        Ok(text) if !bytes.contains(&0) => text,
        _ => {
            return Err(ToolError {
                error_type: "FileSystemPortError",
                message: format!(
                    "Unsupported or binary text encoding: {}",
                    absolute.display()
                ),
                code: Some("unsupported"),
                detail: None,
            });
        }
    };

    select_and_format(content, args, &bytes)
}

/// 入参校验。
///
/// 顺序（观察里没有两条同时触发的样本，属**选定**顺序）：JSON Schema 的类型/范围闸门 →
/// `limit > 0` 的 zod 精化 → `file_path` 非空。三处观察分别是
/// `offset: "2"`→schema 闸门、`limit: 0`→zod、`file_path: ""`→路径校验。
fn validate(args: &Value) -> Result<String, ToolError> {
    let file_path = match args.get("file_path") {
        // 缺 `file_path`：声明的 `required` 是唯一依据（没有样本）。
        None => return Err(input_schema_failure()),
        Some(Value::String(text)) => text.clone(),
        Some(_) => return Err(input_schema_failure()),
    };
    if let Some(offset) = integer_field(args, "offset")? {
        if offset < 0 {
            return Err(input_schema_failure());
        }
    }
    if let Some(limit) = integer_field(args, "limit")? {
        if limit <= 0 {
            return Err(ToolError {
                error_type: "ZodError",
                message: LIMIT_TOO_SMALL_MESSAGE.to_string(),
                code: None,
                detail: None,
            });
        }
    }
    if file_path.is_empty() {
        return Err(ToolError {
            error_type: "tool_execution_failed",
            message: "Tool path must not be empty".to_string(),
            code: Some("TOOL_EXECUTION_FAILED"),
            detail: None,
        });
    }
    Ok(file_path)
}

/// 取一个整数字段。缺失 → `None`；类型不符 → 走 schema 闸门的错误。
///
/// 注意：`2.0` 这类**整数值的浮点**按 JSON Schema draft 2020-12 算 `integer`，所以接受
/// （`1.5` 才拒）。实测只有 `"2"`（字符串）与 `1.5` 两个拒绝样本。
fn integer_field(args: &Value, key: &str) -> Result<Option<i64>, ToolError> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    if let Some(number) = value.as_i64() {
        return Ok(Some(number));
    }
    if let Some(number) = value.as_f64().and_then(integral_f64_to_i64) {
        return Ok(Some(number));
    }
    Err(input_schema_failure())
}

/// 整数值的浮点（`2.0`）当整数看；`1.5` 不是。
#[allow(clippy::cast_possible_truncation)]
fn integral_f64_to_i64(value: f64) -> Option<i64> {
    if value.fract() == 0.0 && value >= i64::MIN as f64 && value <= i64::MAX as f64 {
        Some(value as i64)
    } else {
        None
    }
}

/// 观察到的 error：`Tool input failed inputSchema validation`。
fn input_schema_failure() -> ToolError {
    ToolError {
        error_type: "tool_execution_failed",
        message: "Tool input failed inputSchema validation".to_string(),
        code: Some("TOOL_EXECUTION_FAILED"),
        detail: None,
    }
}

/// 观察到的 error：文件不存在。
///
/// `message` 里带的是**会话工作目录**（实测：`...your current working directory is <cwd>.`），
/// 不是文件所在目录——所以这里要 `ctx`。
/// `detail` 里带 `read_file_not_found` + 绝对路径 + Node 风格的 `ENOENT` 文案（逐字观察）。
fn file_not_found(absolute: &Path, ctx: &ToolContext) -> ToolError {
    ToolError {
        error_type: "tool_execution_failed",
        message: format!(
            "File does not exist. Note: your current working directory is {}.",
            ctx.cwd
        ),
        code: Some("TOOL_EXECUTION_FAILED"),
        detail: Some(format!(
            "code=read_file_not_found\nFile not found: {path}\nENOENT: no such file or directory, stat '{path}'",
            path = absolute.display()
        )),
    }
}

/// 观察到的读前预检文案（`.bin`）——扩展名出现在文案里，所以按实际扩展名拼。
fn binary_preflight_message(extension: &str) -> String {
    format!(
        "<tool_use_error>This tool cannot read binary files. The file appears to be a binary .{extension} file. Please use appropriate tools for binary file analysis.</tool_use_error>"
    )
}

/// 相对路径按会话工作目录解析（实测成功；工具描述说必须绝对路径，实测不然）。
///
/// 末尾按**组件**重拼一次：观察到的消息里分隔符是原生形态（`Cannot read directory as text file:
/// <cwd>\dir`），而 `PathBuf` 会原样保留输入里的分隔符——脚本给的 `file_path` 是正斜杠形式
/// （夹具录到的入参就是 `<cwd>/dir`），不归一会让消息变成 `<cwd>/dir`，与观察值不符。
fn resolve(file_path: &str, ctx: &ToolContext) -> PathBuf {
    let candidate = Path::new(file_path);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        Path::new(&ctx.cwd).join(candidate)
    };
    joined.components().collect()
}

/// 小写扩展名（实测 `.PNG` 与 `.png` 同路）。
fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
}

/// 按 `\n` 切段并去掉每段行尾的一个 `\r`；末尾空段保留。
fn split_lines(content: &str) -> Vec<&str> {
    content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

/// 选中范围、做两道上限检查、拼出带行号的正文。
fn select_and_format(content: &str, args: &Value, bytes: &[u8]) -> Result<ToolOutcome, ToolError> {
    let lines = split_lines(content);
    let line_count = lines.len();
    let offset = integer_field(args, "offset")?.unwrap_or(1);
    let limit = integer_field(args, "limit")?;
    // 「显式」= `offset` 或 `limit` 任一出现（实测：只给 `offset: 2` 也走严格错误）。
    let explicit = args.get("offset").is_some() || limit.is_some();

    if offset >= line_count as i64 {
        return Ok(ToolOutcome::text(
            OFFSET_WARNING_TEMPLATE
                .replace("{offset}", &offset.to_string())
                .replace("{lines}", &line_count.to_string()),
        ));
    }

    let start = usize::try_from(offset.saturating_sub(1)).unwrap_or(0);
    let end = match limit {
        Some(limit) => {
            let count = usize::try_from(limit).unwrap_or(usize::MAX);
            start.saturating_add(count).min(line_count)
        }
        None => line_count,
    };
    let selected = &lines[start..end];

    if explicit {
        let selected_bytes = source_bytes(selected);
        let tokens = estimated_tokens(selected_bytes);
        if tokens > TOKEN_LIMIT {
            return Err(ToolError {
                error_type: "tool_execution_failed",
                message: TOO_MANY_TOKENS_TEMPLATE.replace("{tokens}", &tokens.to_string()),
                code: Some("TOOL_EXECUTION_FAILED"),
                detail: Some("code=read_output_too_many_tokens".to_string()),
            });
        }
    } else {
        // 隐式路径：先大小上限（只在这条路径上生效——实测加了 limit 的 304999 字节文件
        // 走的是 token 错误而不是大小错误）。
        let file_size = bytes.len() as u64;
        if file_size > FILE_SIZE_LIMIT_BYTES {
            return Err(ToolError {
                error_type: "tool_execution_failed",
                message: TOO_LARGE_TEMPLATE.replace("{kb}", &kilobytes(file_size)),
                code: Some("TOOL_EXECUTION_FAILED"),
                detail: Some("code=read_file_too_large".to_string()),
            });
        }
        let tokens = estimated_tokens(file_size);
        if tokens > TOKEN_LIMIT {
            // 运行时在这里返回「部分视图」；截取规则只从一个样本反推不出来，不猜（见文件头）。
            return Err(unimplemented_surface(
                "read_partial_view",
                format!(
                    "{} 字节 / {tokens} tokens 超出 {TOKEN_LIMIT} 的 token 预算，\
                     运行时此处返回部分视图；截取行数的规则未观测齐，见 read.rs 文件头",
                    file_size
                ),
            ));
        }
    }

    Ok(ToolOutcome::text(format_lines(selected, offset)))
}

/// 选中范围在**源文本**里的字节数（各行长度 + 行间隔的 `\n`）。
///
/// 实测口径：2000 × 60 字符 + 1999 个换行 = 121999 字节 → 40667 tokens。
fn source_bytes(lines: &[&str]) -> u64 {
    let text: u64 = lines.iter().map(|line| line.len() as u64).sum();
    text + lines.len().saturating_sub(1) as u64
}

/// `error.message` 里的 KB 数：一位小数（实测 `295.4KB` = 302499 / 1024）。
#[allow(clippy::cast_precision_loss)]
fn kilobytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1024.0)
}

/// `"{number}\t{text}"` 用 `\n` 连接；`number = offset + 返回序号`。
fn format_lines(lines: &[&str], offset: i64) -> String {
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{}\t{line}", offset + index as i64))
        .collect::<Vec<_>>()
        .join("\n")
}
