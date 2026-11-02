//! 存储清理计划：给定类别与候选文件，决定哪些能删。
//!
//! 真源：`packages/services/src/storage/domain/cleanPlan.ts`。
//! 本地日判断由调用方传入固定时区偏移秒数（TS 使用系统本地时区）。

use std::collections::HashMap;

use crate::catalog::{
    classify_storage_path, get_storage_category_cleanability, is_protected_storage_path,
    StorageCatalogContext, StorageCategoryId, StorageCleanability,
};

/// 子代理产物：会话目录 24 小时内有更新就整个跳过。
pub const SUBAGENT_ACTIVE_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
const MS_PER_DAY: i64 = 86_400_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageCleanCandidate {
    pub relative_path: String,
    pub bytes: i64,
    pub mtime_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageCleanParams<'a> {
    pub category_id: StorageCategoryId,
    pub candidates: &'a [StorageCleanCandidate],
    pub context: StorageCatalogContext,
    pub now: i64,
    pub offset_secs: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageCleanPlan {
    pub targets: Vec<StorageCleanCandidate>,
    pub skipped_count: usize,
}

fn is_same_local_day(a: i64, b: i64, offset_secs: i32) -> bool {
    // 固定偏移下“同一个本地日期”等价于本地 1970-01-01 起的天数相同。
    let offset_ms = offset_secs as i64 * 1000;
    (a + offset_ms).div_euclid(MS_PER_DAY) == (b + offset_ms).div_euclid(MS_PER_DAY)
}

fn session_key(path: &str) -> String {
    path.split('/').take(3).collect::<Vec<_>>().join("/")
}

fn filter_inactive_session_dirs(
    targets: &[StorageCleanCandidate],
    all_candidates: &[StorageCleanCandidate],
    now: i64,
) -> Vec<StorageCleanCandidate> {
    let mut latest_by_session: HashMap<String, i64> = HashMap::new();
    for candidate in all_candidates {
        let key = session_key(&candidate.relative_path);
        let latest = latest_by_session.entry(key).or_insert(0);
        *latest = (*latest).max(candidate.mtime_ms);
    }
    targets
        .iter()
        .filter(|candidate| {
            let latest = latest_by_session
                .get(&session_key(&candidate.relative_path))
                .copied()
                .unwrap_or(0);
            now - latest > SUBAGENT_ACTIVE_WINDOW_MS
        })
        .cloned()
        .collect()
}

pub fn plan_storage_clean(params: &StorageCleanParams<'_>) -> StorageCleanPlan {
    if get_storage_category_cleanability(params.category_id) == StorageCleanability::None {
        return StorageCleanPlan {
            targets: Vec::new(),
            skipped_count: params.candidates.len(),
        };
    }
    // 候选来自按前缀枚举，可能混入其他类别；只保留分类一致且未受保护的。
    let owned: Vec<StorageCleanCandidate> = params
        .candidates
        .iter()
        .filter(|candidate| {
            classify_storage_path(&candidate.relative_path, &params.context).category_id
                == params.category_id
                && !is_protected_storage_path(&candidate.relative_path)
        })
        .cloned()
        .collect();
    let targets = if params.category_id == StorageCategoryId::Logs {
        owned
            .into_iter()
            .filter(|candidate| {
                !is_same_local_day(candidate.mtime_ms, params.now, params.offset_secs)
            })
            .collect()
    } else if params.category_id == StorageCategoryId::SubagentTranscripts {
        filter_inactive_session_dirs(&owned, params.candidates, params.now)
    } else {
        owned
    };
    StorageCleanPlan {
        skipped_count: params.candidates.len() - targets.len(),
        targets,
    }
}

impl StorageCleanCandidate {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "relativePath": self.relative_path,
            "bytes": self.bytes,
            "mtimeMs": self.mtime_ms,
        })
    }
}

impl StorageCleanPlan {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "targets": self.targets.iter().map(StorageCleanCandidate::to_json).collect::<Vec<_>>(),
            "skippedCount": self.skipped_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(path: &str, mtime_ms: i64) -> StorageCleanCandidate {
        StorageCleanCandidate {
            relative_path: path.to_string(),
            bytes: 1,
            mtime_ms,
        }
    }

    #[test]
    fn skips_same_local_day_logs() {
        let now = 86_400_000 * 2 + 1_000;
        let candidates = vec![
            candidate("logs/a.log", now),
            candidate("logs/b.log", now - MS_PER_DAY),
        ];
        let plan = plan_storage_clean(&StorageCleanParams {
            category_id: StorageCategoryId::Logs,
            candidates: &candidates,
            context: StorageCatalogContext {
                root_id: crate::StorageRootId::Home,
                has_custom_data_base_dir: false,
            },
            now,
            offset_secs: 0,
        });
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].relative_path, "logs/b.log");
        assert_eq!(plan.skipped_count, 1);
    }

    #[test]
    fn session_activity_includes_non_owned_files() {
        let now = 10 * MS_PER_DAY;
        let candidates = vec![
            candidate("cli/agents/s1/x/transcript.jsonl", 0),
            candidate("cli/agents/s1/x/metadata.json", now),
            candidate("cli/agents/s2/x/transcript.jsonl", 0),
            candidate("cli/agents/s2/x/metadata.json", 0),
        ];
        let plan = plan_storage_clean(&StorageCleanParams {
            category_id: StorageCategoryId::SubagentTranscripts,
            candidates: &candidates,
            context: StorageCatalogContext {
                root_id: crate::StorageRootId::Home,
                has_custom_data_base_dir: false,
            },
            now,
            offset_secs: 0,
        });
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(
            plan.targets[0].relative_path,
            "cli/agents/s2/x/transcript.jsonl"
        );
        assert_eq!(plan.skipped_count, 3);
    }
}
