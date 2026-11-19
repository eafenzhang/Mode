//! 企微智能机器人 WS 客户端（W5 第四波）。
//!
//! wire 事实源：`@wecom/aibot-node-sdk`（MIT）的 `dist/index.cjs.js`——按
//! `docs/specs/clean-room-policy.md` 的第三方厂商 SDK 接口面口径提取，事实与
//! file:line 引用见 `docs/specs/bots-im-transport.md` §4d/§6。
//! 实现只依据该 spec；本 crate 不读 SDK 代码结构。

#![forbid(unsafe_code)]

pub mod api;
pub mod connection;
pub mod download;
pub mod frame;
pub mod reply;

pub use api::{plan_upload, UploadResult, WeComClient, UPLOAD_CHUNK_SIZE, UPLOAD_MAX_CHUNKS};
pub use connection::{
    set_log_hook, spawn_connection, ConnectionCallbacks, RunOutcome, StatusUpdate, WeComConfig,
    DEFAULT_HEARTBEAT_INTERVAL, DEFAULT_MAX_AUTH_FAILURE_ATTEMPTS, DEFAULT_MAX_RECONNECT_ATTEMPTS,
    DEFAULT_RECONNECT_BASE, DEFAULT_WS_URL, MAX_MISSED_PONG, RECONNECT_MAX_DELAY,
};
pub use download::{decrypt_file, parse_filename, FileDownloader, ReqwestFileDownloader};
pub use frame::{
    classify, generate_req_id, outbound_frame, Frame, FrameHeaders, Inbound, CMD_CALLBACK,
    CMD_EVENT_CALLBACK, CMD_HEARTBEAT, CMD_RESPONSE, CMD_RESPONSE_UPDATE, CMD_RESPONSE_WELCOME,
    CMD_SEND_MSG, CMD_SUBSCRIBE, CMD_UPLOAD_MEDIA_CHUNK, CMD_UPLOAD_MEDIA_FINISH,
    CMD_UPLOAD_MEDIA_INIT, EVENT_DISCONNECTED, EVENT_ENTER_CHAT, EVENT_FEEDBACK,
    EVENT_TEMPLATE_CARD, MSG_FILE, MSG_IMAGE, MSG_MIXED, MSG_TEXT, MSG_VIDEO, MSG_VOICE,
};
pub use reply::{AckResult, ReplyBroker, MAX_REPLY_QUEUE_SIZE, REPLY_ACK_TIMEOUT};
