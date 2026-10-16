//! `Read`：读一个文本文件并按 `cat -n` 形式带行号返回。
//!
//! **每一条语义都来自黑盒观察**（2026-10-16，探针
//! `probe-read.mts`（`read` / `trunc` / `limit-edge` / `partial` / `ext` 组）与 `probe-read-warnings.mts`（`offset-warning` 组）
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
//!   （`probe-read.mts` 的 `read` 组 `unknown-key`）。
//! - 声明说 `file_path` 必须是绝对路径，实测相对路径按会话工作目录解析并成功
//!   （`probe-read.mts` 的 `read` 组 `no-trailing-newline` 同组；夹具 `read-offset-edges` 第 6 个调用就是这条）。
//! - 声明说「Reads up to 2000 lines by default」，实测 2500 行的文件一次读到 2500 行
//!   （`probe-read.mts` 的 `trunc` 组 `slim-2500x60` 同组的 `bytes-74999`），即**没有** 2000 行默认上限。
//!
//! ## 2. 分行、行尾 `\r`、行号
//!
//! - 按 `\n` 切段，**保留**末尾空段：`"u\n"` → 两行（`"1\tu\n2\t"`）；`""` → 一行（空）。
//! - 每段**去掉一个行尾 `\r`**，行内 `\r` 保留：
//!   `"x\r\ny\r\n"` → `"1\tx\n2\ty\n3\t"`；`"a\rb\nc\n"` → `"1\ta\rb\n2\tc\n3\t"`。
//! - 返回正文 = 各行 `"{number}\t{text}"` 用 `\n` 连接；
//!   `number = offset + 返回序号`（`offset` 缺省为 1）。所以 `offset: 1` 从 1 编号，
//!   而 `offset: 0` 会从 **0** 开始编号、且从第 1 行开始取（起点 = `max(offset-1, 0)`）——
//!   `offset: 0` 的这条怪癖是实测（`probe-read.mts` 的 `read` 组 `offset-0`：`"1\n2\n3\n"` → `"0\t1\n1\t2\n2\t3\n3\t"`）。
//! - `limit` 是**行数**不是结束行号（`offset: 2, limit: 1` → 只回第 2 行）。
//!
//! ## 3. `offset` 越过最后一行：**成功**，正文是一条 system-reminder
//!
//! 判据是 `offset > **真实行数**`，真实行数与两条文案的实测见本节末尾的规则表（**不是**
//! 「`offset >= 段数`」——那条在单行文件上会多给一次提醒，是 fix round 2 修掉的缺陷）：
//!
//! ```text
//! <system-reminder>Warning: the file exists but is shorter than the provided offset ({offset}). The file has {行数} lines.</system-reminder>
//! ```
//!
//! 实测（`probe-read-warnings.mts`）：`"1\n2\n3\n"`（真实行数 3）配 `offset: 3`
//! 正常返回、配 `offset: 4` 走提醒；`"a\nb"`（真实行数 2）配 `offset: 2` 正常返回；空文件真实行数是 0。
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
//! | 同上 `offset: 2` | 152438 → 50813 tokens | **错误** 同上（「显式」的判据见下面的规则表：`offset` 键存在**且非零**，或给了 `limit`） |
//! | 同上 `limit: 3000`（夹到 2500 行） | 152499 → 50833 tokens | **错误** |
//! | 同上 `limit: 1000` | 60999 → 20333 tokens | 成功，整段返回 |
//! | 304999 B 文件 `limit: 5000` | 304999 → 101667 tokens | **错误**（token 错误，不是大小错误） |
//! | 2500×120（302499 B）`limit: 2000` | 241999 → 80667 tokens | **错误**（同上） |
//!
//! 「选中范围字节」的口径也由实测确定：是**源文本**里这些行用 `\n` 连接的字节数
//! （`60×2000 + 1999 = 121999`），不是带 `"{n}\t"` 前缀后的长度（那会是 125999 → 42000）。
//!
//! ## 5. 文件大小上限 256KB，只作用于**没有** `limit` 的路径，且在 token 检查**之前**
//!
//! `Read` 302499 B（2500×120）→ 错误
//! `File content (295.4KB) exceeds maximum allowed size (256KB). …`（295.4 = 302499/1024，一位小数）。
//! 而 304999 B 的文件加 `limit: 5000` → **不是**大小错误，是 token 错误（见上表）；302499 B 的文件加
//! `offset: 2`（没给 `limit`）→ **仍是大小错误**（尽管按 token 也会超）。所以判据是**有没有给 `limit`**，
//! 与 `offset` 无关；顺序也是实测的（夹具 `read-truncation-errors` 第 3/4/5 条三条大小错误并置）。
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
//! | 文件超出 256KB（**没给 `limit`** 时） | `tool_execution_failed` | `TOOL_EXECUTION_FAILED` | `detail` 带 `read_file_too_large` |
//!
//! 校验顺序在观察里没有两条同时触发的样本，故本实现的顺序是**选定**的（先 JSON Schema 的
//! 类型/范围闸门、再 zod 精化、最后路径非空），并在下面的注释里逐处标出。
//!
//! # `offset` 越界提醒与两道上限的**先后**
//!
//! 提醒**先于**两道上限（实测：超预算的单行文件加 `offset: 2` 得到的是提醒、不是 token 错误），
//! 判据是 `offset > 真实行数`，而**真实行数不含末尾那个空段**：
//!
//! | 文件 | 段数 | 真实行数 | `offset` | 结果 |
//! | --- | --- | --- | --- | --- |
//! | `"a\nb"` | 2 | 2 | 2 | 正文（`2\tb`） |
//! | `"a\nb\n"` | 3 | 2 | 3 | 提醒 `…offset (3). The file has 3 lines.` |
//! | `"1\n2\n3\n"` | 4 | 3 | 3 | 正文 |
//! | `""` | 1 | 0 | 1 | 提醒 `…offset (1). The file has 1 lines.` |
//! | `""` | 1 | 0 | 2 | **另一条**提醒 `…the contents are empty.` |
//! 全部来自 `probe-read-warnings.mts`；其中「同段数、不同末尾」的两行是它给出的
//! 决定性样本（`w-twoline-offset-2` 与 `w-threeline-offset-3`）。
//!
//! 上限的先后也已实测：**没给 `limit` 时大小上限在 token 检查之前**（302499 字节的文件加
//! `offset: 2` 得到大小错误），而**给了 `limit` 时没有大小上限**（304999 字节的文件加
//! `limit: 5000` 得到 token 错误）。`offset: 0` 既不算显式、也让路径保持隐式
//! （超预算的单行文件加 `offset: 0` 得到**部分视图**），所以「显式」的判据是
//! 「`offset` 键存在且非零，或给了 `limit`」。
//!
//! # 本实现**刻意不做**的唯一一件事：图片面
//!
//! 实测 `png`/`jpg`/`gif`/`webp` 会被导到图片读取路径（内容不是真图片时答复
//! `Unable to decode image data` / `read_image_processing_failed`）。图片与视频要模态内容与解码，
//! 属 M2 范围外，所以这一档（以及**没观察到的**扩展名里可能是图片的那些）返回
//! [`crate::unimplemented_surface`]；`agent-core` 的对拍用例显式断言这个标记不出现。
//! 文本是**默认档**（`.xyz` 这种没观察过的扩展名也按文本读成功，见夹具
//! `read-extension-tables`），所以「未观察到的扩展名」不构成缺口；残留未知只是
//! **没观察到的二进制扩展名**（见报告 §8.4）。

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::path_policy::{empty_path_error, resolve};
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

