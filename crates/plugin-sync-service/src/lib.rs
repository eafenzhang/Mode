//! 插件归档同步（`plugin-sync-service`）：候选扫描、远端判重、写权限与归档导入导出。
//!
//! 真源：`packages/services/src/plugin-sync/pluginSyncService.ts`（clean-room，只读本仓库 TS）。
//! marketplace source 归档在后续切片补齐；行为契约见
//! `docs/specs/plugin-sync-service.md`。

mod archive;
mod candidates;
mod paths;
mod plugins_archive;
mod statuses;
mod types;
mod write_access;

use std::path::{Path, PathBuf};

pub use archive::{
    create_archive, extract_archive, ArchiveEntry, ArchiveMetadata, ArchiveMetadataPlugin,
    PLUGIN_SYNC_METADATA_ARCHIVE_PATH,
};
pub use plugins_archive::{import_plugins_archive, read_archive_metadata};

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
    PluginSyncArchiveExportResult, PluginSyncArchivePlugin, PluginSyncCandidate,
    PluginSyncCandidateListResult, PluginSyncComponentType, PluginSyncImportResult,
    PluginSyncImportResultItem, PluginSyncImportStatus, PluginSyncRemoteStatus,
    PluginSyncRemoteStatusReason, PluginSyncRemoteStatusResult, RemoteSyncWriteAccessResult,
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

    pub fn export_plugins_archive(
        &self,
        plugin_ids: &[String],
    ) -> Result<PluginSyncArchiveExportResult, String> {
        let candidates = collect_local_user_plugin_candidates(&self.home_dir)?;
        let mut selected = Vec::with_capacity(plugin_ids.len());
        for plugin_id in plugin_ids {
            let candidate = candidates
                .iter()
                .find(|candidate| &candidate.id == plugin_id)
                .ok_or_else(|| format!("plugin sync candidate not found: {plugin_id}"))?;
            selected.push(candidate);
        }
        let selected_bytes: u64 = selected.iter().map(|candidate| candidate.size_bytes).sum();
        if selected_bytes > self.max_archive_bytes {
            return Err(format!(
                "plugin sync archive exceeds limit: {selected_bytes}/{}",
                self.max_archive_bytes
            ));
        }
        let entries: Vec<ArchiveEntry> = selected
            .iter()
            .map(|candidate| ArchiveEntry::Source {
                source_path: plugin_discovery::js::realpath(Path::new(&candidate.path))
                    .unwrap_or_else(|| PathBuf::from(&candidate.path)),
                archive_path: candidate.directory_name.clone(),
            })
            .collect();
        let metadata = ArchiveMetadata {
            plugins: selected
                .iter()
                .map(|candidate| ArchiveMetadataPlugin {
                    name: candidate.name.clone(),
                    plugin_id: candidate.plugin_id.clone(),
                    directory_name: candidate.directory_name.clone(),
                    enabled: candidate.enabled_override,
                })
                .collect(),
        };
        let archive = create_archive(&entries, &metadata)?;
        if archive.len() as u64 > self.max_archive_bytes {
            return Err(format!(
                "plugin sync archive exceeds limit: {}/{}",
                archive.len(),
                self.max_archive_bytes
            ));
        }
        Ok(PluginSyncArchiveExportResult {
            archive_bytes: archive.len() as u64,
            archive,
            plugins: selected
                .iter()
                .map(|candidate| PluginSyncArchivePlugin {
                    id: candidate.id.clone(),
                    name: candidate.name.clone(),
                    plugin_id: candidate.plugin_id.clone(),
                    directory_name: candidate.directory_name.clone(),
                    enabled: candidate.enabled_override,
                })
                .collect(),
        })
    }

    pub fn import_plugins_archive(
        &self,
        archive: &[u8],
        overwrite: bool,
    ) -> Result<PluginSyncImportResult, String> {
        if overwrite {
            return Err("plugin sync overwrite is not supported".to_string());
        }
        if archive.len() as u64 > self.max_archive_bytes {
            return Err(format!(
                "plugin sync archive exceeds limit: {}/{}",
                archive.len(),
                self.max_archive_bytes
            ));
        }
        import_plugins_archive(archive, &self.home_dir, self.max_archive_bytes)
    }
}
