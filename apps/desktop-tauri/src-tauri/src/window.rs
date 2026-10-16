//! 窗口装配：**必须在 Rust 侧建窗口**，否则挂不上注入脚本。
//!
//! 依据（spike §1 第 2 条）：`tauri.conf.json` 的 `WindowConfig`（`tauri-utils 2.10.2`
//! `src/config.rs:1952`）**没有** `initialization_script` 字段，配置里的窗口拿不到注入；
//! 只有 `WebviewWindowBuilder::initialization_script` 能让桥在页面脚本之前执行。
//!
//! 因此产品壳的 `app.windows` 是空数组，窗口在这里建（label 仍是 `main`，
//! `capabilities/default.json` 的 `windows: ["main"]` 照旧生效）。

use tauri::{AppHandle, Runtime, WebviewUrl, WebviewWindow};

/// 主窗口的 label（与 `capabilities/default.json` 一致，不要改）。
pub const MAIN_WINDOW_LABEL: &str = "main";

/// 建一个带 Host 桥的窗口。
///
/// `url` 是页面来源：**必须是本地内容（asset 协议）**——远端 `http://` 来源拿不到 app 命令
/// （spike §4.2 实测：`!is_local` 会强制走 ACL，而 app 命令不在 `allowed_commands` 里）。
pub fn build_host_window<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    url: WebviewUrl,
    title: &str,
    startup_id: &str,
) -> tauri::Result<WebviewWindow<R>> {
    tauri::WebviewWindowBuilder::new(app, label, url)
        .title(title)
        .inner_size(1280.0, 800.0)
        .resizable(true)
        .initialization_script(sidecar_host::bridge::bridge_script(startup_id))
        .build()
}