/// 观察值：**空文件 + `offset > 1`** 时那条 system-reminder（`probe-read-warnings.mts` 的
/// `w-empty-offset-2` 逐字录到）。空文件 + 缺省 `offset: 1` 走的是下面那条越界提醒。
pub const EMPTY_FILE_TEMPLATE: &str =
    "<system-reminder>Warning: the file exists but the contents are empty.</system-reminder>";

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

/// **观察到的**二进制扩展名（探针 `probe-read.mts` 的 `ext` 组：读前预检拒绝，`message` 带扩展名）。
///
/// 这是一张**正表**：没观察到的二进制扩展名（`.7z`/`.dll` 之类）会走文本路径，由下面的编码检查
/// （NUL / 非法 UTF-8）兜底而不是靠扩展名。这是本任务记录在案的残留未知（见报告）。
const BINARY_EXTENSIONS: &[&str] = &["bin", "exe", "zip"];

/// **观察到的**「按文本读」的扩展名（同一个 `ext` 组）。
///
/// 两处值得记：`.bmp` / `.pdf` 也被当**文本**读；而**没观察到的**扩展名（探针里的 `.xyz`）
/// 同样读成功——所以「文本」是这条分流的**默认档**，这张表只用来记录观察覆盖，
/// 不在代码里当白名单用（把未观察到的扩展名改成 fail-loud 会与 oracle 分叉，
/// 复评 Minor 4 的处置因此按新观察调整，见报告）。
pub const TEXT_EXTENSIONS: &[&str] = &[
    "bmp", "cs", "css", "html", "json", "log", "md", "pdf", "py", "rs", "ts", "txt", "xyz", "yaml",
];

