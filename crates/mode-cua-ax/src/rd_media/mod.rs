//! 原生媒体面（spec: docs/specs/lan-remote-desktop-native-media.md）。
pub mod capture;
pub mod encode;
pub mod napi;
pub mod state;

// 契约 A 的测试缝：stop 空跑幂等（真实 DUPL 在无桌面会话的 CI 可能不可用）。
pub use napi::stop_for_tests;

/// 稀有异常路径的最小诊断面（本 crate 无日志设施）：只在 worker 错误退出、
/// stop 超时、switch/光标降级这类一次性事件打 stderr；不写帧内容/密钥/凭据。
pub(crate) fn warn(msg: &str) { eprintln!("[rd_media] {msg}"); }
