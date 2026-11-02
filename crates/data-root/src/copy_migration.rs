//! 旧数据根复制迁移：staging 复制 → 写归属 → 冲突备份 → rename 落位。
//!
//! 真源：`packages/services/src/data-root/migration.ts` 的
//! `executeDataRootCopyMigration`。时间戳、staging id 与回调由调用方注入。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::migration::collect_legacy_candidate_stats;
use crate::{
    forfeit_conflicting_data_root, read_data_root_status, resolve_data_root_dir,
    write_data_root_manifest_into_root, DataRootManifest, DataRootManifestCreatedBy,
    DataRootManifestMigration, DataRootManifestMigrationMode, DataRootStatus,
    LegacyDataRootCandidate, MODE_DATA_ROOT_DIR_NAME,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MigrationPhase {
    Preparing,
    Copying,
    Finalizing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataRootMigrationProgress {
    pub phase: MigrationPhase,
    pub copied_bytes: u64,
    pub total_bytes: Option<u64>,
    pub current_base_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataRootMigrationResult {
    Ok {
        migrated_bases: Vec<String>,
        manifest_paths: Vec<String>,
    },
    Err {
        error: String,
        cancelled: bool,
    },
}

pub struct ExecuteDataRootCopyMigrationInput<'a> {
    pub candidates: &'a [LegacyDataRootCandidate],
    pub created_by: DataRootManifestCreatedBy,
    pub app_version: &'a str,
    pub mode: DataRootManifestMigrationMode,
    pub on_progress: &'a dyn Fn(&DataRootMigrationProgress),
    pub is_cancelled: &'a dyn Fn() -> bool,
    pub staging_id: &'a dyn Fn() -> String,
    pub now_iso: &'a dyn Fn() -> String,
    pub backup_timestamp: &'a dyn Fn() -> String,
}

enum CopyError {
    Cancelled,
    Message(String),
}

fn should_skip_transient_file(name: &str) -> bool {
    if name == ".DS_Store" {
        return true;
    }
    if name.ends_with(".lock") {
        return true;
    }
    name.starts_with("setting.json.")
}

#[cfg(unix)]
fn inherit_mode(source: &Path, dest: &Path, known_mode: Option<u32>) {
    use std::os::unix::fs::PermissionsExt;
    let mode = known_mode.or_else(|| {
        std::fs::metadata(source)
            .ok()
            .map(|metadata| metadata.permissions().mode())
    });
    if let Some(mode) = mode {
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode & 0o777));
    }
}

#[cfg(not(unix))]
fn inherit_mode(_source: &Path, _dest: &Path, _known_mode: Option<u32>) {}

