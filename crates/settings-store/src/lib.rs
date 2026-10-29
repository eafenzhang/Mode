//! settings 的文件层（W1-1a）。
//!
//! 只负责「找到文件、读出来、坏了隔离、写回去」；**schema 校验与合并规则不在这里**——
//! 它依赖 `AppSettings` 的字段表，属下一步（W1-1b）。行为真源：
//! `packages/services/src/setting/settingService.ts` 的 `resolveUserHomeDir` /
//! `getSettingsDir` / `readSettingsWithMeta` / `writeSettings`。

pub mod data_dir;
pub mod document;
pub mod paths;
pub mod project;
pub mod store;

pub use data_dir::{copy_data_directory, is_path_equal_or_inside, update_data_base_dir, validate_data_base_dir_target, DataDirError};
pub use document::{allowed_keys, apply_patch, defaults, merge_persisted, normalize};
pub use paths::{resolve_home, resolve_settings_paths, HomeEnv, SettingsPaths};
pub use project::{ensure_default_project, DefaultProject, DEFAULT_PROJECT_NAME};
pub use store::{read_document, write_document, ReadOptions, ReadOutcome};
