//! 微信 iLink 请求面（W5 第二波）。
//!
//! 行为真源：`packages/services/src/bots/providers/weixinProvider.ts` 与
//! `weixinChannelRuntime.ts`（逐条对齐，file:line 见各模块注释）。
//! 协议事实源与边界（入站语义解析留在 TS callback 管线）：
//! `docs/specs/bots-im-transport.md` §4b。iLink 无官方公开文档。

#![forbid(unsafe_code)]

pub mod aes;
pub mod classify;
pub mod headers;
pub mod http;
pub mod send;
pub mod typing;
pub mod updates;

pub use aes::aes_key_from_metadata;
pub use aes::decrypt_cdn_media;
pub use classify::{classify_failure, WeixinError, WeixinFailureCode};
pub use headers::{
    build_headers, build_uin_header_value, DEFAULT_WEIXIN_ILINK_BASE_URL, WEIXIN_BOT_API_PREFIX,
    WEIXIN_CHANNEL_VERSION, WEIXIN_GET_UPDATES_TIMEOUT_MS, WEIXIN_MESSAGE_STATE_FINISH,
    WEIXIN_MESSAGE_TYPE_BOT,
};
pub use http::{parse_weixin_response, ReqwestWeixinHttp, WeixinHttp, WeixinRequest};
pub use send::{build_client_id, build_send_message, outbound_text, plain_text_reply};
pub use typing::{request_typing, TypingTickets};
pub use updates::{get_updates, next_buf_of, read_messages, UpdatesResult};