#[cfg(unix)]
fn source_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn source_mode(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

fn copy_tree(
    src_dir: &Path,
    dest_dir: &Path,
    is_cancelled: &dyn Fn() -> bool,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<(), CopyError> {
    std::fs::create_dir_all(dest_dir).map_err(|error| CopyError::Message(error.to_string()))?;
    let entries =
        std::fs::read_dir(src_dir).map_err(|error| CopyError::Message(error.to_string()))?;
    for entry in entries.flatten() {
        if is_cancelled() {
            return Err(CopyError::Cancelled);
        }
        let name = entry.file_name();
        let src_path = src_dir.join(&name);
        let dest_path = dest_dir.join(&name);
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            copy_tree(&src_path, &dest_path, is_cancelled, on_bytes)?;
            inherit_mode(&src_path, &dest_path, None);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if should_skip_transient_file(&name.to_string_lossy()) {
            continue;
        }
        let mut size = 0u64;
        let mut mode = None;
        if let Ok(metadata) = std::fs::metadata(&src_path) {
            size = metadata.len();
            mode = source_mode(&metadata);
        }
        std::fs::copy(&src_path, &dest_path)
            .map_err(|error| CopyError::Message(error.to_string()))?;
        inherit_mode(&src_path, &dest_path, mode);
        on_bytes(size);
    }
    Ok(())
}

fn remove_dir_force(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// `executeDataRootCopyMigration`。
pub fn execute_data_root_copy_migration(
    input: &ExecuteDataRootCopyMigrationInput<'_>,
) -> DataRootMigrationResult {
    if input.candidates.is_empty() {
        return DataRootMigrationResult::Err {
            error: "没有可迁移的旧数据目录".to_string(),
            cancelled: false,
        };
    }
    let mut staging_paths: Vec<PathBuf> = Vec::new();
    let mut migrated_bases = Vec::new();
    let mut manifest_paths = Vec::new();

    let mut run = || -> Result<(), CopyError> {
        let mut total_bytes = 0u64;
        for candidate in input.candidates {
            total_bytes += collect_legacy_candidate_stats(&candidate.legacy_root).size_bytes;
        }
        let mut copied_bytes = 0u64;

        for candidate in input.candidates {
            if (input.is_cancelled)() {
                return Err(CopyError::Cancelled);
            }
            let target_root = resolve_data_root_dir(&candidate.base_dir);
            let staging_root = candidate.base_dir.join(format!(
                "{MODE_DATA_ROOT_DIR_NAME}.migrating-{}",
                (input.staging_id)()
            ));
            staging_paths.push(staging_root.clone());

            (input.on_progress)(&DataRootMigrationProgress {
                phase: MigrationPhase::Preparing,
                copied_bytes,
                total_bytes: Some(total_bytes),
                current_base_dir: Some(candidate.base_dir.to_string_lossy().into_owned()),
            });

            remove_dir_force(&staging_root).map_err(CopyError::Message)?;
            let mut on_bytes = |bytes| {
                copied_bytes += bytes;
                (input.on_progress)(&DataRootMigrationProgress {
                    phase: MigrationPhase::Copying,
                    copied_bytes,
                    total_bytes: Some(total_bytes),
                    current_base_dir: Some(candidate.base_dir.to_string_lossy().into_owned()),
                });
            };
            copy_tree(
                &candidate.legacy_root,
                &staging_root,
                input.is_cancelled,
                &mut on_bytes,
            )?;

            let manifest = DataRootManifest {
                product: crate::DATA_ROOT_PRODUCT_ID.to_string(),
                schema_version: crate::DATA_ROOT_MANIFEST_SCHEMA_VERSION,
                created_by: input.created_by,
                created_at: (input.now_iso)(),
                first_seen_version: input.app_version.to_string(),
                migration: Some(DataRootManifestMigration {
                    from: candidate.legacy_root.to_string_lossy().into_owned(),
                    at: (input.now_iso)(),
                    mode: input.mode,
                }),
            };
            write_data_root_manifest_into_root(&staging_root, &manifest)
                .map_err(CopyError::Message)?;

            (input.on_progress)(&DataRootMigrationProgress {
                phase: MigrationPhase::Finalizing,
                copied_bytes,
                total_bytes: Some(total_bytes),
                current_base_dir: Some(candidate.base_dir.to_string_lossy().into_owned()),
            });

            let status = read_data_root_status(&candidate.base_dir);
            match status {
                DataRootStatus::Normal { .. } => {
                    let _ = remove_dir_force(&staging_root);
                    continue;
                }
                DataRootStatus::Unowned { .. } | DataRootStatus::Corrupt { .. } => {
                    forfeit_conflicting_data_root(
                        &candidate.base_dir,
                        &status,
                        &(input.backup_timestamp)(),
                    )
                    .map_err(CopyError::Message)?;
                }
                DataRootStatus::Absent => {}
            }
            if let Err(error) = std::fs::rename(&staging_root, &target_root) {
                let after_status = read_data_root_status(&candidate.base_dir);
                if matches!(after_status, DataRootStatus::Normal { .. }) {
                    let _ = remove_dir_force(&staging_root);
                    continue;
                }
                return Err(CopyError::Message(error.to_string()));
            }
            migrated_bases.push(target_root.to_string_lossy().into_owned());
            manifest_paths.push(
                target_root
                    .join(crate::DATA_ROOT_MANIFEST_FILE_NAME)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        Ok(())
    };

    let result = run();
    drop(run);
    match result {
        Ok(()) => DataRootMigrationResult::Ok {
            migrated_bases,
            manifest_paths,
        },
        Err(error) => {
            for staging_path in &staging_paths {
                let _ = remove_dir_force(staging_path);
            }
            match error {
                CopyError::Cancelled => DataRootMigrationResult::Err {
                    error: "迁移已取消".to_string(),
                    cancelled: true,
                },
                CopyError::Message(message) => DataRootMigrationResult::Err {
                    error: message,
                    cancelled: false,
                },
            }
        }
    }
}
