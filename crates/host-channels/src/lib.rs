//! 壳内频道适配器。
//!
//! 只做「RPC 方法名 + 位置参数」到服务调用的翻译与错误映射；服务本身在各自的 crate 里。
//! 参数形状与 `ProxyChannel` 一致：`arg` 是**位置参数数组**，对象参数以 `Value::Json` 承载。
//!
//! 已适配：`memory`（两个只读方法）、`prompt-attachment-transfer`（本地零拷贝 + 四个 no-op）。

mod params;

pub mod attachment_transfer;
pub mod memory_channel;

pub use attachment_transfer::AttachmentTransferChannel;
pub use memory_channel::{MemoryChannel, LIST_MEMORIES, READ_MEMORY_FILE};

pub const MEMORY_CHANNEL: &str = "memory";
pub const ATTACHMENT_TRANSFER_CHANNEL: &str = "prompt-attachment-transfer";
