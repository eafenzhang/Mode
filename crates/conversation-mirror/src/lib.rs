//! 远程工作区对话镜像（`conversation-mirror` 频道的数据层）。
//!
//! 真源：`packages/shared/src/conversation-mirror.ts` 与
//! `packages/services/src/conversation-mirror/*`。服务层只做**存储**：接收快照、按工作区路径
//! 读取全部快照，不读会话内容、不启动 runtime。

pub mod model;
pub mod service;
pub mod store;

pub use model::{
    bound_record, bound_snapshot, dedupe_records, record_byte_size, snapshot_byte_size,
    validate_snapshot, Message, Record, Snapshot, ValidationError, MAX_RECORDS,
    MAX_RECORD_BYTES, MAX_SNAPSHOT_BYTES,
};
pub use service::{ConversationMirrorService, MirrorIdentity, PublishResult};
pub use store::ConversationMirrorStore;
