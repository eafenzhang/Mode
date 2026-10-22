//! `createTempTextAttachment`：把长粘贴文本落成临时文件，供 composer 作为附件提交。
//!
//! 规格：`docs/specs/tauri-platform-commands.md` §1 第 12 条。
//! 行为契约：请求 `{ text, filename? }`，返回 `{ filename, localPath, mimeType, sizeBytes }`。
//!
//! 落点：系统临时目录下的 `mode-temp-attachments/<pid>-<纳秒>/<filename>`。
//! - 每次调用一个**唯一子目录**，同名附件不会互相覆盖；
//! - 文件名只取最后一段（去掉 `/` 与 `\` 路径分量），非法字符替换为 `_`；
//! - 空名 / `.` / `..` 回退到 `attachment.txt`；
//! - 失败向上抛稳定错误码，由渲染层按当前 locale 展示，不在壳里拼用户文案。
//!
//! 这是 Tauri 线的真实实现；Electron 线的 main IPC 已随旧线 EOL 删除。

use std::fmt;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use protocol::{CreateTempTextAttachmentRequest, CreateTempTextAttachmentResult, TempTextMimeType};

/// 临时附件目录名（位于系统临时目录下）。
pub const TEMP_ATTACHMENT_DIR: &str = "mode-temp-attachments";

/// 缺省文件名。
const DEFAULT_FILENAME: &str = "attachment.txt";

/// 写入失败的原因；命令面把它折成稳定错误码，细节只进 stderr。
#[derive(Debug)]
pub enum TempAttachmentError {
    CreateDirectory(std::io::Error),
    WriteFile(std::io::Error),
}

impl fmt::Display for TempAttachmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDirectory(error) => write!(formatter, "创建临时附件目录失败：{error}"),
            Self::WriteFile(error) => write!(formatter, "写入临时附件失败：{error}"),
        }
    }
}

impl std::error::Error for TempAttachmentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CreateDirectory(error) | Self::WriteFile(error) => Some(error),
        }
    }
}

/// `IPlatformService.createTempTextAttachment(payload)` 的 Tauri 命令。
///
/// 失败返回稳定错误码（`create_temp_text_attachment_failed`）；具体 IO 原因写 stderr，
/// 避免把机器路径带进用户可见文案。
#[tauri::command(rename = "createTempTextAttachment")]
pub fn create_temp_text_attachment(
    payload: CreateTempTextAttachmentRequest,
) -> Result<CreateTempTextAttachmentResult, String> {
    let root = std::env::temp_dir().join(TEMP_ATTACHMENT_DIR);
    match write_temp_text_attachment_in(&root, &payload) {
        Ok(result) => Ok(result),
        Err(error) => {
            eprintln!("[createTempTextAttachment] {error}");
            Err("create_temp_text_attachment_failed".to_string())
        }
    }
}

/// 在指定根目录下写入一条临时文本附件。
///
/// 抽成纯函数是为了让集成测试用隔离目录驱动真实写盘；命令面只负责选系统临时目录。
pub fn write_temp_text_attachment_in(
    root: &Path,
    request: &CreateTempTextAttachmentRequest,
) -> Result<CreateTempTextAttachmentResult, TempAttachmentError> {
    let filename = sanitize_filename(request.filename.as_deref());
    let directory = root.join(unique_directory_name());
    fs::create_dir_all(&directory).map_err(TempAttachmentError::CreateDirectory)?;

    let path = directory.join(&filename);
    fs::write(&path, request.text.as_bytes()).map_err(TempAttachmentError::WriteFile)?;

    Ok(CreateTempTextAttachmentResult {
        filename,
        local_path: path.to_string_lossy().into_owned(),
        mime_type: TempTextMimeType::TextPlain,
        size_bytes: request.text.len() as u64,
    })
}

/// 每次调用一个唯一子目录：进程 id + 纳秒时间戳。时钟回拨时退化为进程 id，仍然唯一到进程内。
fn unique_directory_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

/// 只保留最后一段文件名，并替换 Windows 非法字符；空名/`.`/`..` 回退到 `attachment.txt`。
fn sanitize_filename(raw: Option<&str>) -> String {
    let candidate = raw
        .unwrap_or_default()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim();
    if candidate.is_empty() || candidate == "." || candidate == ".." {
        return DEFAULT_FILENAME.to_string();
    }

    let mut sanitized = String::with_capacity(candidate.len());
    for ch in candidate.chars() {
        if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
            sanitized.push('_');
        } else {
            sanitized.push(ch);
        }
    }
    // Windows 不允许文件名以空格或点结尾。
    let trimmed = sanitized.trim_end_matches([' ', '.']);
    if trimmed.is_empty() {
        DEFAULT_FILENAME.to_string()
    } else {
        trimmed.to_string()
    }
}
