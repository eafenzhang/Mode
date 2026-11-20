//! IM bots 域 provider 中立契约（W5 B1 seam）。
//!
//! 规范：`docs/specs/bots-domain-migration.md` §3。四通道 transport crate
//! （dingtalk-stream / weixin-ilink / feishu-ws / wecom-ws）在 B2 起通过本 trait
//! 接入服务层；本 crate 不持连接、不做平台调用。

#![forbid(unsafe_code)]

pub mod adapter;
pub mod types;

pub use adapter::{BoxFutureResult, ProviderAdapter};
pub use types::{
    AttachmentKind, ChatType, InboundAttachment, InboundMessage, MediaKind, OutboundMedia,
    OutboundMessage, ProviderError, ProviderKind, StreamingCardBlock, StreamingCardHandle,
    StreamingCardState, StreamingCardStatus, TypingTarget,
};
