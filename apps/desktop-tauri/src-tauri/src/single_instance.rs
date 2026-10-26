//! 单实例：第二个进程把深链交回已运行的实例，然后**不建窗口、不建 Host** 直接退出。
//!
//! 规格：`docs/specs/tauri-platform-commands.md` 的单实例一节（DR-T4-1）；
//! 深链侧的投递实现在 `crates/deeplink`（本模块只接线）。
//!
//! ## 为什么这条是窗口模型的事，不是「加个锁」
//!
//! Windows / Linux 上「应用已在运行 → 用户点 `mode://` 链接」一定会**起一个新进程**（链接在 argv 里）。
//! 新进程若走完整启动路径就会再建一个 Host、再建一个窗口，而 `AGENTS.md` 的口径是
//! 「每个窗口一个 window-scoped Local Host、本地 workspace 共享该 Host」——第二个 Host
//! 意味着第二份 workspace 状态，与那条口径相冲。
//!
//! ## 早退为什么在结构上成立
//!
//! `tauri-plugin-single-instance` 的早退发生在**插件 setup 钩子**里，而 Tauri 的顺序是
//! 「插件 setup 钩子（`Builder::build` 期间的 `initialize_plugins`）→ 事件循环 Ready → app 的
//! `setup` 闭包」。本壳的**窗口与 Host 都只在 app `setup` 里建**，所以第二实例在钩子里
//! `process::exit(0)` 时，二者都还不存在——这不是靠自觉，是构造上的先后顺序。
//!
//! ## 转发通道：复用既有的深链事件通道
//!
//! 插件只负责「第二个进程把 argv 交给第一个进程」（Windows 上是命名互斥体 + `WM_COPYDATA`，
//! 由插件实现，本壳没有另造任何进程间通道）。argv 落到 [`handoff`] 之后走的是
//! `deeplink::forward_second_instance_args` → 同一个 `DeepLinkRouter` → 同一个
//! `mode:deep-link` 事件 —— 也就是说**渲染层侧的通道与 M1 的深链完全一致**，没有新增事件名、
//! 没有新增命令、没有新增 IPC 面。
//!
//! ## 本模块刻意不做的事（代价写在 spec 里）
//!
//! - **不重构窗口管理**：只聚焦既有主窗口（否则用户再点一次图标会因为「什么都没发生」而
//!   以为应用坏了）。多窗口的窗口模型（M4）要重新设计这一层，本任务只保证
//!   「同一个 workspace 不重复起 Host」。
//! - **不做启动互斥之外的进程管理**：第二进程的清理由插件自己完成（`cleanup_before_exit` +
//!   `exit(0)`），此处不介入。
use tauri::{AppHandle, Manager, Runtime};

use crate::window::MAIN_WINDOW_LABEL;

/// 注册单实例插件。
///
/// **必须是 builder 上的第一个插件**：插件按注册顺序执行 setup 钩子，而第二实例的早退就在钩子里
/// ——注册晚了（或漏了），第二个进程就会走完 app setup，建出第二个窗口与第二个 Host。
/// 这条顺序由 `apps/desktop-tauri/test/singleInstance.test.mts` 的静态守卫与真机用例一起钉住。
pub fn install<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.plugin(tauri_plugin_single_instance::init(handoff::<R>))
}

/// 第二个进程的 argv 到达本进程（**本进程是第一个实例**）：转发深链 + 聚焦既有窗口。
///
/// `cwd` 由插件一并转来，本壳不用它（没有依赖相对路径的启动逻辑），故忽略。
fn handoff<R: Runtime>(app: &AppHandle<R>, args: Vec<String>, _cwd: String) {
    // ① 深链：走既有的路由与事件通道（见模块文档）。没有链接时它返回 `None`——
    //    那就是「用户又点了一次图标」，只聚焦窗口。
    let forwarded = deeplink::forward_second_instance_args(app, &args);
    match forwarded {
        Some(url) => eprintln!("[single-instance] 第二个实例的深链已转入：{url}"),
        None => eprintln!("[single-instance] 第二个实例没有深链（只聚焦既有窗口）"),
    }
    // ② 聚焦既有主窗口。窗口不存在时**什么都不做**：这里绝不能去建一个窗口——
    //    「第二实例不建窗口」是这条改动的全部意义。
    focus_existing_main_window(app);
}

/// 显示、还原并聚焦既有主窗口；窗口不在时静默返回。
///
/// 两个消费者：单实例二次启动（点程序图标）、通知点击（spec 平台命令表第 8 行）——
/// 同一个「让主窗口回到用户面前」的语义，只有一份实现。
pub fn focus_existing_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        eprintln!("[single-instance] 主窗口 {MAIN_WINDOW_LABEL} 不在，无法聚焦（不新建）");
        return;
    };
    // 失败只可能因为没有窗口系统支持（例如无桌面会话）：诊断留痕，不影响转发已经完成的事实。
    //
    // `show()` 不能省：关闭到托盘把窗口 `hide()` 掉了（实测 bounds 变成 -32000 的隐藏位），
    // 而 `unminimize()` 只处理「最小化」这一种非可见态——2026-10-26 真人模拟实测：托盘模式
    // 下双击程序图标，进程被单实例接管后窗口**仍然隐藏**，用户看到的是「点了没反应」。
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}
