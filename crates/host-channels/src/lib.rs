//! 壳内频道适配器。
//!
//! 只做「RPC 方法名 + 位置参数」到服务调用的翻译与错误映射；服务本身在各自的 crate 里。
//! 参数形状与 `ProxyChannel` 一致：`arg` 是**位置参数数组**，对象参数以 `Value::Json` 承载。
//!
//! 已适配：`memory`（两个只读方法）、`prompt-attachment-transfer`（本地零拷贝 + 四个 no-op）、
//! `credential`（四个文件方法）、`commands`（6 个命令管理方法）、`skills`（6 个技能方法）、
//! `subagents`（8 个 subagent 管理方法）、`settings-sync`（6 个外部配置同步方法）、
//! `broadcast`（发送 + 5 个 claim 方法 + `onMessage`）、`skill-sync`（5 个归档同步方法）、
//! `git`（18 个仓库操作；提交信息生成依赖 W4）、`git-checkpoint`（4 个 workspace 快照方法）。

mod base64;
mod params;

pub mod attachment_transfer;
pub mod broadcast;
pub mod client_metadata;
pub mod commands;
pub mod conversation_mirror;
pub mod credential;
pub mod file;
pub mod file_watcher;
pub mod git;
pub mod git_checkpoint;
pub mod media_preview;
pub mod setting;
pub mod settings_sync;
pub mod skill_sync;
pub mod skills;
pub mod subagents;
pub mod system;
pub mod memory_channel;

pub use attachment_transfer::AttachmentTransferChannel;
pub use broadcast::BroadcastChannel;
pub use client_metadata::{ClientConfigChannel, ClientScenesChannel};
pub use commands::CommandsChannel;
pub use conversation_mirror::ConversationMirrorChannel;
pub use credential::CredentialChannel;
pub use file::FileChannel;
pub use file_watcher::FileWatcherChannel;
pub use git::GitChannel;
pub use git_checkpoint::GitCheckpointChannel;
pub use media_preview::MediaPreviewChannel;
pub use setting::SettingChannel;
pub use settings_sync::SettingsSyncChannel;
pub use skill_sync::SkillSyncChannel;
pub use skills::SkillsChannel;
pub use subagents::SubagentsChannel;
pub use system::SystemChannel;
pub use memory_channel::{MemoryChannel, LIST_MEMORIES, READ_MEMORY_FILE};

pub const MEMORY_CHANNEL: &str = "memory";
pub const BROADCAST_CHANNEL: &str = "broadcast";
pub const ATTACHMENT_TRANSFER_CHANNEL: &str = "prompt-attachment-transfer";
pub const CLIENT_CONFIG_CHANNEL: &str = "client-config";
pub const CLIENT_SCENES_CHANNEL: &str = "client-scenes";
pub const MEDIA_PREVIEW_CHANNEL: &str = "media-preview";
pub const CONVERSATION_MIRROR_CHANNEL: &str = "conversation-mirror";
pub const SETTING_CHANNEL: &str = "setting";
pub const CREDENTIAL_CHANNEL: &str = "credential";
pub const SYSTEM_CHANNEL: &str = "system";
pub const FILE_CHANNEL: &str = "file";
pub const FILE_WATCHER_CHANNEL: &str = "file-watcher";
pub const COMMANDS_CHANNEL: &str = "commands";
pub const SKILLS_CHANNEL: &str = "skills";
pub const SUBAGENTS_CHANNEL: &str = "subagents";
pub const SETTINGS_SYNC_CHANNEL: &str = "settings-sync";
pub const SKILL_SYNC_CHANNEL: &str = "skill-sync";
pub const GIT_CHANNEL: &str = "git";
pub const GIT_CHECKPOINT_CHANNEL: &str = "git-checkpoint";
