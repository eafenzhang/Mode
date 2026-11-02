//! 数据根归属文件（`{base}/.mode/.mode-root.json`）读写、状态判定与冲突目录备份。
//!
//! 真源：`packages/shared/src/data-root.ts`、
//! `packages/services/src/data-root/ownership.ts`。
//! 备份时间戳由调用方注入（TS 用本地时间格式化），规范见 `docs/specs/data-root.md`。

mod copy_migration;
mod initializer;
mod migration;

pub use copy_migration::{
    execute_data_root_copy_migration, DataRootMigrationProgress, DataRootMigrationResult,
    ExecuteDataRootCopyMigrationInput, MigrationPhase,
};
pub use initializer::{
    resolve_data_root_action_from_env, DataRootImportResult, DataRootInitResult,
    DataRootInitializedBy, DataRootInitializer, DataRootNonInteractiveAction, FreshDataRootResult,
    DIAGNOSTIC_ROOT_TTL_MS,
};
pub use migration::{
    cleanup_stale_migration_staging, cleanup_stale_migration_staging_with,
    collect_legacy_candidate_stats, discover_legacy_data_root_candidates, iso8601_utc,
    resolve_absolute, LegacyDataRootCandidate, LegacyDataRootCandidateStats, STAGING_STALE_MS,
};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MODE_DATA_ROOT_DIR_NAME: &str = ".mode";
pub const LEGACY_MODE_DATA_ROOT_DIR_NAMES: [&str; 2] = [".zcodium", ".zcode"];
pub const DATA_ROOT_MANIFEST_FILE_NAME: &str = ".mode-root.json";
pub const LEGACY_DATA_ROOT_MANIFEST_FILE_NAMES: [&str; 2] =
    [".zcodium-root.json", ".zcode-root.json"];
