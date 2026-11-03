//! `storageService` 的同步编排核心（app 层）。
//!
//! 真源：`packages/services/src/storage/app/storageService.ts`（scanJob 状态机已在
//! `scan_job.rs`）。TS 侧的扫描 runner / 真实文件系统适配是异步 IO，仍归宿主装配；
//! 这里承载可同步化的两块编排：**job 身份门控**（旧 job 尾包不得覆盖新 job 快照、
//! cancel 只认 current）与**清理管线**（可清理判定 → 取根 → 作用域 → 列候选 →
//! 计划 → 删除 → 跳过数合并）。TS app 层尚无测试骨架，本模块的行为断言以 TS
//! 源码语义为准，TS 完整对拍待补（同 `terminal-service` 的处理方式）。

use crate::catalog::{
    get_storage_category_cleanability, get_storage_clean_scopes, StorageCatalogContext,
    StorageCategoryId, StorageCleanScope, StorageCleanability,
};
use crate::clean::{plan_storage_clean, StorageCleanCandidate, StorageCleanParams};
use crate::scan_job::{StoragePathError, StorageUsageSnapshot};
use crate::usage::StorageRootSpec;

/// TS `StorageDeleteResult`（`FsCleanerPort.deleteFiles` 的返回值）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageDeleteResult {
    pub deleted_count: i64,
    pub freed_bytes: i64,
    pub failures: Vec<StoragePathError>,
}

/// TS `clean()` 的返回值：删除结果 + 计划跳过数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageCleanOutcome {
    pub deleted_count: i64,
    pub freed_bytes: i64,
    pub failures: Vec<StoragePathError>,
    pub skipped_count: usize,
}

/// storageService 的 job 身份状态（TS 闭包状态的结构化形态）。
///
/// 不驱动异步 runner：宿主对 runner 的 abort/终态回包由调用方在对应方法前后接线。
#[derive(Debug, Default)]
pub struct StorageServiceState {
    job_counter: usize,
    latest_job_id: Option<String>,
    current_job_id: Option<String>,
    latest_snapshot: Option<StorageUsageSnapshot>,
}

impl StorageServiceState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `startScan`：取消 current（对旧 runner 的 abort 由宿主执行），生成
    /// `scan-{n}` 新 job 并同时成为 current 与 emit 门控基准（latestJobId）。
    pub fn start_scan(&mut self) -> String {
        self.current_job_id = None;
        self.job_counter += 1;
        let job_id = format!("scan-{}", self.job_counter);
        self.latest_job_id = Some(job_id.clone());
        self.current_job_id = Some(job_id.clone());
        job_id
    }

    /// emit 门控：`jobId === latestJobId` 才允许写入 latestSnapshot；
    /// 被新 job 取代的旧 job 尾包对 emitter 仍可见（TS fire 不受门控），但不得覆盖快照。
    pub fn accepts_snapshot(&self, job_id: &str) -> bool {
        self.latest_job_id.as_deref() == Some(job_id)
    }

    pub fn record_snapshot(&mut self, job_id: &str, snapshot: StorageUsageSnapshot) {
        if self.accepts_snapshot(job_id) {
            self.latest_snapshot = Some(snapshot);
        }
    }

    /// job 终态后的条件清理（TS `done.then`：current 仍是该 job 才清空）。
    pub fn finish_job(&mut self, job_id: &str) {
        if self.current_job_id.as_deref() == Some(job_id) {
            self.current_job_id = None;
        }
    }

    /// `cancelScan(jobId)`：只取消 current；命中返回 true（宿主随后对 runner abort）。
    pub fn cancel_scan(&mut self, job_id: &str) -> bool {
        if self.current_job_id.as_deref() == Some(job_id) {
            self.current_job_id = None;
            true
        } else {
            false
        }
    }

    /// `clean` 与 `dispose` 的前置动作：取消 current（状态面）。
    pub fn cancel_current(&mut self) {
        self.current_job_id = None;
    }

    pub fn current_job_id(&self) -> Option<&str> {
        self.current_job_id.as_deref()
    }

    pub fn latest_job_id(&self) -> Option<&str> {
        self.latest_job_id.as_deref()
    }

    pub fn latest_snapshot(&self) -> Option<&StorageUsageSnapshot> {
        self.latest_snapshot.as_ref()
    }
}