/// **观察到的**图片扩展名（`ext` 组 + `limit-edge`：`png`/`jpg`/`gif`/`webp` 走图片路径）。
///
/// 图片面要模态内容与解码，属 M2 范围外，所以这一档只用来**拦住它们**、返回 fail-loud 标记
/// （而不是让它们掉进文本路径，返回一个看起来很像 oracle 的错误）。
const IMAGE_EXTENSIONS: &[&str] = &["gif", "jpg", "png", "webp"];

/// 部分视图的**字节预算**：按行累加「行长 + 1」直到放不下为止。
///
/// 值由**七个样本**夹出来（`probe-read.mts` 的 `partial` 组 p1/p2/p4/p6/p7/s0/s1）：
///
/// | 样本 | 行×宽 | 每行字节 | 显示行数 | 累加字节 | 再多一行 |
/// | --- | --- | --- | --- | --- | --- |
/// | p1 | 2500×60 | 61 | 1045 | 63745 | 63806 |
/// | p2 | 1200×100 | 101 | 631 | 63731 | 63832 |
/// | p4 | 400×250 | 251 | 253 | 63503 | 63754 |
/// | p7 | 3000×30 | 31 | 2056 | 63736 | 63767 |
/// | s1 | 900×84 | 85 | **750** | **63750** | 63835 |
///
/// 取交集得预算 ∈ [63750, 63754)：`s1` 恰好把下界顶到 63750（750×85 = 63750 被收下了），
/// `p4` 给上界（再多一行 63754 放不下）。p6（两行：100 字符 + 75000 字符）只显示 **1** 行，
/// 排除了「预算 ÷ 每行字节」的除法模型——是**逐行累加**。
/// **为什么**是 63750（= 25000 × 3 × 0.85 的 15% 余量？）从行为观察不出来：只有值被观察到了。
const PARTIAL_VIEW_BYTES: u64 = 63_750;

/// 观察到的**首行自身就超预算**时的提醒头（`trunc` 组的 bytes-75001 / bytes-75003 逐字录到）。
pub const PARTIAL_FIRST_LINE_TEMPLATE: &str = "<system-reminder>The file is too large to display in full ({tokens} estimated tokens, limit 25000). Showing a partial view of the first line because the first line alone exceeds the token budget. Use Read with a smaller range or use a search tool to find a specific section.</system-reminder>";

