//! Tauri 更新状态机：检查 / 下载 / 安装拆开，状态通过事件推给渲染层。
//!
//! Electron 线的「检查更新」不是直接安装：main 维护
//! `UpdateStatePayload`（checking / update-available / download-progress /
//! update-downloaded），渲染层顶部更新入口和原版 `UpdateStatusDialog` 都消费这份状态。
//! 本模块把同一份状态机落到 Tauri 壳，避免 `executeDesktopCommand("checkForUpdates")`
//! 直接下载并静默安装。
//!
//! 事件名 `mode:update-state` 由注入层订阅；命令名由
//! `crates/sidecar-host/src/window_mode.ts` 调用。

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_updater::{Update, UpdaterExt};

/// 渲染层订阅的更新状态事件名。
pub const UPDATE_STATE_EVENT: &str = "mode:update-state";

/// 本模块注册的命令名；与注入层的 invoke 名逐字一致。
pub const COMMAND_NAMES: &[&str] = &[
    "getUpdateState",
    "downloadUpdate",
    "cancelUpdateDownload",
    "quitAndInstallUpdate",
    "skipUpdateVersion",
    "getAutoUpdatePreferences",
    "setAutoDownloadAndInstallUpdates",
];

/// 更新状态机的所有可变状态。
pub struct UpdateManager {
    state: Mutex<Value>,
    update: Mutex<Option<Update>>,
    release_notes: Mutex<Option<Value>>,
    bytes: Mutex<Option<Vec<u8>>>,
    checking: AtomicBool,
    auto_download: AtomicBool,
    cancelling: AtomicBool,
    skipped_version: Mutex<Option<String>>,
}

impl Default for UpdateManager {
    fn default() -> Self {
        Self {
            state: Mutex::new(json!({ "kind": "idle", "enabled": true })),
            update: Mutex::new(None),
            release_notes: Mutex::new(None),
            bytes: Mutex::new(None),
            checking: AtomicBool::new(false),
            auto_download: AtomicBool::new(false),
            cancelling: AtomicBool::new(false),
            skipped_version: Mutex::new(None),
        }
    }
}

fn set_state<R: Runtime>(app: &AppHandle<R>, state: Value) {
    if let Some(manager) = app.try_state::<UpdateManager>() {
        if let Ok(mut current) = manager.state.lock() {
            *current = state.clone();
        }
    }
    let _ = app.emit(UPDATE_STATE_EVENT, state);
}

fn state_snapshot<R: Runtime>(app: &AppHandle<R>) -> Value {
    app.try_state::<UpdateManager>()
        .and_then(|manager| manager.state.lock().ok().map(|state| state.clone()))
        .unwrap_or_else(|| json!({ "kind": "idle", "enabled": true }))
}

fn release_notes_for(update: &Update) -> Option<Value> {
    update.body.as_ref().map(|markdown| {
        json!({
            "version": update.version,
            "title": format!("Mode {}", update.version),
            "markdown": markdown,
        })
    })
}

