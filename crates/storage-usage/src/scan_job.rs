//! 单次扫描 job 的状态/节流纯逻辑。
//!
//! 真源：`packages/services/src/storage/app/scanJob.ts`。
//! 异步 runner 与 timer 由适配层驱动；本模块只处理快照更新、节流决策与终态。

use serde_json::{json, Value};

use crate::usage::StorageRootUsage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageScanStatus {
    Scanning,
    Complete,
    Cancelled,
    Failed,
}

impl StorageScanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scanning => "scanning",
            Self::Complete => "complete",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoragePathError {
    pub path: String,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageUsageSnapshot {
    pub job_id: String,
    pub status: StorageScanStatus,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub roots: Vec<StorageRootUsage>,
    pub errors: Vec<StoragePathError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageScanProgress {
    pub roots: Vec<StorageRootUsage>,
    pub errors: Vec<StoragePathError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanJobFailure {
    pub is_abort_error: bool,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanJobProgressAction {
    Emitted(StorageUsageSnapshot),
    TimerScheduled { delay_ms: i64 },
    Ignored,
}

pub struct StorageScanJob {
    job_id: String,
    throttle_ms: i64,
    latest: StorageUsageSnapshot,
    last_emit_at: Option<i64>,
    timer_pending: bool,
    settled: bool,
    aborted: bool,
}

impl StorageScanJob {
    pub fn new(
        job_id: String,
        roots: &[crate::StorageRootSpec],
        now: i64,
        throttle_ms: i64,
    ) -> Self {
        let placeholders = roots
            .iter()
            .map(|root| StorageRootUsage {
                id: root.id,
                path: root.path.clone(),
                volume: None,
                bytes: 0,
                file_count: 0,
                categories: Vec::new(),
            })
            .collect();
        Self {
            job_id: job_id.clone(),
            throttle_ms,
            latest: StorageUsageSnapshot {
                job_id,
                status: StorageScanStatus::Scanning,
                started_at: now,
                finished_at: None,
                roots: placeholders,
                errors: Vec::new(),
            },
            last_emit_at: None,
            timer_pending: false,
            settled: false,
            aborted: false,
        }
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn cancel(&mut self) {
        self.aborted = true;
    }

    pub fn is_aborted(&self) -> bool {
        self.aborted
    }

    pub fn progress(&mut self, progress: StorageScanProgress, now: i64) -> ScanJobProgressAction {
        if self.settled {
            return ScanJobProgressAction::Ignored;
        }
        self.latest.roots = progress.roots;
        self.latest.errors = progress.errors;
        let elapsed = self.last_emit_at.map_or(i64::MAX, |last| now - last);
        if elapsed >= self.throttle_ms {
            self.timer_pending = false;
            self.last_emit_at = Some(now);
            return ScanJobProgressAction::Emitted(self.latest.clone());
        }
        if !self.timer_pending {
            self.timer_pending = true;
            return ScanJobProgressAction::TimerScheduled {
                delay_ms: self.throttle_ms - elapsed,
            };
        }
        ScanJobProgressAction::Ignored
    }

    pub fn timer_fired(&mut self, now: i64) -> Option<StorageUsageSnapshot> {
        if self.settled || !self.timer_pending {
            return None;
        }
        self.timer_pending = false;
        self.last_emit_at = Some(now);
        Some(self.latest.clone())
    }

    pub fn finish_success(
        &mut self,
        progress: StorageScanProgress,
        now: i64,
    ) -> StorageUsageSnapshot {
        self.latest.roots = progress.roots;
        self.latest.errors = progress.errors;
        self.latest.status = if self.aborted {
            StorageScanStatus::Cancelled
        } else {
            StorageScanStatus::Complete
        };
        self.latest.finished_at = Some(now);
        self.settle()
    }

    pub fn finish_error(&mut self, failure: ScanJobFailure, now: i64) -> StorageUsageSnapshot {
        self.latest.status = if failure.is_abort_error || self.aborted {
            StorageScanStatus::Cancelled
        } else {
            StorageScanStatus::Failed
        };
        self.latest.finished_at = Some(now);
        if !failure.is_abort_error {
            self.latest.errors.push(StoragePathError {
                path: String::new(),
                code: failure.code,
            });
        }
        self.settle()
    }

    pub fn snapshot(&self) -> &StorageUsageSnapshot {
        &self.latest
    }

    fn settle(&mut self) -> StorageUsageSnapshot {
        self.settled = true;
        self.timer_pending = false;
        self.latest.clone()
    }
}

impl StoragePathError {
    pub fn to_json(&self) -> Value {
        json!({ "path": self.path, "code": self.code })
    }
}

impl StorageUsageSnapshot {
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "jobId": self.job_id,
            "status": self.status.as_str(),
            "startedAt": self.started_at,
            "roots": self.roots.iter().map(StorageRootUsage::to_json).collect::<Vec<_>>(),
            "errors": self.errors.iter().map(StoragePathError::to_json).collect::<Vec<_>>(),
        });
        if let Some(finished_at) = self.finished_at {
            value["finishedAt"] = json!(finished_at);
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StorageRootId, StorageRootSpec};

    fn root() -> StorageRootSpec {
        StorageRootSpec {
            id: StorageRootId::Home,
            path: "/home/u/.mode".to_string(),
            has_custom_data_base_dir: false,
        }
    }

    fn progress(bytes: i64) -> StorageScanProgress {
        StorageScanProgress {
            roots: vec![StorageRootUsage {
                id: StorageRootId::Home,
                path: "/home/u/.mode".to_string(),
                volume: None,
                bytes,
                file_count: 1,
                categories: Vec::new(),
            }],
            errors: Vec::new(),
        }
    }

    #[test]
    fn throttles_progress_and_flushes_timer() {
        let mut job = StorageScanJob::new("scan-1".to_string(), &[root()], 1_000, 300);
        assert!(matches!(
            job.progress(progress(1), 1_000),
            ScanJobProgressAction::Emitted(_)
        ));
        assert!(matches!(
            job.progress(progress(2), 1_100),
            ScanJobProgressAction::TimerScheduled { delay_ms: 200 }
        ));
        assert!(matches!(
            job.progress(progress(3), 1_200),
            ScanJobProgressAction::Ignored
        ));
        let flushed = job.timer_fired(1_300).expect("timer emits");
        assert_eq!(flushed.roots[0].bytes, 3);
    }

    #[test]
    fn abort_maps_to_cancelled() {
        let mut job = StorageScanJob::new("scan-2".to_string(), &[root()], 0, 300);
        job.cancel();
        let snapshot = job.finish_error(
            ScanJobFailure {
                is_abort_error: true,
                code: "AbortError".to_string(),
            },
            10,
        );
        assert_eq!(snapshot.status, StorageScanStatus::Cancelled);
        assert!(snapshot.errors.is_empty());
        assert_eq!(snapshot.finished_at, Some(10));
    }
}