/// 隐式路径超预算时的「部分视图」：提醒头 + 前若干行（或首行的前若干字符）。
///
/// 两个样本给的首行截断长度完全相同（单行 75001 与 75003 字节的文件正文都是 64043 字节），
/// 即截断点是**定值**：289 字节的提醒头 + 2 个字符的行号前缀 + [`PARTIAL_VIEW_BYTES`] 个字符。
fn partial_view(lines: &[&str], tokens: u64) -> ToolOutcome {
    let mut consumed: u64 = 0;
    let mut shown = 0_usize;
    for line in lines {
        let bytes = line.len() as u64 + 1;
        if consumed + bytes > PARTIAL_VIEW_BYTES {
            break;
        }
        consumed += bytes;
        shown += 1;
    }

    if shown == 0 {
        let header = PARTIAL_FIRST_LINE_TEMPLATE.replace("{tokens}", &tokens.to_string());
        let kept: String = lines
            .first()
            .copied()
            .unwrap_or("")
            .chars()
            .take(PARTIAL_VIEW_BYTES as usize)
            .collect();
        let body = format!("{header}{NL}{NL}1{TAB}{kept}");
        return ToolOutcome::text(body);
    }

    let header = PARTIAL_VIEW_HEADER_TEMPLATE
        .replace("{tokens}", &tokens.to_string())
        .replace("{shown}", &shown.to_string())
        .replace("{total}", &lines.len().to_string())
        .replace("{next}", &(shown + 1).to_string());
    ToolOutcome::text(format!(
        "{header}{NL}{NL}{}",
        format_lines(&lines[..shown], 1)
    ))
}

/// 制表符与换行（用在 `format!` 里，避免把转义写进模板字符串）。
/// 制表符（行号前缀用）与换行。
const TAB: char = '\t';
const NL: &str = "\n";

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
        // 与 `Write` 共用同一条错误（观察值一致），实现在 `crate::path_policy`。
        return Err(empty_path_error());
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

/// 小写扩展名（实测 `.PNG` 与 `.png` 同路）。
fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
}

/// 「真实行数」= 段数减去**末尾那个空段**（文件以换行结尾时切出来的空段不算一行）。
///
/// 这是越界提醒的判据（见 `select_and_format`）："\n"a\nb"\n" 是 2，"\n"a\nb\n"\n" 也是 2，
/// `""` 是 **0**（空文件还有单独一支提醒）。实测依据在 `probe-read-warnings.mts`。
fn real_line_count(content: &str, segments: usize) -> i64 {
    if content.is_empty() {
        return 0;
    }
    let trailing = usize::from(content.ends_with('\n'));
    i64::try_from(segments - trailing).unwrap_or(i64::MAX)
}

/// 按 `\n` 切段并去掉每段行尾的一个 `\r`；末尾空段保留。
fn split_lines(content: &str) -> Vec<&str> {
    content
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

/// 重复读的填充语（逐字录到，见 `probe-read-history.mts` 的结论表）。
pub const WASTED_CALL: &str =
    "Wasted call — file unchanged since your last Read. Refer to that earlier tool_result instead.";

/// 去重键：**解析后的路径 + 范围**（`offset` 缺省归一成 1；`limit` 缺省是 `None`）。
///
/// 实测（`probe-read-history.mts`）：键含范围（换 `offset` 就回正文）、相对路径与绝对路径是同一个键、
/// 别的文件读一遍不重置、跨 turn 仍然命中。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReadKey {
    path: String,
    offset: i64,
    limit: Option<i64>,
}

