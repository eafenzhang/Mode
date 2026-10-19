//! `mode-tauri` 的库面：给主二进制与 opt-in harness bin 共用的壳部件。
//!
//! 为什么要有 lib 目标：`src/bin/host_roundtrip_harness.rs`（opt-in 的真 webview 往返）必须
//! 用**同一份**窗口装配与 Host 接线，否则它验的就不是产品路径。两个 bin 都在本包内，
//! 走 lib 面共享即可，不需要把代码挪进 `crates/**`。
//!
//! 这里**不放业务状态**：窗口与进程调度属壳（`AGENTS.md` 的「进程、协议与远程控制」），
//! 任务/会话状态归 Host（`crates/sidecar-host` 拉起的 Node sidecar）。

use tauri::Runtime;

pub mod host;
pub mod single_instance;
pub mod window;
pub mod window_controls;

pub use host::{resolve_config, spawn_host, COMMAND_NAMES};
pub use window::{build_host_window, DEFAULT_WINDOW_SIZE, MAIN_WINDOW_LABEL};
pub use window_controls::COMMAND_NAMES as WINDOW_CONTROL_COMMAND_NAMES;

/// 壳侧全部追加命令的**唯一** `extra` 处理器（`platform::commands::init_with_extra` 要它）。
///
/// 为什么需要这一层合成：`invoke_handler` 是覆盖语义、只能有一个（M1 的 smoke 吃过亏，见
/// `main.rs` 的模块文档），而 `generate_handler!` 生成的闭包吃 `Invoke<R>` 的**所有权**
/// ——没法把它串起来调两次。所以按**命令名**派发：命中窗口控制面就交给它，否则交给 Host 通道。
///
/// 顺序是确定的：窗口控制的命令名与 Host 通道的四个名字不重叠（两个清单都由
/// `crates/sidecar-host/src/bridge.rs` 与 `window_controls.rs` 各自的守卫盯着），
/// 所以这里是「先看谁认得」，不是「谁先谁赢」。
pub fn extra_handler<R: Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static
{
    let window_controls = window_controls::handler::<R>();
    let update_state = updater::state::handler::<R>();
    let host = host::handler::<R>();
    move |invoke| {
        if updater::state::COMMAND_NAMES.contains(&invoke.message.command()) {
            update_state(invoke)
        } else if window_controls::COMMAND_NAMES.contains(&invoke.message.command()) {
            window_controls(invoke)
        } else {
            host(invoke)
        }
    }
}
