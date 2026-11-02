//! 存储扫描折叠与卷分组。
//!
//! 真源：`packages/services/src/storage/domain/usageAggregate.ts`、
//! `packages/shared/src/storage.ts` 的 `groupStorageRootsByVolume`。

use serde_json::{json, Value};

use crate::catalog::{
    classify_storage_path, get_storage_category_cleanability, StorageCatalogContext,
    StorageCategoryId, StorageCleanability, StorageRootId, STORAGE_CATEGORY_IDS,
};

pub const STORAGE_MORE_ENTRIES_PATH: &str = "…";
const DEFAULT_MAX_ENTRIES_PER_CATEGORY: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageScanEntry {
    pub relative_path: String,
    pub bytes: i64,
    pub mtime_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageRootSpec {
    pub id: StorageRootId,
    pub path: String,
    pub has_custom_data_base_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageVolume {
    pub device_id: String,
    pub mount_point: String,
    pub total_bytes: i64,
    pub free_bytes: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageEntryUsage {
    pub relative_path: String,
    pub bytes: i64,
    pub file_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageCategoryUsage {
    pub id: StorageCategoryId,
    pub bytes: i64,
    pub file_count: u64,
    pub cleanability: StorageCleanability,
    pub entries: Vec<StorageEntryUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageRootUsage {
    pub id: StorageRootId,
    pub path: String,
    pub volume: Option<StorageVolume>,
    pub bytes: i64,
    pub file_count: u64,
    pub categories: Vec<StorageCategoryUsage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageVolumeGroup {
    pub key: String,
    pub volume: Option<StorageVolume>,
    pub roots: Vec<StorageRootUsage>,
    pub bytes: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct EntryBucket {
    bytes: i64,
    file_count: u64,
}

#[derive(Clone, Debug, Default)]
struct CategoryBucket {
    bytes: i64,
    file_count: u64,
    entries: indexmap::IndexMap<String, EntryBucket>,
}

pub struct StorageUsageAccumulator {
    spec: StorageRootSpec,
    max_entries_per_category: usize,
    buckets: indexmap::IndexMap<StorageCategoryId, CategoryBucket>,
    total_bytes: i64,
    total_files: u64,
}

impl StorageUsageAccumulator {
    pub fn new(spec: StorageRootSpec, max_entries_per_category: Option<usize>) -> Self {
        Self {
            spec,
            max_entries_per_category: max_entries_per_category
                .unwrap_or(DEFAULT_MAX_ENTRIES_PER_CATEGORY),
            buckets: indexmap::IndexMap::new(),
            total_bytes: 0,
            total_files: 0,
        }
    }

    pub fn add(&mut self, entry: StorageScanEntry) {
        let classification = classify_storage_path(
            &entry.relative_path,
            &StorageCatalogContext {
                root_id: self.spec.id,
                has_custom_data_base_dir: self.spec.has_custom_data_base_dir,
            },
        );
        let bucket = self.buckets.entry(classification.category_id).or_default();
        bucket.bytes += entry.bytes;
        bucket.file_count += 1;
        let current = bucket.entries.entry(classification.entry_key).or_default();
        current.bytes += entry.bytes;
        current.file_count += 1;
        self.total_bytes += entry.bytes;
        self.total_files += 1;
    }

    pub fn snapshot(&self, volume: Option<&StorageVolume>) -> StorageRootUsage {
        let categories = STORAGE_CATEGORY_IDS
            .iter()
            .map(|category_id| {
                let bucket = self.buckets.get(category_id);
                StorageCategoryUsage {
                    id: *category_id,
                    bytes: bucket.map_or(0, |bucket| bucket.bytes),
                    file_count: bucket.map_or(0, |bucket| bucket.file_count),
                    cleanability: get_storage_category_cleanability(*category_id),
                    entries: bucket
                        .map(|bucket| fold_entries(&bucket.entries, self.max_entries_per_category))
                        .unwrap_or_default(),
                }
            })
            .collect();
        StorageRootUsage {
            id: self.spec.id,
            path: self.spec.path.clone(),
            volume: volume.cloned(),
            bytes: self.total_bytes,
            file_count: self.total_files,
            categories,
        }
    }
}

fn fold_entries(
    entries: &indexmap::IndexMap<String, EntryBucket>,
    max_entries: usize,
) -> Vec<StorageEntryUsage> {
    let mut sorted: Vec<StorageEntryUsage> = entries
        .iter()
        .map(|(relative_path, usage)| StorageEntryUsage {
            relative_path: relative_path.clone(),
            bytes: usage.bytes,
            file_count: usage.file_count,
        })
        .collect();
    // TS 使用 localeCompare；这里按字节序比较，非 ASCII/大小写混排的并列顺序可能不同。
    sorted.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then_with(|| a.relative_path.cmp(&b.relative_path))
    });
    if sorted.len() <= max_entries {
        return sorted;
    }
    let rest = sorted.split_off(max_entries);
    let bytes = rest.iter().map(|entry| entry.bytes).sum();
    let file_count = rest.iter().map(|entry| entry.file_count).sum();
    sorted.push(StorageEntryUsage {
        relative_path: STORAGE_MORE_ENTRIES_PATH.to_string(),
        bytes,
        file_count,
    });
    sorted
}

pub fn group_storage_roots_by_volume(roots: Vec<StorageRootUsage>) -> Vec<StorageVolumeGroup> {
    let mut groups: Vec<StorageVolumeGroup> = Vec::new();
    for root in roots {
        let key = match &root.volume {
            Some(volume) => format!("dev:{}", volume.device_id),
            None => format!("path:{}", root.path),
        };
        if let Some(group) = groups.iter_mut().find(|group| group.key == key) {
            group.roots.push(root.clone());
            group.bytes += root.bytes;
            continue;
        }
        groups.push(StorageVolumeGroup {
            key,
            volume: root.volume.clone(),
            bytes: root.bytes,
            roots: vec![root],
        });
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.bytes));
    groups
}

impl StorageVolume {
    pub fn to_json(&self) -> Value {
        json!({
            "deviceId": self.device_id,
            "mountPoint": self.mount_point,
            "totalBytes": self.total_bytes,
            "freeBytes": self.free_bytes,
        })
    }
}

impl StorageEntryUsage {
    pub fn to_json(&self) -> Value {
        json!({
            "relativePath": self.relative_path,
            "bytes": self.bytes,
            "fileCount": self.file_count,
        })
    }
}

impl StorageCategoryUsage {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id.as_str(),
            "bytes": self.bytes,
            "fileCount": self.file_count,
            "cleanability": self.cleanability.as_str(),
            "entries": self.entries.iter().map(StorageEntryUsage::to_json).collect::<Vec<_>>(),
        })
    }
}

impl StorageRootUsage {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id.as_str(),
            "path": self.path,
            "volume": self.volume.as_ref().map(StorageVolume::to_json),
            "bytes": self.bytes,
            "fileCount": self.file_count,
            "categories": self.categories.iter().map(StorageCategoryUsage::to_json).collect::<Vec<_>>(),
        })
    }
}