/// TS `clean(request)`：整条清理管线（不含取消 current 的状态面）。
///
/// 顺序与 TS 一致：可清理判定（含未知类别）→ 找根 → 作用域 → 列候选 → 计划 →
/// 删除（keepDirectories = 作用域前缀）→ 合并 `skippedCount`。
/// `offset_secs` 对应 TS `isSameLocalDay` 的本地时区（宿主注入）。
pub fn clean_category(
    category_id: &str,
    root_id: &str,
    roots: &[StorageRootSpec],
    now: i64,
    offset_secs: i32,
    list_candidates: impl FnOnce(
        &str,
        &[StorageCleanScope],
    ) -> Result<Vec<StorageCleanCandidate>, String>,
    delete_files: impl FnOnce(
        &str,
        &[StorageCleanCandidate],
        &[String],
    ) -> Result<StorageDeleteResult, String>,
) -> Result<StorageCleanOutcome, String> {
    let category = StorageCategoryId::parse(category_id)
        .filter(|category| {
            get_storage_category_cleanability(*category) != StorageCleanability::None
        })
        .ok_or_else(|| format!("storage category is not cleanable: {category_id}"))?;
    let root = roots
        .iter()
        .find(|root| root.id.as_str() == root_id)
        .ok_or_else(|| format!("storage root not available: {root_id}"))?;
    let scopes = get_storage_clean_scopes(category);
    let candidates = list_candidates(&root.path, &scopes)?;
    let plan = plan_storage_clean(&StorageCleanParams {
        category_id: category,
        candidates: &candidates,
        context: StorageCatalogContext {
            root_id: root.id,
            has_custom_data_base_dir: root.has_custom_data_base_dir,
        },
        now,
        offset_secs,
    });
    let keep_directories: Vec<String> = scopes.iter().map(|scope| scope.prefix.clone()).collect();
    let result = delete_files(&root.path, &plan.targets, &keep_directories)?;
    Ok(StorageCleanOutcome {
        deleted_count: result.deleted_count,
        freed_bytes: result.freed_bytes,
        failures: result.failures,
        skipped_count: plan.skipped_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::StorageRootId;
    use std::cell::RefCell;

    fn root(id: StorageRootId, path: &str) -> StorageRootSpec {
        StorageRootSpec {
            id,
            path: path.to_string(),
            has_custom_data_base_dir: false,
        }
    }

    fn snapshot(job_id: &str) -> StorageUsageSnapshot {
        StorageUsageSnapshot {
            job_id: job_id.to_string(),
            status: crate::scan_job::StorageScanStatus::Scanning,
            started_at: 0,
            finished_at: None,
            roots: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn job_ids_sequence_and_gate_old_job_tails() {
        let mut state = StorageServiceState::new();
        let first = state.start_scan();
        assert_eq!(first, "scan-1");
        assert!(state.accepts_snapshot(&first));
        let second = state.start_scan();
        assert_eq!(second, "scan-2");
        // 旧 job 尾包被门控拒绝，新 job 快照可写入。
        state.record_snapshot(&first, snapshot(&first));
        assert!(state.latest_snapshot().is_none());
        state.record_snapshot(&second, snapshot(&second));
        assert_eq!(
            state.latest_snapshot().map(|snap| snap.job_id.as_str()),
            Some("scan-2")
        );
    }

    #[test]
    fn cancel_scan_only_hits_current_and_finish_is_conditional() {
        let mut state = StorageServiceState::new();
        let first = state.start_scan();
        let second = state.start_scan();
        assert!(!state.cancel_scan(&first), "旧 job 不再是 current");
        assert!(state.cancel_scan(&second));
        assert_eq!(state.current_job_id(), None);
        state.finish_job(&second);
        // cancel 后的 finish 是幂等空操作。
        assert_eq!(state.current_job_id(), None);
        let third = state.start_scan();
        assert_eq!(third, "scan-3");
        state.finish_job(&third);
        assert_eq!(state.current_job_id(), None);
        assert_eq!(state.latest_job_id(), Some("scan-3"));
    }

    #[test]
    fn clean_rejects_unknown_and_uncleanable_categories() {
        let error = clean_category(
            "nope",
            "home",
            &[],
            0,
            0,
            |_, _| Ok(Vec::new()),
            |_, _, _| unreachable!("不可清理类别不应触达 cleaner"),
        )
        .expect_err("未知类别");
        assert_eq!(error, "storage category is not cleanable: nope");
        let error = clean_category(
            "config",
            "home",
            &[],
            0,
            0,
            |_, _| Ok(Vec::new()),
            |_, _, _| unreachable!("不可清理类别不应触达 cleaner"),
        )
        .expect_err("config 不可清理");
        assert_eq!(error, "storage category is not cleanable: config");
    }

    #[test]
    fn clean_requires_available_root() {
        let error = clean_category(
            "logs",
            "absent",
            &[],
            0,
            0,
            |_, _| Ok(Vec::new()),
            |_, _, _| unreachable!("根缺失不应触达 cleaner"),
        )
        .expect_err("根缺失");
        assert_eq!(error, "storage root not available: absent");
    }

    #[test]
    fn clean_pipeline_passes_scopes_and_merges_skipped_count() {
        let roots = vec![root(StorageRootId::Home, "/data")];
        let list_calls: RefCell<Vec<(String, Vec<StorageCleanScope>)>> = RefCell::new(Vec::new());
        let delete_calls: RefCell<Vec<(String, Vec<String>)>> = RefCell::new(Vec::new());
        let outcome = clean_category(
            "logs",
            "home",
            &roots,
            1_700_000_000_000,
            0,
            |path, scopes| {
                list_calls
                    .borrow_mut()
                    .push((path.to_string(), scopes.to_vec()));
                // 混入一条不属于 logs 的候选：计划阶段应跳过。
                Ok(vec![StorageCleanCandidate {
                    relative_path: "random/other.bin".to_string(),
                    bytes: 10,
                    mtime_ms: 0,
                }])
            },
            |path, targets, keep_directories| {
                delete_calls
                    .borrow_mut()
                    .push((path.to_string(), keep_directories.to_vec()));
                assert!(targets.is_empty(), "分类不符的候选不是删除目标");
                Ok(StorageDeleteResult {
                    deleted_count: 0,
                    freed_bytes: 0,
                    failures: vec![StoragePathError {
                        path: "/x".to_string(),
                        code: "EACCES".to_string(),
                    }],
                })
            },
        )
        .expect("clean");
        assert_eq!(outcome.skipped_count, 1);
        assert_eq!(outcome.deleted_count, 0);
        assert_eq!(outcome.failures.len(), 1);
        let list_calls = list_calls.into_inner();
        assert_eq!(list_calls.len(), 1);
        assert_eq!(list_calls[0].0, "/data");
        let delete_calls = delete_calls.into_inner();
        assert_eq!(delete_calls.len(), 1);
        assert_eq!(delete_calls[0].0, "/data");
        // keepDirectories = 作用域前缀全集（logs 共 6 个前缀）。
        assert_eq!(
            delete_calls[0].1,
            vec![
                "v2/logs".to_string(),
                "cli/log".to_string(),
                "logs".to_string(),
                "v2/crash".to_string(),
                "v2/perf".to_string(),
                "computer-use/run".to_string(),
            ]
        );
    }

    #[test]
    fn clean_port_errors_propagate() {
        let roots = vec![root(StorageRootId::Home, "/data")];
        let error = clean_category(
            "logs",
            "home",
            &roots,
            0,
            0,
            |_, _| Err("walker failed".to_string()),
            |_, _, _| unreachable!(),
        )
        .expect_err("列候选失败");
        assert_eq!(error, "walker failed");
        let error = clean_category(
            "logs",
            "home",
            &roots,
            0,
            0,
            |_, _| Ok(Vec::new()),
            |_, _, _| Err("delete failed".to_string()),
        )
        .expect_err("删除失败");
        assert_eq!(error, "delete failed");
    }
}
