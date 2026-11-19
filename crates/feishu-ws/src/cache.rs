//! 分片重组 DataCache（spec §5.3 第 3 条；SDK :85202-85259 的对齐）。
//!
//! 服务端把一条事件拆成 `sum` 个片段（`headers.sum`），每片带 `seq` 序号；
//! 只有全部片段到齐才按 seq 顺序拼接 → UTF-8 → `JSON.parse` → 派发。
//! 未完成分片 10s 过期清理（SDK 的 magic number）。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

pub const CACHE_EXPIRY: Duration = Duration::from_secs(10);

struct Pending {
    chunks: Vec<Option<Vec<u8>>>,
    created_at: Instant,
}

#[derive(Default)]
pub struct DataCache {
    pending: HashMap<String, Pending>,
}

impl DataCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 与 SDK `mergeData` 同语义：按 `(message_id, sum, seq)` 落片；未齐返回 None；
    /// 齐了按 seq 顺序拼接并 `JSON.parse`（解析失败返回 None——SDK 的 JSON.parse 抛错
    /// 会冒泡到 message 处理器，这里降级为“丢弃该事件”，调用方留日志）。
    pub fn merge(
        &mut self,
        message_id: &str,
        sum: usize,
        seq: usize,
        chunk: Vec<u8>,
        now: Instant,
    ) -> Option<Value> {
        if sum == 0 || seq >= sum {
            return None;
        }
        let entry = self
            .pending
            .entry(message_id.to_string())
            .or_insert_with(|| Pending {
                chunks: vec![None; sum],
                created_at: now,
            });
        // 分片数不一致时以首个到达的 sum 为准（SDK 同样只按首次 sum 建 buffer）。
        if let Some(slot) = entry.chunks.get_mut(seq) {
            *slot = Some(chunk);
        }
        if entry.chunks.iter().all(Option::is_some) {
            let mut merged = Vec::new();
            for chunk in entry.chunks.iter().flatten() {
                merged.extend_from_slice(chunk);
            }
            self.pending.remove(message_id);
            return serde_json::from_slice(&merged).ok();
        }
        None
    }

    /// 清理超过 `expiry` 的未完成分片（SDK `clearAtInterval` 10s 周期）。
    pub fn expire(&mut self, now: Instant, expiry: Duration) -> usize {
        let before = self.pending.len();
        self.pending
            .retain(|_, entry| now.saturating_duration_since(entry.created_at) <= expiry);
        before - self.pending.len()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_out_of_order_chunks_by_seq() {
        let mut cache = DataCache::new();
        let now = Instant::now();
        let payload = br#"{"schema":"2.0","event":{"type":"im.message.receive_v1"}}"#;
        let split = 20;
        assert!(cache
            .merge("m-1", 2, 1, payload[split..].to_vec(), now)
            .is_none());
        let merged = cache
            .merge("m-1", 2, 0, payload[..split].to_vec(), now)
            .expect("全部到齐应合并");
        assert_eq!(merged["schema"], serde_json::json!("2.0"));
        assert_eq!(
            merged["event"]["type"],
            serde_json::json!("im.message.receive_v1")
        );
        assert!(cache.is_empty(), "合并后应删缓存");
    }

    #[test]
    fn duplicate_seq_overwrites_and_still_merges() {
        let mut cache = DataCache::new();
        let now = Instant::now();
        cache.merge("m-2", 2, 0, b"{\"a\":".to_vec(), now);
        cache.merge("m-2", 2, 0, b"{\"a\":".to_vec(), now);
        let merged = cache
            .merge("m-2", 2, 1, b"1}".to_vec(), now)
            .expect("应合并");
        assert_eq!(merged["a"], serde_json::json!(1));
    }

    #[test]
    fn expires_stale_pending_chunks() {
        let mut cache = DataCache::new();
        let now = Instant::now();
        cache.merge("m-3", 2, 0, b"{}".to_vec(), now);
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.expire(now + Duration::from_secs(11), CACHE_EXPIRY),
            1,
            "10s 后应过期"
        );
        assert!(cache.is_empty());
    }

    #[test]
    fn invalid_sum_or_seq_is_ignored() {
        let mut cache = DataCache::new();
        let now = Instant::now();
        assert!(cache.merge("m-4", 0, 0, b"{}".to_vec(), now).is_none());
        assert!(cache.merge("m-4", 2, 5, b"{}".to_vec(), now).is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn non_json_payload_is_dropped() {
        let mut cache = DataCache::new();
        let now = Instant::now();
        assert!(cache
            .merge("m-5", 1, 0, b"not-json".to_vec(), now)
            .is_none());
        assert!(cache.is_empty());
    }
}
