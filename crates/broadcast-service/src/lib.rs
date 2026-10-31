//! 广播与 opaque claim（local-only 实现）。
//!
//! 真源：`packages/services/src/broadcast/broadcastService.ts` 在 `parentPort === null`
//! 时的分支（Tauri 线 `host-standalone` 正是这样构造的：没有跨 Host 中转）。
//!
//! 语义：
//! - `send` 校验消息后分发给本地订阅者（同窗口 Renderer 通过 RPC event 收到）；
//! - claim 是进程内状态机：`reserved`（TTL 5s）→ `committed`（永久）或 `release`；
//!   `busy` 的 `retryAfterMs` 上限 250ms；空 key 返回 `unavailable`；
//! - 最多保留 1024 个 claim，超限按插入序淘汰最旧的。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const CLAIM_RESERVATION_TTL_MS: u64 = 5_000;
const CLAIM_RETRY_MS: u64 = 250;
const MAX_LOCAL_CLAIMS: usize = 1_024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastMessage {
    pub channel: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_window_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimLease {
    pub key: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ClaimAcquireResult {
    #[serde(rename_all = "camelCase")]
    Acquired {
        lease: ClaimLease,
    },
    #[serde(rename_all = "camelCase")]
    Busy {
        retry_after_ms: u64,
    },
    Committed,
    Unavailable,
}

#[derive(Clone)]
struct ClaimRecord {
    token: String,
    status: ClaimStatus,
    expires_at: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClaimStatus {
    Reserved,
    Committed,
}

type Listener = Arc<dyn Fn(&BroadcastMessage) + Send + Sync>;
type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub struct BroadcastService {
    listeners: Mutex<Vec<(u64, Listener)>>,
    next_listener_id: AtomicU64,
    claims: Mutex<VecDeque<(String, ClaimRecord)>>,
    clock: Clock,
    sequence: AtomicU64,
}

impl BroadcastService {
    pub fn new() -> Self {
        let initial = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let base = Arc::new(AtomicU64::new(initial));
        Self::with_clock(Arc::new(move || base.load(Ordering::Relaxed)))
    }

    /// 测试注入时钟（毫秒）；只影响 claim 的 TTL 判定。
    pub fn with_clock(clock: Clock) -> Self {
        Self {
            listeners: Mutex::new(Vec::new()),
            next_listener_id: AtomicU64::new(0),
            claims: Mutex::new(VecDeque::new()),
            clock,
            sequence: AtomicU64::new(0),
        }
    }

    /// 订阅 `onMessage`；`Subscription` 被丢弃时自动反订阅。
    pub fn subscribe(self: &Arc<Self>, listener: Listener) -> Subscription {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.listeners.lock().unwrap().push((id, listener));
        Subscription {
            service: Arc::downgrade(self),
            id,
        }
    }

    fn remove_listener(&self, id: u64) {
        self.listeners
            .lock()
            .unwrap()
            .retain(|(existing, _)| *existing != id);
    }

    /// `send`：校验必填字段后分发给本地订阅者。
    pub fn send(&self, message: &Value) -> Result<BroadcastMessage, String> {
        let object = message
            .as_object()
            .ok_or_else(|| "Invalid broadcast message: expected object".to_string())?;
        let channel = object
            .get("channel")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Invalid broadcast message: channel is required".to_string())?
            .to_string();
        let source_window_id = match object.get("sourceWindowId") {
            None | Some(Value::Null) => None,
            Some(Value::Number(number)) => {
                let Some(value) = number.as_i64() else {
                    return Err(
                        "Invalid broadcast message: sourceWindowId must be an integer".into(),
                    );
                };
                Some(value)
            }
            Some(_) => {
                return Err("Invalid broadcast message: sourceWindowId must be an integer".into())
            }
        };
        let validated = BroadcastMessage {
            channel,
            payload: object.get("payload").cloned().unwrap_or(Value::Null),
            source_window_id,
        };
        self.emit(&validated);
        Ok(validated)
    }

    fn emit(&self, message: &BroadcastMessage) {
        let listeners: Vec<Listener> = self
            .listeners
            .lock()
            .unwrap()
            .iter()
            .map(|(_, listener)| Arc::clone(listener))
            .collect();
        for listener in listeners {
            listener(message);
        }
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    fn next_token(&self, prefix: &str) -> String {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        format!("local-{prefix}-{}-{sequence}", self.now())
    }

    fn prune(&self, claims: &mut VecDeque<(String, ClaimRecord)>, now: u64) {
        claims.retain(|(_, claim)| {
            !(claim.status == ClaimStatus::Reserved
                && claim
                    .expires_at
                    .map(|expires| expires <= now)
                    .unwrap_or(false))
        });
        while claims.len() > MAX_LOCAL_CLAIMS {
            claims.pop_front();
        }
    }

    pub fn acquire_claim(&self, key: &str) -> ClaimAcquireResult {
        let normalized = key.trim();
        if normalized.is_empty() {
            return ClaimAcquireResult::Unavailable;
        }
        let now = self.now();
        let mut claims = self.claims.lock().unwrap();
        self.prune(&mut claims, now);
        if let Some((_, existing)) = claims.iter().find(|(claim_key, _)| claim_key == normalized) {
            if existing.status == ClaimStatus::Committed {
                return ClaimAcquireResult::Committed;
            }
            let remaining = existing.expires_at.unwrap_or(now).saturating_sub(now);
            return ClaimAcquireResult::Busy {
                retry_after_ms: remaining.min(CLAIM_RETRY_MS),
            };
        }
        let lease = ClaimLease {
            key: normalized.to_string(),
            token: self.next_token(normalized),
        };
        claims.push_back((
            normalized.to_string(),
            ClaimRecord {
                token: lease.token.clone(),
                status: ClaimStatus::Reserved,
                expires_at: Some(now + CLAIM_RESERVATION_TTL_MS),
            },
        ));
        self.prune(&mut claims, now);
        ClaimAcquireResult::Acquired { lease }
    }

    pub fn commit_claim(&self, lease: &ClaimLease) {
        let now = self.now();
        let mut claims = self.claims.lock().unwrap();
        self.prune(&mut claims, now);
        if let Some((_, record)) = claims.iter_mut().find(|(key, _)| key == &lease.key) {
            if record.status == ClaimStatus::Reserved && record.token == lease.token {
                record.status = ClaimStatus::Committed;
                record.expires_at = None;
            }
        }
    }

    pub fn release_claim(&self, lease: &ClaimLease) {
        let now = self.now();
        let mut claims = self.claims.lock().unwrap();
        self.prune(&mut claims, now);
        if let Some(index) = claims.iter().position(|(key, _)| key == &lease.key) {
            let matches = claims[index].1.status == ClaimStatus::Reserved
                && claims[index].1.token == lease.token;
            if matches {
                claims.remove(index);
            }
        }
    }

    pub fn try_claim(&self, key: &str) -> bool {
        match self.acquire_claim(key) {
            ClaimAcquireResult::Acquired { lease } => {
                self.commit_claim(&lease);
                true
            }
            _ => false,
        }
    }

    /// 测试辅助：当前 claim 数。
    pub fn claim_count(&self) -> usize {
        self.claims.lock().unwrap().len()
    }
}

impl Default for BroadcastService {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Subscription {
    service: std::sync::Weak<BroadcastService>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(service) = self.service.upgrade() {
            service.remove_listener(self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn service_at(clock: Arc<AtomicU64>) -> BroadcastService {
        BroadcastService::with_clock(Arc::new(move || clock.load(Ordering::Relaxed)))
    }

    #[test]
    fn claim_lifecycle() {
        let clock = Arc::new(AtomicU64::new(1_000));
        let service = service_at(clock.clone());
        let acquired = service.acquire_claim("key");
        let lease = match acquired {
            ClaimAcquireResult::Acquired { lease } => lease,
            other => panic!("expected acquired, got {other:?}"),
        };
        assert!(matches!(
            service.acquire_claim("key"),
            ClaimAcquireResult::Busy { retry_after_ms } if retry_after_ms == 250
        ));
        service.commit_claim(&lease);
        assert_eq!(service.acquire_claim("key"), ClaimAcquireResult::Committed);
        service.release_claim(&lease);
        assert_eq!(service.acquire_claim("key"), ClaimAcquireResult::Committed);
        assert_eq!(service.claim_count(), 1);
    }

    #[test]
    fn reservation_expires_after_ttl() {
        let clock = Arc::new(AtomicU64::new(1_000));
        let service = service_at(clock.clone());
        assert!(matches!(
            service.acquire_claim("k"),
            ClaimAcquireResult::Acquired { .. }
        ));
        clock.fetch_add(5_001, Ordering::Relaxed);
        assert!(matches!(
            service.acquire_claim("k"),
            ClaimAcquireResult::Acquired { .. }
        ));
    }

    #[test]
    fn empty_key_is_unavailable_and_try_claim_commits() {
        let clock = Arc::new(AtomicU64::new(1_000));
        let service = service_at(clock);
        assert_eq!(
            service.acquire_claim("   "),
            ClaimAcquireResult::Unavailable
        );
        assert!(service.try_claim("once"));
        assert!(!service.try_claim("once"));
    }

    #[test]
    fn send_validates_and_notifies_listeners() {
        let service = Arc::new(BroadcastService::new());
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = received.clone();
        let subscription = service.subscribe(Arc::new(move |message: &BroadcastMessage| {
            sink.lock().unwrap().push(message.clone());
        }));
        let message = service
            .send(&serde_json::json!({
                "channel": "state:theme",
                "payload": { "theme": "dark" },
                "sourceWindowId": 3,
            }))
            .unwrap();
        assert_eq!(message.channel, "state:theme");
        assert_eq!(
            received.lock().unwrap()[0],
            BroadcastMessage {
                channel: "state:theme".into(),
                payload: serde_json::json!({ "theme": "dark" }),
                source_window_id: Some(3),
            }
        );
        assert!(service.send(&serde_json::json!({ "payload": 1 })).is_err());
        assert!(service
            .send(&serde_json::json!({ "channel": "x", "sourceWindowId": 1.5 }))
            .is_err());
        drop(subscription);
        assert!(service.listeners.lock().unwrap().is_empty());
    }
}
