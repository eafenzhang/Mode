//! 原生媒体面（spec: docs/specs/lan-remote-desktop-native-media.md）。
pub mod capture;
pub mod encode;
pub mod napi;
pub mod state;

// 契约 A 的测试缝：stop 空跑幂等（真实 DUPL 在无桌面会话的 CI 可能不可用）。
pub use napi::stop_for_tests;
