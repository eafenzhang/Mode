//! 钉钉 Stream 模式长连客户端（W5 第一波）。
//!
//! 行为真源：`packages/services/src/bots/dingtalkChannelRuntime.ts`（逐条对齐，
//! file:line 见 [`frame::handle_frame`] 与 [`runtime`] 的注释）。
//! 协议事实源：钉钉官方「Stream模式协议接入说明」（open-dingtalk.github.io
//! `docs/learn/stream/protocol`）+ 自家 TS 实现。spec：`docs/specs/bots-im-transport.md`。
//!
//! 第一波是纯 Rust 库，不接产品进程；接入形态（napi / 壳内频道）属后续裁决。

#![forbid(unsafe_code)]

pub mod dedupe;
pub mod frame;
pub mod open;
pub mod runtime;

pub use dedupe::{DedupeEntry, DedupeSet};
pub use frame::{DownFrame, FrameOutcome, UpFrame};
pub use open::{
    build_open_request, HttpStreamOpen, OpenError, OpenRequest, Registration, StreamOpen,
};
pub use runtime::{
    next_backoff_ms, run_connection, set_log_hook, BotConnection, RuntimeCallbacks, RuntimeStatus,
    StatusUpdate, StreamTiming, DEDUPE_PERSIST_DEBOUNCE_MS, HEALTHY_CONNECTION_MS,
    RECONNECT_BACKOFF_MAX_MS, RECONNECT_BACKOFF_MIN_MS, REGISTRATION_RETRY_MS, WS_READ_TIMEOUT_MS,
};
