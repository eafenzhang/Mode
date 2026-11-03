//! 终端 PTY 会话管理（spec `terminal-pty-in-shell.md`）：PTY 进程下沉到 Tauri 壳。
//!
//! 管理器已抽到独立 crate `terminal-session`，供壳命令面与 `terminal` 频道共用；
//! 本模块只保留原有公开路径，避免命令面与测试改导入。

pub use terminal_session::{
    TerminalCreateResult, TerminalEmit, TerminalEvent, TerminalSessionManager,
};
