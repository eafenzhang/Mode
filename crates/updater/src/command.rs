//! 更新链的**可执行触发面**（M5 Task 1）。
//!
//! M1–M4 期间这里只有「声明端点 + 注册插件」：装好的应用无法触发更新，渲染层的
//! `executeDesktopCommand("checkForUpdates")` 会拿到 `desktop_command_unsupported`。
//! 本模块补上唯一一条真实路径：检查 → 下载 → 校验签名 → 交给 NSIS 安装器 → 重启。
//!
//! ## 为什么不是一个独立的 `#[tauri::command]`
//!
//! 产品 UI 的「帮助 → 检查更新」本来就发
//! [`crate::CHECK_FOR_UPDATES_COMMAND`]（`@mode/shared` 的
//! `DesktopCommandIds.CheckForUpdates`）到壳的 `executeDesktopCommand`。把触发接在这里，
//! 渲染层不需要任何改动，也不会多出第二个命令名。壳侧只在
//! `window_controls::execute_desktop_command` 里为这一个 id 分支。
//!
//! ## Windows 上的语义
//!
//! `tauri-plugin-updater` 的 `Update::install` 在 Windows 上会：
//! 1. 再次校验签名（`verify_signature`，公钥来自 `tauri.conf.json`）；
//! 2. 用 `ShellExecuteW` 启动 NSIS 安装器（默认参数含 `/UPDATE /R`，即安装后重启应用）；
//! 3. `std::process::exit(0)`。
//!
//! 因此成功路径上的「返回值」不可达；调用方只会看到进程退出。失败（网络、清单、签名）
//! 则返回 `{"status":"error", ...}`，旧版本继续运行——这就是 fail closed。
//!
//! ## 观测面
//!
//! 每个阶段都向 stderr 写一行 `[mode-tauri] updater ...`。发布构建是
//! `windows_subsystem = "windows"`，从资源管理器启动时没有 stderr；演练与排障必须从
//! 带管道的发起进程启动（见 `RELEASE.md` 的更新演练步骤）。

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use serde_json::{json, Value};
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::UpdaterExt;

/// 产品 UI 通过 `executeDesktopCommand` 发来的更新命令 id。
pub const CHECK_FOR_UPDATES_COMMAND: &str = "checkForUpdates";

/// 同一时刻只允许一轮更新；重复点击返回 `busy` 而不是并发下载/并发启动安装器。
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 检查并安装更新。
///
/// 返回的 JSON 只用于诊断与自动化断言（渲染层当前忽略返回值）：
///
/// - `{"status":"up-to-date","currentVersion":...}`
/// - `{"status":"error","phase":...,"error":...}`（旧版本继续运行）
/// - `{"status":"busy"}`（已有一轮在进行）
///
/// Windows 安装成功时进程退出，不会返回 `"installed"`。
pub async fn check_and_install<R: Runtime>(app: &AppHandle<R>) -> Value {
    if IN_FLIGHT.swap(true, Ordering::SeqCst) {
        eprintln!("[mode-tauri] updater trigger ignored: already in flight");
        return json!({ "status": "busy" });
    }

    let outcome = run(app).await;
    IN_FLIGHT.store(false, Ordering::SeqCst);
    outcome
}

async fn run<R: Runtime>(app: &AppHandle<R>) -> Value {
    let current_version = app.package_info().version.to_string();
    eprintln!("[mode-tauri] updater check start current={current_version}");

    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(error) => {
            eprintln!("[mode-tauri] updater init failed error={error}");
            return json!({
                "status": "error",
                "phase": "init",
                "currentVersion": current_version,
                "error": error.to_string(),
            });
        }
    };

    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => {
            eprintln!("[mode-tauri] updater check result=up-to-date current={current_version}");
            return json!({ "status": "up-to-date", "currentVersion": current_version });
        }
        Err(error) => {
            eprintln!("[mode-tauri] updater check failed error={error}");
            return json!({
                "status": "error",
                "phase": "check",
                "currentVersion": current_version,
                "error": error.to_string(),
            });
        }
    };

    eprintln!(
        "[mode-tauri] updater update_available current={} version={} target={} url={}",
        update.current_version, update.version, update.target, update.download_url
    );

    let received = Arc::new(AtomicUsize::new(0));
    let on_chunk_received = received.clone();
    let on_finish_received = received.clone();

    let bytes = match update
        .download(
            move |chunk, total| {
                let now = on_chunk_received.fetch_add(chunk, Ordering::Relaxed) + chunk;
                if let Some(total) = total {
                    if now as u64 == total {
                        eprintln!("[mode-tauri] updater download complete bytes={now}/{total}");
                    }
                }
            },
            move || {
                eprintln!(
                    "[mode-tauri] updater download finished bytes={}",
                    on_finish_received.load(Ordering::Relaxed)
                );
            },
        )
        .await
    {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("[mode-tauri] updater download failed error={error}");
            return json!({
                "status": "error",
                "phase": "download",
                "currentVersion": current_version,
                "version": update.version,
                "error": error.to_string(),
            });
        }
    };

    eprintln!(
        "[mode-tauri] updater signature verified bytes={} version={}",
        bytes.len(),
        update.version
    );

    // Windows: 校验通过后启动 NSIS 安装器并退出进程；失败则返回错误、旧版本继续运行。
    match update.install(bytes) {
        Ok(()) => json!({
            "status": "installing",
            "currentVersion": current_version,
            "version": update.version,
        }),
        Err(error) => {
            eprintln!("[mode-tauri] updater install failed error={error}");
            json!({
                "status": "error",
                "phase": "install",
                "currentVersion": current_version,
                "version": update.version,
                "error": error.to_string(),
            })
        }
    }
}