pub const DATA_ROOT_MANIFEST_SCHEMA_VERSION: i64 = 1;
pub const DATA_ROOT_PRODUCT_ID: &str = "dev.mode.app";
pub const LEGACY_DATA_ROOT_PRODUCT_IDS: [&str; 1] = ["dev.zcodium.app"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataRootManifestCreatedBy {
    Desktop,
    Cli,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataRootManifestMigrationMode {
    Copy,
    Import,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataRootManifestMigration {
    pub from: String,
    pub at: String,
    pub mode: DataRootManifestMigrationMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataRootManifest {
    pub product: String,
    pub schema_version: i64,
    pub created_by: DataRootManifestCreatedBy,
    pub created_at: String,
    pub first_seen_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration: Option<DataRootManifestMigration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnownedReason {
    ManifestMissing,
    ProductMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CorruptReason {
    ManifestUnreadable,
    SchemaUnsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DataRootStatus {
    Normal {
        manifest: DataRootManifest,
    },
    Absent,
    Unowned {
        reason: UnownedReason,
        #[serde(rename = "rootDir")]
        root_dir: String,
    },
    Corrupt {
        reason: CorruptReason,
        #[serde(rename = "rootDir")]
        root_dir: String,
    },
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
}

fn parse_created_by(value: Option<&Value>) -> Option<DataRootManifestCreatedBy> {
    match value?.as_str()? {
        "desktop" => Some(DataRootManifestCreatedBy::Desktop),
        "cli" => Some(DataRootManifestCreatedBy::Cli),
        "server" => Some(DataRootManifestCreatedBy::Server),
        _ => None,
    }
}

fn parse_migration_mode(value: Option<&Value>) -> Option<DataRootManifestMigrationMode> {
    match value?.as_str()? {
        "copy" => Some(DataRootManifestMigrationMode::Copy),
        "import" => Some(DataRootManifestMigrationMode::Import),
        _ => None,
    }
}

fn integer(value: Option<&Value>) -> Option<i64> {
    let number = value?.as_f64()?;
    (number.is_finite() && number.fract() == 0.0).then_some(number as i64)
}

/// `parseDataRootManifest`：不可解析/字段非法返回 `None`。
pub fn parse_data_root_manifest(raw: &str) -> Option<DataRootManifest> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let object = parsed.as_object()?;
    let product = non_empty_string(object.get("product"))?;
    let schema_version = integer(object.get("schemaVersion"))?;
    let created_by = parse_created_by(object.get("createdBy"))?;
    let created_at = non_empty_string(object.get("createdAt"))?;
    let first_seen_version = non_empty_string(object.get("firstSeenVersion"))?;
    let migration = match object.get("migration") {
        None => None,
        Some(value) if value.is_object() => {
            let migration = value.as_object()?;
            Some(DataRootManifestMigration {
                from: non_empty_string(migration.get("from"))?,
                at: non_empty_string(migration.get("at"))?,
                mode: parse_migration_mode(migration.get("mode"))?,
            })
        }
        Some(_) => return None,
    };
    Some(DataRootManifest {
        product,
        schema_version,
        created_by,
        created_at,
        first_seen_version,
        migration,
    })
}

pub fn is_accepted_data_root_product(product: &str) -> bool {
    product == DATA_ROOT_PRODUCT_ID || LEGACY_DATA_ROOT_PRODUCT_IDS.contains(&product)
}

/// 数据根目录绝对路径（`{base}/.mode`）。
pub fn resolve_data_root_dir(base_dir: &Path) -> PathBuf {
    base_dir.join(MODE_DATA_ROOT_DIR_NAME)
}

fn resolve_manifest_path(root_dir: &Path) -> Option<PathBuf> {
    for name in [DATA_ROOT_MANIFEST_FILE_NAME]
        .into_iter()
        .chain(LEGACY_DATA_ROOT_MANIFEST_FILE_NAMES)
    {
        let candidate = root_dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn root_dir_string(root_dir: &Path) -> String {
    root_dir.to_string_lossy().into_owned()
}

/// `readDataRootStatus`。
pub fn read_data_root_status(base_dir: &Path) -> DataRootStatus {
    let root_dir = resolve_data_root_dir(base_dir);
    if !root_dir.exists() {
        return DataRootStatus::Absent;
    }
    let Some(manifest_path) = resolve_manifest_path(&root_dir) else {
        return DataRootStatus::Unowned {
            reason: UnownedReason::ManifestMissing,
            root_dir: root_dir_string(&root_dir),
        };
    };
    let Ok(raw) = std::fs::read_to_string(&manifest_path) else {
        return DataRootStatus::Corrupt {
            reason: CorruptReason::ManifestUnreadable,
            root_dir: root_dir_string(&root_dir),
        };
    };
    let Some(manifest) = parse_data_root_manifest(&raw) else {
        return DataRootStatus::Corrupt {
            reason: CorruptReason::ManifestUnreadable,
            root_dir: root_dir_string(&root_dir),
        };
    };
    if manifest.schema_version > DATA_ROOT_MANIFEST_SCHEMA_VERSION {
        return DataRootStatus::Corrupt {
            reason: CorruptReason::SchemaUnsupported,
            root_dir: root_dir_string(&root_dir),
        };
    }
    if !is_accepted_data_root_product(&manifest.product) {
        return DataRootStatus::Unowned {
            reason: UnownedReason::ProductMismatch,
            root_dir: root_dir_string(&root_dir),
        };
    }
    DataRootStatus::Normal { manifest }
}

/// 原子写归属文件到指定数据根目录；返回 manifest 路径。
pub fn write_data_root_manifest_into_root(
    root_dir: &Path,
    manifest: &DataRootManifest,
) -> Result<String, String> {
    std::fs::create_dir_all(root_dir).map_err(|error| error.to_string())?;
    let manifest_path = root_dir.join(DATA_ROOT_MANIFEST_FILE_NAME);
    let tmp_path = PathBuf::from(format!(
        "{}.{}.tmp",
        manifest_path.to_string_lossy(),
        std::process::id()
    ));
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(manifest).map_err(|error| error.to_string())?
    );
    let result = (|| -> Result<(), String> {
        std::fs::write(&tmp_path, text).map_err(|error| error.to_string())?;
        std::fs::rename(&tmp_path, &manifest_path).map_err(|error| error.to_string())
    })();
    let _ = std::fs::remove_file(&tmp_path);
    result?;
    Ok(manifest_path.to_string_lossy().into_owned())
}

/// 原子写归属文件到 `{base}/.mode`；已存在时覆盖。
pub fn write_data_root_manifest(
    base_dir: &Path,
    manifest: &DataRootManifest,
) -> Result<String, String> {
    write_data_root_manifest_into_root(&resolve_data_root_dir(base_dir), manifest)
}

/// 把 `{base}/.mode` 整体备份为 `{base}/.mode.<label>-<stamp>`，不删除、不合并。
pub fn forfeit_data_root_by_label(
    base_dir: &Path,
    label: &str,
    timestamp: &str,
) -> Result<Option<String>, String> {
    let root_dir = base_dir.join(MODE_DATA_ROOT_DIR_NAME);
    if !root_dir.exists() {
        return Ok(None);
    }
    let mut attempt = 1;
    let mut target = base_dir.join(format!("{MODE_DATA_ROOT_DIR_NAME}.{label}-{timestamp}"));
    while target.exists() {
        if attempt > 100 {
            return Err(format!(
                "无法为冲突数据根生成唯一备份目录: {}",
                root_dir.to_string_lossy()
            ));
        }
        target = base_dir.join(format!(
            "{MODE_DATA_ROOT_DIR_NAME}.{label}-{timestamp}-{attempt}"
        ));
        attempt += 1;
    }
    std::fs::rename(&root_dir, &target).map_err(|error| error.to_string())?;
    Ok(Some(target.to_string_lossy().into_owned()))
}

/// unowned/corrupt 冲突目录整体备份让路。
pub fn forfeit_conflicting_data_root(
    base_dir: &Path,
    status: &DataRootStatus,
    timestamp: &str,
) -> Result<Option<String>, String> {
    let label = match status {
        DataRootStatus::Corrupt { .. } => "corrupt",
        DataRootStatus::Unowned { .. } => "unowned",
        _ => return Ok(None),
    };
    forfeit_data_root_by_label(base_dir, label, timestamp)
}

/// 读取旧 setting.json 里的 `dataBaseDir`（仅用于发现附加迁移源）。
pub fn read_legacy_data_base_dir_from_settings(legacy_root: &Path) -> Option<String> {
    let settings_path = legacy_root.join("v2").join("setting.json");
    if !settings_path.exists() {
        return None;
    }
    let raw = std::fs::read_to_string(settings_path).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    let object = parsed.as_object()?;
    object
        .get("dataBaseDir")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
