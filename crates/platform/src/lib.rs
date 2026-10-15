//! `IPlatformService`（`packages/shared/src/platform.ts`）的 Rust 对应。
//!
//! 规格：`docs/specs/tauri-platform-commands.md`。
//! 取材口径：按 `docs/specs/clean-room-policy.md` §允许输入，只读 `platform.ts` 的
//! **接口面**（方法签名与类型声明），不读实现体；命令名与参数逐字对齐，不做重命名。

pub mod commands;
mod state;

pub use state::AppState;
