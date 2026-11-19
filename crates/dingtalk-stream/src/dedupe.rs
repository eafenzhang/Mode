//! 入站消息去重（TS `dingtalkChannelRuntime.ts:34-36,107-146` 的对齐）。
//!
//! 服务端可能重复推送（官方：网络抖动触发超时重推），去重是**必须**。语义：
//! - 重复判定按插入序淘汰（`snapshot` 时截断到 200 条 / 24h TTL 过滤在加载时做）；
//! - 与 TS 的一致点：`is_duplicate` 不刷新重复项的时间戳、TTL 只作用于启动加载；
//! - 与 TS 的差异（已登记）：TS 的内存 Map 只增不减（持久化时只截断落盘数组），
//!   这里 `snapshot` 同时裁剪内存——对外行为无差（去重窗口仍是 200 条），只修内存占用。

use std::collections::{HashMap, VecDeque};

/// TS :34-35 的两个窗口常量。
pub const MESSAGE_DEDUPE_LIMIT: usize = 200;
pub const MESSAGE_DEDUPE_TTL_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupeEntry {
    pub id: String,
    pub at: u64,
}

#[derive(Debug)]
pub struct DedupeSet {
    order: VecDeque<(String, u64)>,
    index: HashMap<String, u64>,
    limit: usize,
    ttl_ms: u64,
}

impl Default for DedupeSet {
    fn default() -> Self {
        Self::with_windows(MESSAGE_DEDUPE_LIMIT, MESSAGE_DEDUPE_TTL_MS)
    }
}

impl DedupeSet {
    /// 生产窗口：200 条 / 24h（TS :34-35）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 测试/定制窗口。
    pub fn with_windows(limit: usize, ttl_ms: u64) -> Self {
        Self {
            order: VecDeque::new(),
            index: HashMap::new(),
            limit,
            ttl_ms,
        }
    }

    /// 从落盘恢复：过滤过期（TS :110-114），保留插入序；窗口沿用构造参数。
    pub fn from_entries(
        entries: impl IntoIterator<Item = DedupeEntry>,
        limit: usize,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Self {
        let mut set = Self::with_windows(limit, ttl_ms);
        for entry in entries {
            if now_ms.saturating_sub(entry.at) < set.ttl_ms {
                set.index.insert(entry.id.clone(), entry.at);
                set.order.push_back((entry.id, entry.at));
            }
        }
        set
    }

    /// 已见过 → true；否则记录并返回 false。重复项**不刷新**时间戳（TS :139-146）。
    pub fn is_duplicate(&mut self, id: &str, now_ms: u64) -> bool {
        if self.index.contains_key(id) {
            return true;
        }
        self.index.insert(id.to_string(), now_ms);
        self.order.push_back((id.to_string(), now_ms));
        false
    }

    /// 持久化快照：取最后 `limit` 条，并把内存裁剪到同一窗口。
    pub fn snapshot(&mut self) -> Vec<DedupeEntry> {
        while self.order.len() > self.limit {
            if let Some((oldest, _)) = self.order.pop_front() {
                self.index.remove(&oldest);
            }
        }
        self.order
            .iter()
            .map(|(id, at)| DedupeEntry {
                id: id.clone(),
                at: *at,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_are_detected_without_touching_timestamps() {
        let mut set = DedupeSet::with_windows(200, 1000);
        assert!(!set.is_duplicate("a", 0));
        assert!(set.is_duplicate("a", 500));
        let snapshot = set.snapshot();
        assert_eq!(snapshot[0].at, 0, "重复项不刷新时间戳（TS :139-146）");
        assert_eq!(snapshot.len(), 1);
    }

    #[test]
    fn snapshot_trims_to_limit_dropping_oldest() {
        let mut set = DedupeSet::with_windows(2, 1000);
        set.is_duplicate("a", 1);
        set.is_duplicate("b", 2);
        set.is_duplicate("c", 3);
        let snapshot = set.snapshot();
        assert_eq!(
            snapshot.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["b", "c"]
        );
        // 裁剪后 "a" 不再是重复项（窗口内已淘汰）。
        assert!(!set.is_duplicate("a", 4));
    }

    #[test]
    fn from_entries_filters_expired_entries() {
        let mut set = DedupeSet::from_entries(
            [
                DedupeEntry {
                    id: "old".into(),
                    at: 0,
                },
                DedupeEntry {
                    id: "fresh".into(),
                    at: 900,
                },
            ],
            200,
            1_000, // TTL 1000：at=0 已过期
            1_000,
        );
        assert!(set.is_duplicate("fresh", 1_100));
        assert!(!set.is_duplicate("old", 1_100));
    }

    #[test]
    fn default_window_matches_ts_constants() {
        assert_eq!(MESSAGE_DEDUPE_LIMIT, 200);
        assert_eq!(MESSAGE_DEDUPE_TTL_MS, 24 * 60 * 60 * 1000);
    }
}
