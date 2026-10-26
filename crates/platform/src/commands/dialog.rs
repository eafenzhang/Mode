//! 文件对话框面：`selectDirectory` / `selectFile` / `saveFile`。
//!
//! 三者都是「用户交互后才有结果」，故用回调式 API + oneshot 接成 async command。
//! 取消（用户关掉对话框）返回 `null` / `canceled`，不是错误。
//!
//! `saveFile` 的两条分支（`data` 与 `sourceUrl`）**顺序一致**：先让用户选路径，再落盘。
//! `sourceUrl` 分支的取回面在 [`crate::source_url`]，带超时与字节上限（M3 Task 4 · DR-T4-3）；
//! 取回失败时**不写任何字节**——「失败」与「写了一半」在结构上不可能同时发生。
use protocol::{SaveFileRequest, SaveFileRequestError, SaveFileResult};
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, FilePath};
use tokio::sync::oneshot;

use crate::source_url;

/// 把回调式对话框接成 `async` 结果；回调被丢弃（对话框被外部关闭）时视为取消。
///
/// `pub(crate)`：`exportLogs` 的保存对话框复用同一接线（一处实现，不各写一份）。
pub(crate) async fn await_pick<F>(open: F) -> Option<FilePath>
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
pub(crate) fn path_string(picked: Option<FilePath>) -> Option<String> {
    picked
        .and_then(|file_path| file_path.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
}

/// 用户取消（关掉对话框）：`canceled: true`，没有错误。
fn canceled() -> SaveFileResult {
    SaveFileResult {
        canceled: Some(true),
        error: None,
        path: None,
        success: false,
    }
}

/// 失败且**没有落任何文件**：只带稳定错误码，不带 `path`。
///
/// 为什么失败时不报 `path`：那个路径上不存在文件（取回失败发生在写之前），
/// 报一个路径等于暗示「那里有东西（只是没成功）」。
fn failed(code: &str) -> SaveFileResult {
    SaveFileResult {
        canceled: None,
        error: Some(code.to_string()),
        path: None,
        success: false,
    }
}

/// 把字节写到用户选的路径上。写失败**保留 `path`**：路径是用户选的，且那里可能留下半截
/// （写失败的情形本来就可能是磁盘满）——把位置报出来，用户才知道去哪儿看。
fn write_bytes(path: String, data: &[u8]) -> SaveFileResult {
    match std::fs::write(&path, data) {
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

/// `IPlatformService.selectDirectory(): Promise<string | null>`
#[tauri::command(rename = "selectDirectory")]
pub async fn select_directory<R: tauri::Runtime>(app: AppHandle<R>) -> Option<String> {
    let picked = await_pick(|callback| app.dialog().file().pick_folder(callback)).await;
    path_string(picked)
}

/// `IPlatformService.selectFile(): Promise<string | null>`
#[tauri::command(rename = "selectFile")]
pub async fn select_file<R: tauri::Runtime>(app: AppHandle<R>) -> Option<String> {
    let picked = await_pick(|callback| app.dialog().file().pick_file(callback)).await;
    path_string(picked)
}

/// `IPlatformService.saveFile(payload: SaveFileRequest): Promise<SaveFileResult>`
///
/// M3 Task 4 起两条分支都是完整实现：
/// - `data`（渲染层已持有字节）：对话框 → 写；
/// - `sourceUrl`（从远端取回）：对话框 → **有上限地**取回 → 写。M1 在这里返回
///   `save_file_source_url_not_implemented_until_m3`（不静默写空文件），本任务把它换成真实现，
///   上限与超时见 `crate::source_url`（改它们必须改动那两处的钉值用例）。
///
/// **取舍（登记）**：取回发生在用户选定路径**之后**，所以最坏情况是「选完路径再等 30s」
/// （超时上限）。反过来先取回再弹对话框会让「用户还没决定保存」就先发生一次网络下载。
#[tauri::command(rename = "saveFile")]
pub async fn save_file<R: tauri::Runtime>(
    app: AppHandle<R>,
    payload: SaveFileRequest,
) -> SaveFileResult {
    if let Err(error) = payload.validate() {
        return failed(match error {
            SaveFileRequestError::MissingPayload => "save_file_missing_payload",
            SaveFileRequestError::ConflictingPayload => "save_file_conflicting_payload",
        });
    }

    let suggested_name = payload.suggested_name.clone();
    let picked = await_pick(move |callback| {
        app.dialog()
            .file()
            .set_file_name(suggested_name)
            .save_file(callback);
    })
    .await;

    let Some(path) = path_string(picked) else {
        return canceled();
    };

    if let Some(data) = payload.data {
        return write_bytes(path, &data);
    }

    // `validate()` 已经排除「两个都缺」，所以这里不可达；仍然显式返回而不是 unwrap
    // （命令面不该有 panic 路径）。
    let Some(source_url) = payload.source_url else {
        return failed("save_file_missing_payload");
    };

    match source_url::fetch_bounded_detailed(&source_url, source_url::Limits::PRODUCT).await {
        Ok(bytes) => write_bytes(path, &bytes),
        Err(failure) => {
            // 细节只进诊断（stderr），上抛的是稳定码：UI 按码决定文案，不该被 URL/远端细节影响。
            // （细节进日志正是 Ruling 30 要的东西：`…_failed` 一个码压着「证书不对 / 连不上 /
            //  协议错」三种完全不同的现场。）
            eprintln!(
                "[saveFile] sourceUrl 取回失败（{}）：{source_url} 详情：{}",
                failure.kind.code(),
                failure.detail
            );
            failed(failure.kind.code())
        }
    }
}
