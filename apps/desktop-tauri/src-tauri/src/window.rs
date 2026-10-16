//! 窗口装配：**必须在 Rust 侧建窗口**，否则挂不上注入脚本。
//!
//! 依据（spike §1 第 2 条）：`tauri.conf.json` 的 `WindowConfig`（`tauri-utils 2.10.2`
//! `src/config.rs:1952`）**没有** `initialization_script` 字段，配置里的窗口拿不到注入；
//! 只有 `WebviewWindowBuilder::initialization_script` 能让桥在页面脚本之前执行。
//!
//! 因此产品壳的 `app.windows` 是空数组，窗口在这里建（label 仍是 `main`，
//! `capabilities/default.json` 的 `windows: ["main"]` 照旧生效）。
//!
//! ## `decorations`：产品页无边框、开发页保留系统栏（M3 Task 4 fix round）
//!
//! 产品 UI **自己画标题栏**（`packages/ui` 的 `DesktopWindowControls` 三个按钮 + 若干
//! `[app-region:drag]` 区域），所以系统标题栏必须在产品窗口上去掉——否则用户看到**两条**标题栏，
//! 而上面那条的按钮还是死的（`executeDesktopCommand` 在注入层是降级空操作，见
//! `src/window_controls.rs`）。两者是一件事的两半，必须一起做。
//!
//! **拖动为什么不会因为去掉系统栏而失效**：wry 在 Windows 上无条件开了 WebView2 的非客户区支持
//! （`wry 0.57.0` 的 `src/webview2/mod.rs`：`settings9.SetIsNonClientRegionSupportEnabled(true)`），
//! 于是页面 CSS 里的 `app-region: drag` / `no-drag` 由 WebView2 自己按非客户区处理——
//! 也就是说渲染层声明的那一片可拖区域**不需要任何 Electron 式的宿主配合**。这正是
//! 「不带宽限地去做无边框」的前提；反过来，如果那条设置没开，无边框窗口会变成一块**移不动**的板子
//! ——比死按钮更糟，所以本模块把这条依赖写在注释里而不是让它隐式存在。
//!
//! **例外**：M1 的命令面验收台（`MODE_TAURI_SHELL_ENTRY=dev/index.html`）与集成测试页
//! **保留系统栏**——那两页是测试资产，没有自绘标题栏，去掉系统栏会让它们既移不动也关不掉
//! （开发期很痛）。判据由调用方给 `decorations`，不在这里按 URL 猜。

use tauri::{AppHandle, Runtime, WebviewUrl, WebviewWindow};

/// 主窗口的 label（与 `capabilities/default.json` 一致，不要改）。
pub const MAIN_WINDOW_LABEL: &str = "main";

/// 初始逻辑尺寸（`resetWindowSize` 也回到这个尺寸，两处同源）。
pub const DEFAULT_WINDOW_SIZE: (f64, f64) = (1280.0, 800.0);

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
    decorations: bool,
) -> tauri::Result<WebviewWindow<R>> {
    let mut builder = tauri::WebviewWindowBuilder::new(app, label, url)
        .title(title)
        .inner_size(DEFAULT_WINDOW_SIZE.0, DEFAULT_WINDOW_SIZE.1)
        .resizable(true)
        // 见模块文档：产品页（自绘标题栏）无边框；测试/开发页保留系统栏。
        .decorations(decorations)
        .initialization_script(injection);
    if let Some(script) = extra_init_script {
        builder = builder.initialization_script(script);
    }
    builder.build()
}
