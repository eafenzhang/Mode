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
///
/// `injection` 是**要注入的那段脚本**，由调用方选：
/// [`sidecar_host::bridge::bridge_script`]（产品：`window.mode` + Host 桥）或
/// [`sidecar_host::bridge::host_bridge_script`]（只撤掉 `window.mode` 的负向对照片，见那边的注释）。
/// 让调用方交脚本而不是交 startupId，是为了让「注入什么」在选择点可见——负向对照才表达得出来。
///
/// `extra_init_script` 追加一段注入脚本（排在 `injection` 之后、页面脚本之前）。产品壳传 `None`；
/// 真产品 UI 的 opt-in 集成测试用它挂一个**探针**（页面本身是 renderer 的产物，不能改，
/// 详见 `docs/specs/tauri-renderer-host.md` 的覆盖分工）。追加而不是替换，
/// 所以「注入先到、探针后到」这个相对顺序是确定的。
pub fn build_host_window<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
    url: WebviewUrl,
    title: &str,
    injection: &str,
    extra_init_script: Option<&str>,
) -> tauri::Result<WebviewWindow<R>> {
    let mut builder = tauri::WebviewWindowBuilder::new(app, label, url)
        .title(title)
        .inner_size(1280.0, 800.0)
        .resizable(true)
        .initialization_script(injection);
    if let Some(script) = extra_init_script {
        builder = builder.initialization_script(script);
    }
    builder.build()
}
