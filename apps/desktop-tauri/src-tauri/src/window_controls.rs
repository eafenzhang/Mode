//! 窗口控制面（M3 Task 4 fix round）：修「自绘标题栏点了没反应，且系统标题栏还在」。
//!
//! 契约来自 `IPlatformService` 的两个成员（**只读接口面**：声明与文档注释，不读实现体）：
//!
//! - `executeDesktopCommand(command: DesktopCommandId): Promise<unknown>`（**必选**）——
//!   「执行桌面窗口级命令（标题栏菜单、缩放、窗口控制等）」。**返回类型是 `unknown` 而不是
//!   `void`**（真源：`packages/shared/src/platform.ts`），所以下面用
//!   `{ success: false, error }` 表达「这个 id 我做不到」是**形状内**的取值；
//! - `getDesktopWindowChromeState?(): Promise<DesktopWindowChromeState>`（可选）——返回
//!   `{ isMaximized, macOSMajorVersion?, supportsNativeRoundedCorners }`。
//!
//! 产品 UI 的自绘标题栏正是这两个的调用方：三个按钮分别发 `DesktopCommandIds` 的
//! `minimizeWindow` / `toggleMaximizeWindow` / `closeWindow`，并在挂载时读一次 chrome 状态决定
//! 画「最大化」还是「还原」图标。**在 Tauri 壳里它们是死的**——注入层把 `executeDesktopCommand`
//! 归在「没有要读的值」的降级空操作里，`getDesktopWindowChromeState` 干脆缺席（可选成员）。
//! 于是「点了没反应」，而系统标题栏还画在同一个位置上（`tauri.conf.json` 里没有任何
//! `decorations` 设置），用户看到**两条**标题栏。本模块与 `window.rs` 的 `decorations(false)`
//! 一起修这两半——只修一半都比现状更糟：只接线不去掉系统栏是两条栏，只去掉系统栏不接线是
//! 一条死栏（还可能连拖动都没了，拖动见 `window.rs` 的注释）。
//!
//! ## 为什么这两个命令不在 M1 的切片表里
//!
//! M1 的十条命令是**跨端契约**（`crates/platform/tests/commands.rs` 扫源码钉住「每条命令都写了
//! `rename` 且与切片表一致」），本任务不许动它。壳侧的追加命令走
//! `platform::commands::init_with_extra` 的 `extra` 入口（M3 Task 1 立的规矩：`invoke_handler`
//! 是覆盖语义，只能有一个），所以这两个命令落在**壳 crate** 里，与
//! `host_send`/`host_recv`/`host_startup_state` 同一个 `invoke_handler`、按命令名派发
//! （见 [`crate::extra_handler`]）。
//!
//! ## 命令 id 的处置（**不假装成功**）
//!
//! `DesktopCommandIds` 有 27 个 id。窗口级的那几个本模块实现：
//! `closeWindow` / `minimizeWindow` / `toggleMaximizeWindow` / `toggleFullScreen` /
//! `resetWindowSize`；`checkForUpdates` 由 M5 Task 1 接到真实的更新链
//! （`updater::command::check_and_install`）。其余 id（`zoomIn`/`zoomOut`/`resetZoom`、
//! `showAbout`、`relaunchApp`、`clearAllData`、`GetCuaOsSupport` …）**不属于窗口控制**：
//! 它们要么是渲染层自己的菜单动作、要么依赖本阶段没接线的能力（CUA 探测 / 数据清理）。
//! 对这些 id 返回 `{ success: false, error: "desktop_command_unsupported: <id>" }` 并在壳的
//! stderr 留一行——声明的返回是 `Promise<unknown>`（「返回值直通 main 进程 handler 的 return」），
//! 所以带 `success: false` 的对象是形状内的取值，既不抛（不会掀翻界面），也不谎报执行过。
use serde_json::{json, Value};
use tauri::{Manager, Runtime, WebviewWindow};

use crate::window::DEFAULT_WINDOW_SIZE;

/// 计时起点（用于诊断行里的 `at_ms`）：由 [`handler`] 在 app 构建期初始化，所以约等于进程启动。
fn started_at() -> &'static std::time::Instant {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now)
}

/// 本模块注册的命令名。
///
/// **这里的字面量是重复的，且已经被守**：`#[tauri::command(rename = "…")]` 只吃字面量，而注入层
/// 按名字 `invoke`——两处漂移的症状是运行期 `Command … not found`（也就是「标题栏又死了」）。
/// 守它的是 `apps/desktop-tauri/test/windowModeInjection.test.mts` 的
/// 「壳侧窗口控制面的 rename 字面量必须与注入名一致」：它读本文件与
/// `crates/sidecar-host/src/bridge.rs` 的 `CMD_*` 常量逐个对拍（两个方向都查）。
pub const COMMAND_NAMES: &[&str] = &[CMD_EXECUTE_DESKTOP_COMMAND, CMD_WINDOW_CHROME_STATE];

/// `executeDesktopCommand` 的 wire 名（注入层按它 `invoke`）。
pub const CMD_EXECUTE_DESKTOP_COMMAND: &str = "executeDesktopCommand";
/// `getDesktopWindowChromeState` 的 wire 名。
pub const CMD_WINDOW_CHROME_STATE: &str = "getDesktopWindowChromeState";