/// 手动检查更新：只发状态、不自动安装。`executeDesktopCommand("checkForUpdates")` 调用。
pub async fn check_for_update<R: Runtime>(app: &AppHandle<R>) -> Value {
    let Some(manager) = app.try_state::<UpdateManager>() else {
        return json!({ "status": "error", "error": "update manager unavailable" });
    };
    if manager
        .checking
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return json!({ "status": "busy" });
    }
    set_state(app, json!({ "kind": "checking", "enabled": true }));

    let current_version = app.package_info().version.to_string();
    let result = match app.updater() {
        Ok(updater) => match updater.check().await {
            Ok(Some(update)) => {
                let notes = release_notes_for(&update);
                if let Ok(mut slot) = manager.update.lock() {
                    *slot = Some(update.clone());
                }
                if let Ok(mut slot) = manager.release_notes.lock() {
                    *slot = notes.clone();
                }
                if let Ok(mut slot) = manager.bytes.lock() {
                    *slot = None;
                }
                let state = json!({
                    "kind": "update-available",
                    "enabled": true,
                    "version": update.version,
                    "releaseNotes": notes,
                });
                set_state(app, state);
                json!({
                    "status": "update-available",
                    "version": update.version,
                    "releaseNotes": notes,
                })
            }
            Ok(None) => {
                if let Ok(mut slot) = manager.update.lock() {
                    *slot = None;
                }
                if let Ok(mut slot) = manager.release_notes.lock() {
                    *slot = None;
                }
                set_state(app, json!({ "kind": "idle", "enabled": true }));
                json!({ "status": "up-to-date", "currentVersion": current_version })
            }
            Err(error) => {
                set_state(app, json!({ "kind": "idle", "enabled": true }));
                json!({ "status": "error", "error": error.to_string() })
            }
        },
        Err(error) => {
            set_state(app, json!({ "kind": "idle", "enabled": true }));
            json!({ "status": "error", "error": error.to_string() })
        }
    };
    manager.checking.store(false, Ordering::SeqCst);
    result
}

/// 下载已发现的更新；进度通过 `mode:update-state` 持续推送。
pub async fn download_update<R: Runtime>(app: &AppHandle<R>) -> Value {
    let Some(manager) = app.try_state::<UpdateManager>() else {
        return json!({ "status": "error", "error": "update manager unavailable" });
    };
    let update = match manager.update.lock().ok().and_then(|slot| slot.clone()) {
        Some(update) => update,
        None => return json!({ "status": "error", "error": "no update available" }),
    };
    manager.cancelling.store(false, Ordering::SeqCst);
    let notes = manager
        .release_notes
        .lock()
        .ok()
        .and_then(|slot| slot.clone());
    let version = update.version.clone();
    let progress_state = |progress: String, transferred: usize, total: Option<u64>| {
        json!({
            "kind": "download-progress",
            "enabled": true,
            "progress": progress,
            "transferredBytes": transferred,
            "totalBytes": total,
            "version": version,
            "releaseNotes": notes,
        })
    };
    set_state(app, progress_state("0".to_string(), 0, None));

    let transferred = Arc::new(AtomicUsize::new(0));
    let app_for_chunk = app.clone();
    let version_for_chunk = update.version.clone();
    let notes_for_chunk = notes.clone();
    let on_chunk = move |chunk: usize, total: Option<u64>| {
        let now = transferred.fetch_add(chunk, Ordering::Relaxed) + chunk;
        let progress = match total {
            Some(total) if total > 0 => {
                format!("{:.0}", (now as f64 / total as f64) * 100.0)
            }
            _ => format!("{} KB", now / 1024),
        };
        let state = json!({
            "kind": "download-progress",
            "enabled": true,
            "progress": progress,
            "transferredBytes": now,
            "totalBytes": total,
            "version": version_for_chunk,
            "releaseNotes": notes_for_chunk,
        });
        set_state(&app_for_chunk, state);
    };

    let downloaded = update.download(on_chunk, || {}).await;
    if manager.cancelling.swap(false, Ordering::SeqCst) {
        set_state(
            app,
            json!({
                "kind": "update-available",
                "enabled": true,
                "version": update.version,
                "releaseNotes": notes,
            }),
        );
        return json!({ "status": "cancelled" });
    }
    match downloaded {
        Ok(bytes) => {
            if let Ok(mut slot) = manager.bytes.lock() {
                *slot = Some(bytes);
            }
            set_state(
                app,
                json!({
                    "kind": "update-downloaded",
                    "enabled": true,
                    "version": update.version,
                }),
            );
            json!({ "status": "downloaded", "version": update.version })
        }
        Err(error) => {
            set_state(
                app,
                json!({
                    "kind": "update-available",
                    "enabled": true,
                    "version": update.version,
                    "releaseNotes": notes,
                }),
            );
            json!({ "status": "error", "error": error.to_string() })
        }
    }
}

