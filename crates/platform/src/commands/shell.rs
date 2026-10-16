//! shell 面：`openExternal` / `openInFileManager`。
use std::path::Path;

use protocol::OpenPathResult;
use tauri_plugin_opener::OpenerExt;

/// `IPlatformService.openExternal(url: string): void`
///
/// 语义与既有 `openExternal` 对齐：把 URL 交给系统默认处理器（OAuth 跳转、外部链接）。
/// M1 **不收紧 scheme**——收紧会改变现网行为（`mailto:`、编辑器 deep link 等都在用），
/// 那是一次需要单独对齐的加固决策，不在骨架期顺手做。只拒绝空串/纯空白。
#[tauri::command(rename = "openExternal")]
pub fn open_external<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    url: String,
) -> Result<(), String> {
    if url.trim().is_empty() {
        return Err("empty_url".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| error.to_string())
}

/// `IPlatformService.openInFileManager(path: string): Promise<{success, error?}>`
///
/// 调用面两种形态都有（`useFileContextActions` 传文件、`WorkspaceFileTree` 等传目录），
/// 故按路径类型分派：目录 → 用文件管理器打开；文件 → 在父目录里选中它。
/// 路径不存在时明确失败，不静默返回 success。
#[tauri::command(rename = "openInFileManager")]
pub fn open_in_file_manager<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    path: String,
) -> OpenPathResult {
    match Path::new(&path).metadata() {
        Ok(metadata) if metadata.is_dir() => match app.opener().open_path(path, None::<&str>) {
            Ok(()) => OpenPathResult {
                success: true,
                error: None,
            },
            Err(error) => OpenPathResult {
                success: false,
                error: Some(error.to_string()),
            },
        },
        Ok(_) => match app.opener().reveal_item_in_dir(Path::new(&path)) {
            Ok(()) => OpenPathResult {
                success: true,
                error: None,
            },
            Err(error) => OpenPathResult {
                success: false,
                error: Some(error.to_string()),
            },
        },
        Err(error) => OpenPathResult {
            success: false,
            error: Some(format!("path_unavailable: {error}")),
        },
    }
}
