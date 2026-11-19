//! 飞书 WS 长连客户端（W5 第三波）。
//!
//! wire 事实源：`@larksuiteoapi/node-sdk`（MIT）的 `lib/index.js`——按
//! `docs/specs/clean-room-policy.md` 的第三方厂商 SDK 接口面口径提取，事实与
//! file:line 引用见 `docs/specs/bots-im-transport.md` §4c/§5。
//! 实现只依据该 spec；本 crate 不读 SDK 代码结构。

#![forbid(unsafe_code)]

pub mod cache;
pub mod gateway;
pub mod pb;
pub mod runtime;
pub mod session;

pub use cache::DataCache;
pub use gateway::{
    parse_registration, FeishuConfig, FeishuDomain, GatewayError, GatewayErrorCode, PullConfig,
    ReqwestPullConfig, SessionParams,
};
pub use pb::{decode, encode, header_value, Frame, Header, PbError};
pub use runtime::{run_connection, RuntimeTiming};
pub use session::{
    run_session, set_log_hook, EventDispatcher, SessionCallbacks, SessionOutcome, SessionTiming,
    HEADER_BIZ_RT, HEADER_MESSAGE_ID, HEADER_SEQ, HEADER_SUM, HEADER_TYPE, METHOD_CONTROL,
    METHOD_DATA, TYPE_EVENT, TYPE_PING, TYPE_PONG,
};