/// **会话内**的文件访问记忆，两个用途共用一份状态：
///
/// 1. `Read` 的重复读去重——键（路径 + 范围）→ 上次读到该文件时的 mtime；
/// 2. `Write` / `Edit` 的「先读再写」前置条件——**路径级**的「这个会话读过/写过它吗」。
///
/// 为什么是**状态**而不是纯函数的一部分：这两条行为都定义在会话上（去重跨 turn 仍然命中；
/// 「先读再写」按会话计），所以 Rust 侧也把记忆放在会话状态里——[`crate::read::invoke`] 与
/// `crate::write::invoke` 都保持纯函数，由会话在调用前后问它。
///
/// 命名（Task 3 改）：原名 `ReadHistory`。Task 3 起 `Write`/`Edit` 也读它，名字若继续叫
/// `ReadHistory` 就成了假陈述，故改名并扩了一个路径级集合；**去重那部分的键与语义一字未动**。
#[derive(Debug, Default)]
pub struct FileState {
    /// 去重表：键 → 上次读到该文件时的 mtime（`Read` 专用，语义未变）。
    entries: std::collections::BTreeMap<ReadKey, std::time::SystemTime>,
    /// 路径级的「本会话碰过它」集合（`Write` / `Edit` 的前置条件）。
    ///
    /// 与 `entries` 分开存是有意的：去重的键含范围，而前置条件问的是**路径**，
    /// 两者粒度不同；`record` 会同时写两处，所以不会出现「读过但不知道读过」的状态。
    known_paths: std::collections::BTreeSet<String>,
}

impl FileState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 命中时给出「Wasted call」结果：**键相同且文件 mtime 未变**。
    ///
    /// 判「变了」用的是 **mtime**（决定性实测：只把 mtime 往后拨、内容一字未改 → 给正文；
    /// 改内容但把 mtime 恢复成原值 → 仍给填充语）。错误不进这张表——所以读不存在的文件
    /// 连读两次都是错误（实测）。
    pub fn stub_for(&self, args: &Value, ctx: &ToolContext) -> Option<ToolOutcome> {
        let key = read_key(args, ctx)?;
        let remembered = self.entries.get(&key)?;
        let current = mtime_of(&key.path)?;
        (current == *remembered).then(|| ToolOutcome::text(WASTED_CALL))
    }

    /// 记下这次读的 mtime。**只在成功读到文件之后调用**：文件不在（或读失败）时不记，
    /// 于是重复的失败读会一直失败——这正是实测到的「错误不去重」。
    pub fn record(&mut self, args: &Value, ctx: &ToolContext) {
        if let Some(key) = read_key(args, ctx) {
            if let Some(mtime) = mtime_of(&key.path) {
                self.known_paths.insert(key.path.clone());
                self.entries.insert(key, mtime);
            }
        }
    }

    /// 本会话是否碰过这个路径（`Read` 成功过，或 `Write`/`Edit` 成功过）——`Write` 的前置条件。
    ///
    /// 观察出处：`docs/specs/tauri-agent-runtime.md` §3.3（`Write` 覆盖已存在但本会话未读过的文件
    /// → `write_file_not_read`；读过的可以覆盖）。**按解析后的路径**判，与 `Read` 的路径解析同一份规则。
    pub fn knows_path(&self, file_path: &str, ctx: &ToolContext) -> bool {
        self.known_paths
            .contains(&resolve(file_path, ctx).to_string_lossy().to_string())
    }

    /// 记下「写成功了」：`Write` / `Edit` 成功后该文件也进入「已读」状态（观察：成功文案里的
    /// 「file state is current in your context — no need to Read it back」是这条语义的自述）。
    pub fn record_written(&mut self, file_path: &str, ctx: &ToolContext) {
        self.known_paths
            .insert(resolve(file_path, ctx).to_string_lossy().to_string());
    }
}

/// 从入参算出键；入参不合法（`offset` 不是整数等）时给 `None`（那种情况走的是校验错误路径）。
fn read_key(args: &Value, ctx: &ToolContext) -> Option<ReadKey> {
    let file_path = args.get("file_path")?.as_str()?;
    if file_path.is_empty() {
        return None;
    }
    let offset = integer_field(args, "offset").ok()?.unwrap_or(1);
    let limit = integer_field(args, "limit").ok()?;
    Some(ReadKey {
        path: resolve(file_path, ctx).to_string_lossy().to_string(),
        offset,
        limit,
    })
}

