//! 数据根初始化器：合法性判定、诊断根 pending、fresh/migrate/fail 与 import 编排。
//!
//! 真源：`packages/services/src/data-root/initializer.ts`。
//! 诊断根目录、pid、时钟、备份时间戳与迁移回调由调用方注入。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::copy_migration::{
    execute_data_root_copy_migration, DataRootMigrationProgress, DataRootMigrationResult,
    ExecuteDataRootCopyMigrationInput,
};
use crate::migration::{
    cleanup_stale_migration_staging, discover_legacy_data_root_candidates, resolve_absolute,
};
use crate::{
    forfeit_conflicting_data_root, forfeit_data_root_by_label, read_data_root_status,
    resolve_data_root_dir, write_data_root_manifest, DataRootManifest, DataRootManifestCreatedBy,
    DataRootManifestMigrationMode, DataRootStatus, LegacyDataRootCandidate, UnownedReason,
    DATA_ROOT_MANIFEST_SCHEMA_VERSION, DATA_ROOT_PRODUCT_ID,
};

pub const DIAGNOSTIC_ROOT_TTL_MS: i64 = 24 * 60 * 60 * 1000;

pub type ClockIso = Arc<dyn Fn() -> String + Send + Sync>;
pub type MtimeLookup = Arc<dyn Fn(&Path) -> Option<i64> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DataRootInitializedBy {
    Fresh,
    Migration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum DataRootInitResult {
    Ready {
        #[serde(rename = "baseDir")]
        base_dir: String,
        status: DataRootStatus,
    },
    Initialized {
        #[serde(rename = "baseDir")]
        base_dir: String,
        #[serde(rename = "initializedBy")]
        initialized_by: DataRootInitializedBy,
        #[serde(rename = "forfeitedRoot", skip_serializing_if = "Option::is_none")]
        forfeited_root: Option<String>,
    },
    Pending {
        #[serde(rename = "baseDir")]
        base_dir: String,
        status: DataRootStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreshDataRootResult {
    pub base_dir: String,
    pub manifest_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forfeited_root: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRootNonInteractiveAction {
    Fresh,
    Migrate,
    Fail,
}

impl DataRootNonInteractiveAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fresh" => Some(Self::Fresh),
            "migrate" => Some(Self::Migrate),
            "fail" => Some(Self::Fail),
            _ => None,
        }
    }
}

/// `resolveDataRootActionFromEnv`：非法非空值回退 fresh。
pub fn resolve_data_root_action_from_env(
    env: &BTreeMap<String, String>,
) -> DataRootNonInteractiveAction {
    env.get("MODE_DATA_ROOT_ACTION")
        .and_then(|value| DataRootNonInteractiveAction::parse(value))
        .unwrap_or(DataRootNonInteractiveAction::Fresh)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum DataRootImportResult {
    Ok {
        ok: bool,
        #[serde(rename = "backupRoot")]
        backup_root: Option<String>,
        #[serde(rename = "migratedBases")]
        migrated_bases: Vec<String>,
    },
    Err {
        ok: bool,
        error: String,
        cancelled: bool,
        #[serde(rename = "backupRoot")]
        backup_root: Option<String>,
    },
}

pub struct DataRootInitializer {
    pub base_dir: PathBuf,
    /// `MODE_DATA_BASE_DIR` 生效时的基目录；`None` 表示非隔离。
    pub env_override_base: Option<PathBuf>,
    pub created_by: DataRootManifestCreatedBy,
    pub app_version: String,
    pub tmp_dir: PathBuf,
    pub pid: u32,
    pub home_dir: PathBuf,
    pub now_iso: ClockIso,
    pub backup_timestamp: ClockIso,
    pub staging_id: ClockIso,
    pub diagnostic_now_ms: i64,
    pub diagnostic_stale_ms: i64,
    pub mtime_of: MtimeLookup,
    active_diagnostic_root: Mutex<Option<PathBuf>>,
    path_override: Mutex<Option<PathBuf>>,
}

impl DataRootInitializer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_dir: impl Into<PathBuf>,
        env_override_base: Option<PathBuf>,
        created_by: DataRootManifestCreatedBy,
        app_version: impl Into<String>,
        tmp_dir: impl Into<PathBuf>,
        pid: u32,
        home_dir: impl Into<PathBuf>,
        now_iso: ClockIso,
        backup_timestamp: ClockIso,
        staging_id: ClockIso,
        diagnostic_now_ms: i64,
        mtime_of: MtimeLookup,
    ) -> Self {
        Self {
            base_dir: base_dir.into(),
            env_override_base,
            created_by,
            app_version: app_version.into(),
            tmp_dir: tmp_dir.into(),
            pid,
            home_dir: home_dir.into(),
            now_iso,
            backup_timestamp,
            staging_id,
            diagnostic_now_ms,
            diagnostic_stale_ms: DIAGNOSTIC_ROOT_TTL_MS,
            mtime_of,
            active_diagnostic_root: Mutex::new(None),
            path_override: Mutex::new(None),
        }
    }

    pub fn active_diagnostic_root(&self) -> Option<PathBuf> {
        self.active_diagnostic_root
            .lock()
            .expect("diagnostic root lock")
            .clone()
    }

    pub fn data_root_path_override(&self) -> Option<PathBuf> {
        self.path_override
            .lock()
            .expect("path override lock")
            .clone()
    }

    pub fn release_pending_diagnostic_mode(&self) {
        if self
            .active_diagnostic_root
            .lock()
            .expect("diagnostic root lock")
            .is_none()
        {
            return;
        }
        *self
            .active_diagnostic_root
            .lock()
            .expect("diagnostic root lock") = None;
        *self.path_override.lock().expect("path override lock") = None;
    }

    pub fn reset_for_test(&self) {
        self.release_pending_diagnostic_mode();
    }

    fn is_isolated_data_base_dir(&self) -> bool {
        let Some(env_base) = &self.env_override_base else {
            return false;
        };
        resolve_absolute(&resolve_data_root_dir(&self.base_dir))
            == resolve_absolute(&resolve_data_root_dir(env_base))
    }

    fn ensure_diagnostic_parent(&self) -> Result<PathBuf, String> {
        let parent = self.tmp_dir.join("mode-startup");
        std::fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700));
        }
        Ok(parent)
    }

    fn cleanup_stale_diagnostic_roots(&self, parent: &Path) {
        let Ok(entries) = std::fs::read_dir(parent) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(modified) = (self.mtime_of)(&path) else {
                continue;
            };
            if self.diagnostic_now_ms - modified > self.diagnostic_stale_ms {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }

    fn enter_pending_diagnostic_mode(&self) -> Result<(), String> {
        let mut active = self
            .active_diagnostic_root
            .lock()
            .expect("diagnostic root lock");
        if active.is_none() {
            let parent = self.ensure_diagnostic_parent()?;
            self.cleanup_stale_diagnostic_roots(&parent);
            let root = parent.join(self.pid.to_string());
            std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
            }
            *active = Some(root);
        }
        let root = active.as_ref().expect("diagnostic root just set").clone();
        *self.path_override.lock().expect("path override lock") = Some(root);
        Ok(())
    }

    fn build_manifest(&self) -> DataRootManifest {
        DataRootManifest {
            product: DATA_ROOT_PRODUCT_ID.to_string(),
            schema_version: DATA_ROOT_MANIFEST_SCHEMA_VERSION,
            created_by: self.created_by,
            created_at: (self.now_iso)(),
            first_seen_version: self.app_version.clone(),
            migration: None,
        }
    }

    /// `initializeFreshDataRoot`。
    pub fn initialize_fresh(&self, adopt_unowned: bool) -> Result<FreshDataRootResult, String> {
        let status = read_data_root_status(&self.base_dir);
        let adoptable = adopt_unowned
            && matches!(
                status,
                DataRootStatus::Unowned {
                    reason: UnownedReason::ManifestMissing,
                    ..
                }
            );
        let forfeited_root = if !adoptable
            && matches!(
                status,
                DataRootStatus::Unowned { .. } | DataRootStatus::Corrupt { .. }
            ) {
            forfeit_conflicting_data_root(&self.base_dir, &status, &(self.backup_timestamp)())?
        } else {
            None
        };
        let manifest_path = write_data_root_manifest(&self.base_dir, &self.build_manifest())?;
        Ok(FreshDataRootResult {
            base_dir: self.base_dir.to_string_lossy().into_owned(),
            manifest_path,
            forfeited_root,
        })
    }

    /// `initializeDataRootInteractive`。
    pub fn initialize_interactive(&self) -> Result<DataRootInitResult, String> {
        let _ = cleanup_stale_migration_staging(&self.base_dir);
        let status = read_data_root_status(&self.base_dir);
        let base_dir = self.base_dir.to_string_lossy().into_owned();
        match status {
            DataRootStatus::Normal { .. } => {
                self.release_pending_diagnostic_mode();
                Ok(DataRootInitResult::Ready { base_dir, status })
            }
            DataRootStatus::Absent => {
                let candidates =
                    discover_legacy_data_root_candidates(&self.base_dir, &self.home_dir);
                if candidates.is_empty() {
                    let fresh = self.initialize_fresh(false)?;
                    self.release_pending_diagnostic_mode();
                    Ok(DataRootInitResult::Initialized {
                        base_dir,
                        initialized_by: DataRootInitializedBy::Fresh,
                        forfeited_root: fresh.forfeited_root,
                    })
                } else {
                    self.enter_pending_diagnostic_mode()?;
                    Ok(DataRootInitResult::Pending { base_dir, status })
                }
            }
            DataRootStatus::Unowned {
                reason: UnownedReason::ManifestMissing,
                ..
            } if self.is_isolated_data_base_dir() => {
                let fresh = self.initialize_fresh(true)?;
                self.release_pending_diagnostic_mode();
                Ok(DataRootInitResult::Initialized {
                    base_dir,
                    initialized_by: DataRootInitializedBy::Fresh,
                    forfeited_root: fresh.forfeited_root,
                })
            }
            status => {
                self.enter_pending_diagnostic_mode()?;
                Ok(DataRootInitResult::Pending { base_dir, status })
            }
        }
    }

    /// `initializeDataRootNonInteractive`。
    pub fn initialize_non_interactive(
        &self,
        action: DataRootNonInteractiveAction,
    ) -> Result<DataRootInitResult, String> {
        let _ = cleanup_stale_migration_staging(&self.base_dir);
        let status = read_data_root_status(&self.base_dir);
        let base_dir = self.base_dir.to_string_lossy().into_owned();
        if matches!(status, DataRootStatus::Normal { .. }) {
            self.release_pending_diagnostic_mode();
            return Ok(DataRootInitResult::Ready { base_dir, status });
        }
        let candidates = discover_legacy_data_root_candidates(&self.base_dir, &self.home_dir);
        if action == DataRootNonInteractiveAction::Fail {
            return Err(format!(
                "数据根 {} 状态异常（{}），MODE_DATA_ROOT_ACTION=fail 下拒绝启动",
                self.base_dir.to_string_lossy(),
                status_kind(&status)
            ));
        }
        if action == DataRootNonInteractiveAction::Migrate && !candidates.is_empty() {
            let result = self.execute_migration(
                &candidates,
                DataRootManifestMigrationMode::Copy,
                None,
                None,
            );
            if let DataRootMigrationResult::Err { error, .. } = &result {
                return Err(format!("旧数据迁移失败: {error}"));
            }
            let migrated_status = read_data_root_status(&self.base_dir);
            if !matches!(migrated_status, DataRootStatus::Normal { .. }) {
                return Err(format!(
                    "迁移完成但数据根未合法化: {}",
                    self.base_dir.to_string_lossy()
                ));
            }
            self.release_pending_diagnostic_mode();
            return Ok(DataRootInitResult::Initialized {
                base_dir,
                initialized_by: DataRootInitializedBy::Migration,
                forfeited_root: None,
            });
        }
        let fresh = self.initialize_fresh(
            action == DataRootNonInteractiveAction::Fresh && self.is_isolated_data_base_dir(),
        )?;
        self.release_pending_diagnostic_mode();
        Ok(DataRootInitResult::Initialized {
            base_dir,
            initialized_by: DataRootInitializedBy::Fresh,
            forfeited_root: fresh.forfeited_root,
        })
    }

    /// `executeDesktopDataRootMigration`。
    pub fn execute_desktop_migration(
        &self,
        candidates: &[LegacyDataRootCandidate],
        on_progress: Option<&dyn Fn(&DataRootMigrationProgress)>,
        is_cancelled: Option<&dyn Fn() -> bool>,
    ) -> DataRootMigrationResult {
        self.execute_migration(
            candidates,
            DataRootManifestMigrationMode::Copy,
            on_progress,
            is_cancelled,
        )
    }

    fn execute_migration(
        &self,
        candidates: &[LegacyDataRootCandidate],
        mode: DataRootManifestMigrationMode,
        on_progress: Option<&dyn Fn(&DataRootMigrationProgress)>,
        is_cancelled: Option<&dyn Fn() -> bool>,
    ) -> DataRootMigrationResult {
        let noop_progress = |_event: &DataRootMigrationProgress| {};
        let never_cancelled = || false;
        let progress = on_progress.unwrap_or(&noop_progress);
        let cancelled = is_cancelled.unwrap_or(&never_cancelled);
        let staging_id = Arc::clone(&self.staging_id);
        let now_iso = Arc::clone(&self.now_iso);
        let backup_timestamp = Arc::clone(&self.backup_timestamp);
        execute_data_root_copy_migration(&ExecuteDataRootCopyMigrationInput {
            candidates,
            created_by: self.created_by,
            app_version: &self.app_version,
            mode,
            on_progress: progress,
            is_cancelled: cancelled,
            staging_id: &|| staging_id(),
            now_iso: &|| now_iso(),
            backup_timestamp: &|| backup_timestamp(),
        })
    }

    /// `executeDesktopFreshStart`。
    pub fn execute_desktop_fresh_start(&self) -> Result<FreshDataRootResult, String> {
        self.initialize_fresh(false)
    }

    /// `executeDataRootImport`。
    pub fn execute_import(
        &self,
        candidates: &[LegacyDataRootCandidate],
        on_progress: Option<&dyn Fn(&DataRootMigrationProgress)>,
        is_cancelled: Option<&dyn Fn() -> bool>,
    ) -> DataRootImportResult {
        let backup_root = match forfeit_data_root_by_label(
            &self.base_dir,
            "import",
            &(self.backup_timestamp)(),
        ) {
            Ok(backup) => backup,
            Err(error) => {
                return DataRootImportResult::Err {
                    ok: false,
                    error,
                    cancelled: false,
                    backup_root: None,
                };
            }
        };
        let result = self.execute_migration(
            candidates,
            DataRootManifestMigrationMode::Import,
            on_progress,
            is_cancelled,
        );
        match result {
            DataRootMigrationResult::Ok { migrated_bases, .. } => DataRootImportResult::Ok {
                ok: true,
                backup_root,
                migrated_bases,
            },
            DataRootMigrationResult::Err { error, cancelled } => DataRootImportResult::Err {
                ok: false,
                error,
                cancelled,
                backup_root,
            },
        }
    }
}

fn status_kind(status: &DataRootStatus) -> &'static str {
    match status {
        DataRootStatus::Normal { .. } => "normal",
        DataRootStatus::Absent => "absent",
        DataRootStatus::Unowned { .. } => "unowned",
        DataRootStatus::Corrupt { .. } => "corrupt",
    }
}
