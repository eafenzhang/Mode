//! 旧数据根探测、体积统计与残留 staging 清理。
//!
//! 真源：`packages/services/src/data-root/migration.ts` 的前半部分。
//! 复制迁移（executeDataRootCopyMigration）尚未迁移。

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    read_legacy_data_base_dir_from_settings, LEGACY_MODE_DATA_ROOT_DIR_NAMES,
    MODE_DATA_ROOT_DIR_NAME,
};

pub const STAGING_STALE_MS: i64 = 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyDataRootCandidate {
    pub base_dir: PathBuf,
    pub legacy_root: PathBuf,
    pub is_primary_base: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyDataRootCandidateStats {
    pub size_bytes: u64,
    pub modified_at: Option<String>,
    pub item_count: u64,
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if output.file_name().is_some() {
                    output.pop();
                } else if !output.has_root() {
                    output.push("..");
                }
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

/// `path.resolve` 的等价：相对路径按当前目录展开并做词法归一化。
pub fn resolve_absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_path(path.to_path_buf())
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        normalize_path(cwd.join(path))
    }
}

fn expand_home_dir(value: &str, home: &Path) -> PathBuf {
    if value == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return home.join(rest);
    }
    PathBuf::from(value)
}

fn find_legacy_root_in_base(base_dir: &Path) -> Option<PathBuf> {
    for name in LEGACY_MODE_DATA_ROOT_DIR_NAMES {
        let candidate = base_dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// `discoverLegacyDataRootCandidates`：主候选 + 旧 setting 指向的自定义 base（一层）。
pub fn discover_legacy_data_root_candidates(
    base_dir: &Path,
    home: &Path,
) -> Vec<LegacyDataRootCandidate> {
    let mut candidates = Vec::new();
    let mut seen = Vec::new();
    let primary_legacy_root = find_legacy_root_in_base(base_dir);
    if let Some(legacy_root) = &primary_legacy_root {
        candidates.push(LegacyDataRootCandidate {
            base_dir: base_dir.to_path_buf(),
            legacy_root: legacy_root.clone(),
            is_primary_base: true,
        });
        seen.push(resolve_absolute(legacy_root));
    }
    if let Some(primary) = &primary_legacy_root {
        if let Some(configured) = read_legacy_data_base_dir_from_settings(primary) {
            let custom_base = resolve_absolute(&expand_home_dir(&configured, home));
            if let Some(custom_legacy_root) = find_legacy_root_in_base(&custom_base) {
                let resolved = resolve_absolute(&custom_legacy_root);
                if !seen.contains(&resolved) {
                    candidates.push(LegacyDataRootCandidate {
                        base_dir: custom_base,
                        legacy_root: custom_legacy_root,
                        is_primary_base: false,
                    });
                    seen.push(resolved);
                }
            }
        }
    }
    candidates
}

fn modified_ms(metadata: &std::fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_millis() as i64)
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `new Date(ms).toISOString()`。
pub fn iso8601_utc(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let ms_of_day = ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hour = ms_of_day / 3_600_000;
    let minute = (ms_of_day / 60_000) % 60;
    let second = (ms_of_day / 1_000) % 60;
    let millis = ms_of_day % 1_000;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// `collectLegacyCandidateStats`：递归统计，符号链接跳过，单项失败忽略。
pub fn collect_legacy_candidate_stats(legacy_root: &Path) -> LegacyDataRootCandidateStats {
    fn walk(dir: &Path, stats: &mut LegacyDataRootCandidateStats, latest: &mut i64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            stats.item_count += 1;
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                walk(&path, stats, latest);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if let Ok(metadata) = std::fs::metadata(&path) {
                stats.size_bytes += metadata.len();
                if let Some(modified) = modified_ms(&metadata) {
                    *latest = (*latest).max(modified);
                }
            }
        }
    }

    let mut stats = LegacyDataRootCandidateStats {
        size_bytes: 0,
        modified_at: None,
        item_count: 0,
    };
    let mut latest = 0i64;
    walk(legacy_root, &mut stats, &mut latest);
    if let Ok(metadata) = std::fs::metadata(legacy_root) {
        if let Some(modified) = modified_ms(&metadata) {
            latest = latest.max(modified);
        }
    }
    stats.modified_at = (latest > 0).then(|| iso8601_utc(latest));
    stats
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn file_mtime_ms(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|metadata| modified_ms(&metadata))
}

/// `cleanupStaleMigrationStaging`：删除 base 下超过 staleMs 的 `.mode.migrating-*`。
pub fn cleanup_stale_migration_staging_with(
    base_dir: &Path,
    now: i64,
    stale_ms: i64,
    mtime_of: &dyn Fn(&Path) -> Option<i64>,
) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(base_dir) else {
        return Vec::new();
    };
    let prefix = format!("{MODE_DATA_ROOT_DIR_NAME}.migrating-");
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&prefix) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let Some(modified) = mtime_of(&path) else {
            continue;
        };
        if now - modified < stale_ms {
            continue;
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            removed.push(path.to_string_lossy().into_owned());
        }
    }
    removed.sort();
    removed
}

/// 使用真实 mtime 的便捷入口。
pub fn cleanup_stale_migration_staging(base_dir: &Path) -> Vec<String> {
    let now = now_ms();
    cleanup_stale_migration_staging_with(base_dir, now, STAGING_STALE_MS, &file_mtime_ms)
}