impl StorageVolumeGroup {
    pub fn to_json(&self) -> Value {
        json!({
            "key": self.key,
            "volume": self.volume.as_ref().map(StorageVolume::to_json),
            "roots": self.roots.iter().map(StorageRootUsage::to_json).collect::<Vec<_>>(),
            "bytes": self.bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> StorageRootSpec {
        StorageRootSpec {
            id: StorageRootId::Home,
            path: "/home/u/.mode".to_string(),
            has_custom_data_base_dir: false,
        }
    }

    #[test]
    fn folds_entries_beyond_limit() {
        let mut accumulator = StorageUsageAccumulator::new(spec(), Some(2));
        for (path, bytes) in [
            ("v2/logs/a.log", 3),
            ("v2/logs/b.log", 2),
            ("v2/logs/c.log", 1),
        ] {
            accumulator.add(StorageScanEntry {
                relative_path: path.to_string(),
                bytes,
                mtime_ms: 0,
            });
        }
        let snapshot = accumulator.snapshot(None);
        let logs = snapshot
            .categories
            .iter()
            .find(|category| category.id == StorageCategoryId::Logs)
            .expect("logs");
        assert_eq!(logs.bytes, 6);
        assert_eq!(logs.entries.len(), 3);
        assert_eq!(logs.entries[2].relative_path, STORAGE_MORE_ENTRIES_PATH);
        assert_eq!(logs.entries[2].bytes, 1);
    }

    #[test]
    fn groups_roots_by_device_and_falls_back_to_path() {
        let root =
            |id: StorageRootId, path: &str, device: Option<&str>, bytes: i64| StorageRootUsage {
                id,
                path: path.to_string(),
                volume: device.map(|device| StorageVolume {
                    device_id: device.to_string(),
                    mount_point: "/".to_string(),
                    total_bytes: 100,
                    free_bytes: 50,
                }),
                bytes,
                file_count: 1,
                categories: Vec::new(),
            };
        let groups = group_storage_roots_by_volume(vec![
            root(StorageRootId::Home, "/a", Some("1"), 10),
            root(StorageRootId::DataBaseDir, "/b", None, 40),
            root(StorageRootId::Home, "/c", Some("1"), 20),
        ]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].key, "path:/b");
        assert_eq!(groups[0].bytes, 40);
        assert_eq!(groups[1].key, "dev:1");
        assert_eq!(groups[1].roots.len(), 2);
        assert_eq!(groups[1].bytes, 30);
    }
}
