//! 插件归档同步（`plugin-sync-service`）：候选扫描、远端判重与写权限探测。
//!
//! 真源：`packages/services/src/plugin-sync/pluginSyncService.ts`（clean-room，只读本仓库 TS）。
//! 本 crate 先覆盖三个本地方法；归档导出/导入与 marketplace source 在后续切片补齐。
//! 行为契约见 `docs/specs/plugin-sync-service.md`。

mod candidates;
mod paths;
mod statuses;
mod types;
mod write_access;

use std::path::{Path, PathBuf};

pub use candidates::{
    collect_configured_inline_plugin_path_by_id, collect_local_user_plugin_candidates,
    compute_recursive_size, create_short_hash, normalize_plugin_id, normalize_plugin_id_key,
    read_plugin_manifest_info, read_user_plugin_config_state, PluginManifestInfo,
    UserPluginConfigState,
};
pub use paths::{
    default_home_dir, normalize_relative_path, resolve_user_path, resolve_within,
    user_mode_config_path, user_mode_plugin_root, MODE_DATA_ROOT_DIR_NAME,
};
pub use statuses::{list_remote_user_plugin_statuses, RemoteStatusQuery};
pub use types::{
    PluginSyncCandidate, PluginSyncCandidateListResult, PluginSyncComponentType,
    PluginSyncRemoteStatus, PluginSyncRemoteStatusReason, PluginSyncRemoteStatusResult,
    RemoteSyncWriteAccessResult,
};
pub use write_access::check_remote_user_plugin_write_access;

pub const DEFAULT_MAX_ARCHIVE_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PluginSyncServiceOptions {
    pub home_dir: PathBuf,
    pub max_archive_bytes: u64,
}

impl PluginSyncServiceOptions {
    pub fn new(home_dir: impl Into<PathBuf>) -> Self {
        Self {
            home_dir: home_dir.into(),
            max_archive_bytes: DEFAULT_MAX_ARCHIVE_BYTES,
        }
    }
}

pub struct PluginSyncService {
    home_dir: PathBuf,
    max_archive_bytes: u64,
}

impl PluginSyncService {
    pub fn new(options: PluginSyncServiceOptions) -> Self {
        Self {
            home_dir: options.home_dir,
            max_archive_bytes: options.max_archive_bytes,
        }
    }

    pub fn with_default_home() -> Self {
        Self::new(PluginSyncServiceOptions::new(default_home_dir()))
    }

    pub fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    pub fn list_local_user_plugin_candidates(
        &self,
    ) -> Result<PluginSyncCandidateListResult, String> {
        Ok(PluginSyncCandidateListResult {
            candidates: collect_local_user_plugin_candidates(&self.home_dir)?,
            max_archive_bytes: self.max_archive_bytes,
        })
    }

    pub fn list_remote_user_plugin_statuses(
        &self,
        plugins: &[RemoteStatusQuery],
    ) -> Result<PluginSyncRemoteStatusResult, String> {
        Ok(PluginSyncRemoteStatusResult {
            statuses: list_remote_user_plugin_statuses(&self.home_dir, plugins)?,
        })
    }

    pub fn check_remote_user_plugin_write_access(&self) -> RemoteSyncWriteAccessResult {
        check_remote_user_plugin_write_access(&self.home_dir)
    }
}
