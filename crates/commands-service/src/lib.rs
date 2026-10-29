//! `commands` 频道的服务实现（W3 下一域）。
//!
//! 行为真源：
//! - `packages/services/src/commands/commandsService.ts`（目录来源优先级、读写、插件命令）
//! - `packages/services/src/commands/commandFileParser.ts`（markdown frontmatter 手写解析）
//! - `packages/services/src/plugins/installedPluginRoots.ts`
//! - `packages/shared/src/command-types.ts` / `settings-source.ts`
//! - `packages/shared/src/plugin-marketplaces.ts`（只取命令发现需要的三个符号）
//! - `packages/shared/src/node/officialPluginCache.ts`（只取 cache 根回退）
//!
//! 规范见 `docs/specs/commands-service.md`。
//!
//! 服务是同步实现：命令目录都是小文件，读-改-写的等待上限由 `atomic-file` 的跨进程锁决定，
//! 因此由频道适配器统一丢进 `spawn_blocking`（与 `credential` 频道同口径）。

mod descriptor;
mod error;
mod parser;
mod plugin;
mod service;
mod types;

pub use descriptor::{
    command_agent_sources, command_file_name, command_name, get_command_source_descriptor,
    CommandAgentSourceDescriptor, COMMAND_DIRECTORY_SOURCE_DESCRIPTORS, MODE_COMMAND_AGENT_SOURCE,
};
pub use error::{commands_error, CommandsResult, CommandsServiceError};
pub use parser::{
    generate_command_file_content, parse_command_file, ParsedCommandFile, REPLACED_FRONTMATTER_KEYS,
};
pub use plugin::{
    canonical_plugin_id, is_default_enabled_official_plugin_id,
    read_installed_plugin_roots, resolve_official_plugin_cache_root, InstalledPluginRoot,
    DEFAULT_ENABLED_OFFICIAL_PLUGIN_IDS, MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID,
    MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID,
};
pub use service::{resolve_user_home_dir, CommandsService, CommandsServiceOptions};
pub use types::*;
