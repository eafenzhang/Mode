//! `mode-tauri` 的库面：给主二进制与 opt-in harness bin 共用的壳部件。
//!
//! 为什么要有 lib 目标：`src/bin/host_roundtrip_harness.rs`（opt-in 的真 webview 往返）必须
//! 用**同一份**窗口装配与 Host 接线，否则它验的就不是产品路径。两个 bin 都在本包内，
//! 走 lib 面共享即可，不需要把代码挪进 `crates/**`。
//!
//! 这里**不放业务状态**：窗口与进程调度属壳（`AGENTS.md` 的「进程、协议与远程控制」），
//! 任务/会话状态归 Host（`crates/sidecar-host` 拉起的 Node sidecar）。

pub mod host;
pub mod window;

pub use host::{handler as host_handler, resolve_config, spawn_host, COMMAND_NAMES};
pub use window::{build_host_window, MAIN_WINDOW_LABEL};