/// 本模块的命令，交给 [`crate::extra_handler`] 与 host 的命令合成同一个 `invoke_handler`。
pub fn handler<R: Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    // 提前把计时起点钉住：`started_at()` 是惰性的，第一次调用才初始化的话，**第一条诊断行
    // 恒是 `at_ms=0`**（那正是最需要看清「是谁在什么时候调的」的那一条）。handler 在 app 构建期
    // 就被调用，所以这里初始化得到的起点≈进程启动。
    let _ = started_at();
    tauri::generate_handler![execute_desktop_command, get_desktop_window_chrome_state]
}

/// `IPlatformService.executeDesktopCommand(command)`。
///
/// `window` 由 Tauri 按**调用方的窗口**注入（不用 `get_webview_window("main")` 猜：
/// 调用来自哪个窗口就作用在哪个窗口上，这在本阶段是同一个窗口，但语义不靠猜）。
#[tauri::command(rename = "executeDesktopCommand")]
pub async fn execute_desktop_command<R: Runtime>(
    window: WebviewWindow<R>,
    command: String,
) -> Value {
    // 更新命令是唯一的异步分支：它要走网络、下载并启动安装器。成功时安装器会带走进程，
    // 失败时返回 `{status:"error"}` 且旧版本继续运行（见 `updater::command` 的模块文档）。
    if command == updater::command::CHECK_FOR_UPDATES_COMMAND {
        let at_ms = started_at().elapsed().as_millis();
        eprintln!(
            "[mode-tauri] window_control command={command} window={} routed=updater at_ms={at_ms}",
            window.label()
        );
        return updater::command::check_and_install(window.app_handle()).await;
    }

    let outcome = apply_window_command(&window, &command);
    // 控制台留痕：这是「自绘标题栏的按钮有没有真的走到壳」的唯一诊断面
    // （用户点按钮时没有别的可观察点），也是 opt-in 用例的判据之一。
    //
    // `at_ms` 是**进程启动以来的毫秒数**：同一条链路里可能有两个调用方（用户/渲染层的启动恢复
    // 与测试探针），只有时间能区分它们——本轮就是靠它才看清「启动后 1 秒那次 toggle 不是探针发的」。
    let at_ms = started_at().elapsed().as_millis();
    let maximized = window.is_maximized().unwrap_or(false);
    match &outcome {
        Ok(()) => eprintln!(
            "[mode-tauri] window_control command={command} window={} ok=true at_ms={at_ms} is_maximized={maximized}",
            window.label()
        ),
        Err(reason) => eprintln!(
            "[mode-tauri] window_control command={command} window={} ok=false reason={reason} at_ms={at_ms}",
            window.label()
        ),
    }
    match outcome {
        Ok(()) => json!({ "success": true }),
        Err(reason) => json!({ "success": false, "error": reason }),
    }
}

/// 把窗口级命令落到窗口上。
fn apply_window_command<R: Runtime>(window: &WebviewWindow<R>, command: &str) -> Result<(), String> {
    let io = |error: tauri::Error| error.to_string();
    match command {
        "closeWindow" => window.close().map_err(io),
        "minimizeWindow" => window.minimize().map_err(io),
        // 切换语义：当前最大化就还原、否则最大化。**用窗口自己的状态判断**，
        // 不让调用方传「我现在是不是最大化」——那个值会与真实状态漂移。
        "toggleMaximizeWindow" => {
            if window.is_maximized().map_err(io)? {
                window.unmaximize().map_err(io)
            } else {
                window.maximize().map_err(io)
            }
        }
        "toggleFullScreen" => {
            let next = !window.is_fullscreen().map_err(io)?;
            window.set_fullscreen(next).map_err(io)
        }
        // 「重置窗口大小」：退出全屏与最大化，再回到初始逻辑尺寸（与建窗时同一个常量）。
        "resetWindowSize" => {
            window.set_fullscreen(false).map_err(io)?;
            window.unmaximize().map_err(io)?;
            window
                .set_size(tauri::LogicalSize::new(
                    DEFAULT_WINDOW_SIZE.0,
                    DEFAULT_WINDOW_SIZE.1,
                ))
                .map_err(io)
        }
        other => Err(format!("desktop_command_unsupported: {other}")),
    }
}

/// `IPlatformService.getDesktopWindowChromeState()`。
///
/// 三个字段各自的口径（**只有第一个有消费者**，另外两个如实标注）：
///
/// - `isMaximized`：窗口的真实状态（自绘标题栏按它决定画哪个图标）；
/// - `macOSMajorVersion`：**非 macOS 一律 `null`**（声明里就是这么写的：「非 macOS 或无法解析时
///   为 null」）；
/// - `supportsNativeRoundedCorners`：Windows 上报 `false`——壳没有「本机是否会替我们画圆角」的
///   可靠判据（那是 DWM 的实现细节，没有对应的查询面），而渲染层拿它决定**要不要自己画圆角**。
///   报 `true` 会在不支持的系统上让窗口变成直角（撒谎），报 `false` 只是让渲染层自绘——后者在
///   两种系统上都成立，所以这不是「默认值」，是**唯一不会说谎的取值**。
#[tauri::command(rename = "getDesktopWindowChromeState")]
pub fn get_desktop_window_chrome_state<R: Runtime>(window: WebviewWindow<R>) -> Value {
    let is_maximized = window.is_maximized().unwrap_or(false);
    json!({
        "isMaximized": is_maximized,
        "macOSMajorVersion": Value::Null,
        "supportsNativeRoundedCorners": false,
    })
}