/// 安装已下载的更新；Windows 成功时 NSIS 安装器会接管并退出进程。
pub fn quit_and_install_update<R: Runtime>(app: &AppHandle<R>) -> Value {
    let Some(manager) = app.try_state::<UpdateManager>() else {
        return json!({ "status": "error", "error": "update manager unavailable" });
    };
    let update = manager.update.lock().ok().and_then(|slot| slot.clone());
    let bytes = manager.bytes.lock().ok().and_then(|mut slot| slot.take());
    let (Some(update), Some(bytes)) = (update, bytes) else {
        return json!({ "status": "error", "error": "update is not downloaded" });
    };
    match update.install(bytes) {
        Ok(()) => json!({ "status": "installing", "version": update.version }),
        Err(error) => json!({ "status": "error", "error": error.to_string() }),
    }
}

#[tauri::command(rename = "getUpdateState")]
pub fn get_update_state<R: Runtime>(app: AppHandle<R>, _manager: State<'_, UpdateManager>) -> Value {
    state_snapshot(&app)
}

#[tauri::command(rename = "downloadUpdate")]
pub async fn download_update_command<R: Runtime>(app: AppHandle<R>) -> Value {
    download_update(&app).await
}

#[tauri::command(rename = "cancelUpdateDownload")]
pub fn cancel_update_download<R: Runtime>(
    app: AppHandle<R>,
    _manager: State<'_, UpdateManager>,
) -> Value {
    if let Some(manager) = app.try_state::<UpdateManager>() {
        manager.cancelling.store(true, Ordering::SeqCst);
        let version = manager
            .update
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(|update| update.version.clone()));
        let notes = manager
            .release_notes
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(version) = version {
            set_state(
                &app,
                json!({
                    "kind": "update-available",
                    "enabled": true,
                    "version": version,
                    "releaseNotes": notes,
                }),
            );
        }
    }
    json!({ "status": "cancelled" })
}

#[tauri::command(rename = "quitAndInstallUpdate")]
pub fn quit_and_install_update_command<R: Runtime>(app: AppHandle<R>) -> Value {
    quit_and_install_update(&app)
}

#[tauri::command(rename = "skipUpdateVersion")]
pub fn skip_update_version<R: Runtime>(
    app: AppHandle<R>,
    _manager: State<'_, UpdateManager>,
    version: String,
) -> Value {
    if let Some(manager) = app.try_state::<UpdateManager>() {
        if let Ok(mut slot) = manager.skipped_version.lock() {
            *slot = Some(version);
        }
        set_state(&app, json!({ "kind": "idle", "enabled": true }));
    }
    json!({ "status": "skipped" })
}

#[tauri::command(rename = "getAutoUpdatePreferences")]
pub fn get_auto_update_preferences<R: Runtime>(
    app: AppHandle<R>,
    _manager: State<'_, UpdateManager>,
) -> Value {
    let enabled = app
        .try_state::<UpdateManager>()
        .map(|manager| manager.auto_download.load(Ordering::SeqCst))
        .unwrap_or(false);
    json!({ "autoDownloadAndInstallUpdates": enabled })
}

#[tauri::command(rename = "setAutoDownloadAndInstallUpdates")]
pub fn set_auto_download_and_install_updates<R: Runtime>(
    app: AppHandle<R>,
    _manager: State<'_, UpdateManager>,
    enabled: bool,
) -> Value {
    if let Some(manager) = app.try_state::<UpdateManager>() {
        manager.auto_download.store(enabled, Ordering::SeqCst);
    }
    json!({ "success": true })
}

/// 供壳的 `extra_handler` 按命令名派发。
pub fn handler<R: Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        get_update_state,
        download_update_command,
        cancel_update_download,
        quit_and_install_update_command,
        skip_update_version,
        get_auto_update_preferences,
        set_auto_download_and_install_updates,
    ]
}
