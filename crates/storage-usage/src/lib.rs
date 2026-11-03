//! 存储分类目录、清理范围/保护、占用累加与卷分组；storageService 的同步编排
//! 核心与数据根遍历/清理执行。
//!
//! 真源：`packages/services/src/storage/domain/storageCatalog.ts`、
//! `usageAggregate.ts`、`packages/shared/src/storage.ts`、
//! `packages/services/src/storage/app/storageService.ts` 与
//! `packages/services/src/storage/adapters/fsWalker.ts`、`fsCleaner.ts`。
//! 规范见 `docs/specs/storage-usage.md`。

mod catalog;
mod clean;
mod fs_ops;
mod scan_job;
mod service;
mod usage;

pub use catalog::{
    classify_storage_path, get_storage_category_cleanability, get_storage_clean_scopes,
    is_protected_storage_path, normalize_storage_relative_path, StorageCatalogContext,
    StorageCategoryId, StorageClassification, StorageCleanScope, StorageCleanability,
    StorageRootId, STORAGE_CATEGORY_IDS,
};
pub use clean::{
    plan_storage_clean, StorageCleanCandidate, StorageCleanParams, StorageCleanPlan,
    SUBAGENT_ACTIVE_WINDOW_MS,
};
pub use fs_ops::{
    delete_storage_files, list_storage_candidates, walk_storage_root, WalkStorageRootResult,
};
pub use scan_job::{
    ScanJobFailure, ScanJobProgressAction, StoragePathError, StorageScanJob, StorageScanProgress,
    StorageScanStatus, StorageUsageSnapshot,
};
pub use service::{clean_category, StorageCleanOutcome, StorageDeleteResult, StorageServiceState};
pub use usage::{
    group_storage_roots_by_volume, StorageCategoryUsage, StorageEntryUsage, StorageRootSpec,
    StorageRootUsage, StorageScanEntry, StorageUsageAccumulator, StorageVolume, StorageVolumeGroup,
    STORAGE_MORE_ENTRIES_PATH,
};
