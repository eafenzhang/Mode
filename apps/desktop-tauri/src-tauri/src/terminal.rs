//! 终端命令面（spec `terminal-pty-in-shell.md`）：PTY 进程下沉到壳。
//!
//! 会话管理器（`platform::terminal::TerminalSessionManager`）是**纯逻辑**（本机可测）；
//! 本文件只有薄命令：从 `State` 取 manager、把事件出口桥到 `app.emit`。渲染层的
//! shell-backed `ITerminalService`（`packages/desktop/src/renderer/src/desktopTerminalBridge.ts`）
//! 按 `COMMAND_NAMES` 逐字调用；服务事件用 Tauri event（`terminal://data` / `terminal://exit`）。
use std::sync::Arc;

use platform::terminal::{TerminalEvent, TerminalSessionManager};
use serde::Serialize;
use tauri::{Emitter, Runtime, State};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TerminalDataEvent {
    id: String,
    data: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TerminalExitEvent {
    id: String,
    code: i64,
}

/// 本模块注册的命令名（与渲染层 `desktopTerminalBridge.ts` 的 invoke 同源）。
pub const COMMAND_NAMES: &[&str] = &[
    "terminalCreate",
    "terminalWrite",
    "terminalResize",
    "terminalDispose",
];

#[tauri::command(rename = "terminalCreate")]
pub fn terminal_create(
    state: State<'_, Arc<TerminalSessionManager>>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    shell: String,
) -> Result<serde_json::Value, String> {
    let id = state.create(cols, rows, cwd.as_deref(), &shell)?;
    Ok(serde_json::json!({ "id": id }))
}

#[tauri::command(rename = "terminalWrite")]
pub fn terminal_write(
    state: State<'_, Arc<TerminalSessionManager>>,
    id: String,
    data: String,
) -> Result<(), String> {
    state.write(&id, &data)
}

#[tauri::command(rename = "terminalResize")]
pub fn terminal_resize(
    state: State<'_, Arc<TerminalSessionManager>>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    state.resize(&id, cols, rows)
}

#[tauri::command(rename = "terminalDispose")]
pub fn terminal_dispose(
    state: State<'_, Arc<TerminalSessionManager>>,
    id: String,
) -> Result<(), String> {
    state.dispose(&id)
}

/// 本模块注册的 handler（命令面）。
pub fn handler<R: Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        terminal_create,
        terminal_write,
        terminal_resize,
        terminal_dispose
    ]
}

/// 在 setup 里构造 manager：事件出口桥到 `app.emit`（`terminal://data` / `terminal://exit`）。
pub fn create_manager<R: Runtime>(app: &tauri::AppHandle<R>) -> Arc<TerminalSessionManager> {
    let emit_app = app.clone();
    let emit: TerminalEventEmit = Arc::new(move |event| match event {
        TerminalEvent::Data { id, data } => {
            let _ = emit_app.emit("terminal://data", TerminalDataEvent { id, data });
        }
        TerminalEvent::Exit { id, code } => {
            let _ = emit_app.emit("terminal://exit", TerminalExitEvent { id, code });
        }
    });
    Arc::new(TerminalSessionManager::new(emit))
}

type TerminalEventEmit = Arc<dyn Fn(TerminalEvent) + Send + Sync>;
