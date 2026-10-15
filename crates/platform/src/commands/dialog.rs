//! 文件对话框面：`selectDirectory` / `selectFile` / `saveFile`。
//!
//! 三者都是「用户交互后才有结果」，故用回调式 API + oneshot 接成 async command。
//! 取消（用户关掉对话框）返回 `null` / `canceled`，不是错误。
use protocol::{SaveFileRequest, SaveFileRequestError, SaveFileResult};
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, FilePath};
use tokio::sync::oneshot;

/// 把回调式对话框接成 `async` 结果；回调被丢弃（对话框被外部关闭）时视为取消。
async fn await_pick<F>(open: F) -> Option<FilePath>
where
    F: FnOnce(Box<dyn FnOnce(Option<FilePath>) + Send + 'static>),
{
    let (tx, rx) = oneshot::channel();
    open(Box::new(move |picked| {
        // 接收端可能已因超时/取消而丢弃；这里的失败无需上报。
        let _ = tx.send(picked);
    }));
    rx.await.unwrap_or(None)
}

/// 选中路径转字符串；非本地文件系统路径（如 `content://`）视为取消，
/// 因为契约要求返回「agent 可访问的本地绝对路径」。
fn path_string(picked: Option<FilePath>) -> Option<String> {
    picked
        .and_then(|file_path| file_path.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
}

/// `IPlatformService.selectDirectory(): Promise<string | null>`
#[tauri::command]
pub async fn select_directory<R: tauri::Runtime>(app: AppHandle<R>) -> Option<String> {
    let picked = await_pick(|callback| app.dialog().file().pick_folder(callback)).await;
    path_string(picked)
}

/// `IPlatformService.selectFile(): Promise<string | null>`
#[tauri::command]
pub async fn select_file<R: tauri::Runtime>(app: AppHandle<R>) -> Option<String> {
    let picked = await_pick(|callback| app.dialog().file().pick_file(callback)).await;
    path_string(picked)
}

/// `IPlatformService.saveFile(payload: SaveFileRequest): Promise<SaveFileResult>`
///
/// `data` 分支（渲染层已持有字节）完整实现；`sourceUrl` 分支需要一次 HTTP 取回，
/// M1 骨架没有 HTTP 客户端，**明确失败**而不是静默写空文件。
#[tauri::command]
pub async fn save_file<R: tauri::Runtime>(
    app: AppHandle<R>,
    payload: SaveFileRequest,
) -> SaveFileResult {
    if let Err(error) = payload.validate() {
        return SaveFileResult {
            canceled: None,
            error: Some(match error {
                SaveFileRequestError::MissingPayload => "save_file_missing_payload".to_string(),
                SaveFileRequestError::ConflictingPayload => {
                    "save_file_conflicting_payload".to_string()
                }
            }),
            path: None,
            success: false,
        };
    }

    let Some(data) = payload.data else {
        return SaveFileResult {
            canceled: None,
            error: Some("save_file_source_url_not_implemented_until_m3".to_string()),
            path: None,
            success: false,
        };
    };

    let suggested_name = payload.suggested_name.clone();
    let picked = await_pick(move |callback| {
        app.dialog()
            .file()
            .set_file_name(suggested_name)
            .save_file(callback);
    })
    .await;

    let Some(path) = path_string(picked) else {
        return SaveFileResult {
            canceled: Some(true),
            error: None,
            path: None,
            success: false,
        };
    };

    match std::fs::write(&path, &data) {
        Ok(()) => SaveFileResult {
            canceled: None,
            error: None,
            path: Some(path),
            success: true,
        },
        Err(error) => SaveFileResult {
            canceled: None,
            error: Some(error.to_string()),
            path: Some(path),
            success: false,
        },
    }
}