/// 文件的 mtime；文件不存在或不是普通文件时 `None`。
fn mtime_of(path: &str) -> Option<std::time::SystemTime> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    metadata.modified().ok()
}

/// 选中范围、做两道上限检查、拼出带行号的正文。
fn select_and_format(content: &str, args: &Value, bytes: &[u8]) -> Result<ToolOutcome, ToolError> {
    let lines = split_lines(content);
    let line_count = lines.len();
    let offset_arg = integer_field(args, "offset")?;
    // 缺省 offset 是 1（行号从 1 起算），但**缺省**与**显式给 0** 在下面要分开：
    // 缺省不是「显式范围」，而显式给 0 也不是（实测 `w-big-overbudget-offset-0`）。
    let offset = offset_arg.unwrap_or(1);
    let limit = integer_field(args, "limit")?;
    // 「显式」的判据由实测夹出来（`probe-read-warnings.mts` / `trunc` 组）：
    // 只给 `offset: 2` → 走严格错误（显式）；而 **`offset: 0` 走隐式路径**——超预算的单行文件
    // （75001 字节）加 `offset: 0` 得到的是**部分视图**（正文 64043 字节），加 `offset: 2`
    // 得到的是越界提醒。所以判据是「`offset` 非零」，不是「`offset` 键存在」。
    let explicit = offset_arg.is_some_and(|value| value != 0) || limit.is_some();

    // 越界提醒**先于**两道上限判断（实测：超预算的单行文件加 `offset: 2` 给提醒、不是 token 错误）。
    // 判据是 `offset > 真实行数`，而**真实行数不含末尾那个空段**："\n"a\nb"\n"（2 段、
    // 末尾非空）配 offset 2 给正文，而 "\n"a\nb\n"\n"（3 段、末尾空）配 offset 3 给提醒。
    // 两处提醒文案都由 `probe-read-warnings.mts` 逐字录到。
    let real_lines = real_line_count(content, line_count);
    if offset > real_lines {
        if content.is_empty() && offset > 1 {
            return Ok(ToolOutcome::text(EMPTY_FILE_TEMPLATE));
        }
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

    // 大小上限（256KB）**只在没给 `limit` 时生效**，而且**先于** token 检查：实测
    // ① 给了 `limit: 5000` 的 304999 字节文件得到的是 token 错误（不是大小错误）；
    // ② 没给 `limit` 的 `wide.txt`（302499 字节）加 `offset: 2` 得到的是**大小错误**
    //    （哪怕按 token 算它也会超）——所以大小检查在 token 检查之前。
    let file_size = bytes.len() as u64;
    if limit.is_none() && file_size > FILE_SIZE_LIMIT_BYTES {
        return Err(ToolError {
            error_type: "tool_execution_failed",
            message: TOO_LARGE_TEMPLATE.replace("{kb}", &kilobytes(file_size)),
            code: Some("TOOL_EXECUTION_FAILED"),
            detail: Some("code=read_file_too_large".to_string()),
        });
    }

    let selected_tokens = estimated_tokens(source_bytes(selected));
    if selected_tokens > TOKEN_LIMIT {
        // 显式范围（`offset` 非零或给了 `limit`）→ 直接报错；两者都没给 → 给**部分视图**。
        if explicit {
            return Err(ToolError {
                error_type: "tool_execution_failed",
                message: TOO_MANY_TOKENS_TEMPLATE.replace("{tokens}", &selected_tokens.to_string()),
                code: Some("TOOL_EXECUTION_FAILED"),
                detail: Some("code=read_output_too_many_tokens".to_string()),
            });
        }
        // 隐式路径下选中范围就是整个文件（没有 offset、也没有 limit），所以这里两个口径相同。
        return Ok(partial_view(&lines, estimated_tokens(file_size)));
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
