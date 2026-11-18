//! 会话事件发布器：seq 分配、提交排序、内存日志与 broadcast 的唯一所有者。
//!
//! 为什么需要它（C3 T1 round-3）：`Session` 与 `agent-server` 方法层如果各自持有
//! 计数器，in-flight 流式事件与 pause/stop 预分配事件会拿到同一个 seq。这里把
//! **分配**与**提交**放在同一把锁下：
//!
//! - [`EventPublisher::allocate`] 只分配 seq 并返回事件（不发布）；`Session::emit`
//!   在持久化（EventSink）之前调用它，保证 `eventId` 在事件对象里可读；
//! - [`EventPublisher::commit`] 由 EventSink/方法层在落库后调用；提交按 seq 连续
//!   推进（pending 中缺号时后序事件等待），因此广播/日志顺序严格单调；
//! - [`EventPublisher::publish`] 是方法层的原子快捷方式（分配 + 提交一次完成）；
//! - 日志 replay 与 broadcast receiver 在同一把锁内建立（无重复、无缺口）。
//!
//! `current_seq()` 返回**已分配**的最高 seq（含尚未提交的 pause/stop 事件），
//! 快照的 `runtime.eventSeq` 用它；日志/广播只含已提交事件。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use agent_protocol::session_event::{DeliveryKind, SessionEvent, SessionEventType};
use serde_json::Value;

struct PublisherState {
    next_seq: u64,
    committed_upto: u64,
    pending: BTreeMap<u64, SessionEvent>,
    log: VecDeque<SessionEvent>,
    log_capacity: usize,
}

/// 一个会话的事件发布器（`Session` 与 `SessionRuntime` 共享同一个 Arc）。
pub struct EventPublisher {
    session_id: String,
    state: Mutex<PublisherState>,
    bus: tokio::sync::broadcast::Sender<SessionEvent>,
}

impl std::fmt::Debug for EventPublisher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EventPublisher")
            .field("session_id", &self.session_id)
            .field("seq", &self.current_seq())
            .finish()
    }
}

impl EventPublisher {
    /// 新建发布器；`broadcast_capacity` / `log_capacity` 至少为 1。
    pub fn new(
        session_id: impl Into<String>,
        broadcast_capacity: usize,
        log_capacity: usize,
    ) -> Arc<Self> {
        let (bus, _receiver) = tokio::sync::broadcast::channel(broadcast_capacity.max(1));
        Arc::new(Self {
            session_id: session_id.into(),
            state: Mutex::new(PublisherState {
                next_seq: 0,
                committed_upto: 0,
                pending: BTreeMap::new(),
                log: VecDeque::new(),
                log_capacity: log_capacity.max(1),
            }),
            bus,
        })
    }

    fn lock(&self) -> MutexGuard<'_, PublisherState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 分配下一个 seq 并构造事件；**不**写入日志/广播。
    pub fn allocate(
        &self,
        turn_id: Option<String>,
        event_type: SessionEventType,
        payload: Value,
        delivery_kind: DeliveryKind,
    ) -> SessionEvent {
        let mut state = self.lock();
        state.next_seq += 1;
        SessionEvent {
            event_id: format!("evt_{}", state.next_seq),
            session_id: self.session_id.clone(),
            turn_id,
            seq: state.next_seq,
            trace_id: None,
            timestamp: now_ms(),
            delivery_kind: Some(delivery_kind),
            event_type,
            payload: Some(payload),
        }
    }

    /// 提交一个已分配事件：落日志 + 广播，并按 seq 连续推进（缺号则等待）。
    pub fn commit(&self, event: &SessionEvent) {
        let mut state = self.lock();
        if event.seq == 0 || event.seq <= state.committed_upto {
            return;
        }
        state.pending.insert(event.seq, event.clone());
        loop {
            let next_seq = state.committed_upto + 1;
            let Some(next) = state.pending.remove(&next_seq) else {
                break;
            };
            state.committed_upto += 1;
            state.log.push_back(next.clone());
            while state.log.len() > state.log_capacity {
                state.log.pop_front();
            }
            // 没有订阅方时 send 返回 Err：事件仍在内存日志里，回放可补。
            let _ = self.bus.send(next);
        }
    }

    /// 分配 + 提交（方法层的同步事件）。
    pub fn publish(
        &self,
        turn_id: Option<String>,
        event_type: SessionEventType,
        payload: Value,
        delivery_kind: DeliveryKind,
    ) -> SessionEvent {
        let event = self.allocate(turn_id, event_type, payload, delivery_kind);
        self.commit(&event);
        event
    }

    /// 已分配的最高 seq（含尚未提交的事件）。
    pub fn current_seq(&self) -> u64 {
        self.lock().next_seq
    }

    /// 订阅：日志 replay + broadcast receiver（同一临界区，无重复/缺口）。
    ///
    /// `after_seq` 大于已分配 seq 视为旧 runtime 游标（reset）。
    pub fn subscribe(
        &self,
        after_seq: Option<u64>,
    ) -> (
        Vec<SessionEvent>,
        tokio::sync::broadcast::Receiver<SessionEvent>,
        Option<u64>,
    ) {
        let state = self.lock();
        let effective = match after_seq {
            Some(after) if after <= state.next_seq => Some(after),
            _ => None,
        };
        let replay = state
            .log
            .iter()
            .filter(|event| match effective {
                None => true,
                Some(after) => event.seq > after,
            })
            .cloned()
            .collect();
        let receiver = self.bus.subscribe();
        (replay, receiver, effective)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_type() -> SessionEventType {
        SessionEventType::SessionUpdated
    }

    #[test]
    fn allocation_is_shared_and_commit_flushes_in_seq_order() {
        let publisher = EventPublisher::new("sess", 8, 8);
        let first = publisher.allocate(
            None,
            event_type(),
            serde_json::json!({ "n": 1 }),
            DeliveryKind::DesktopContinuous,
        );
        let second = publisher.allocate(
            None,
            event_type(),
            serde_json::json!({ "n": 2 }),
            DeliveryKind::DesktopContinuous,
        );
        assert_eq!((first.seq, second.seq), (1, 2));
        assert_eq!(publisher.current_seq(), 2);

        // 后序先提交：在缺号补齐前不能出现在日志/总线里。
        publisher.commit(&second);
        let (replay, _receiver, _) = publisher.subscribe(None);
        assert!(replay.is_empty(), "seq 1 未提交时 2 不能先发布");
        publisher.commit(&first);
        let (replay, _receiver, _) = publisher.subscribe(None);
        assert_eq!(
            replay.iter().map(|event| event.seq).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(replay[0].event_id, "evt_1");
        assert_eq!(replay[1].event_id, "evt_2");

        // 重复提交不会产生第二个事件。
        publisher.commit(&first);
        let (replay, _receiver, _) = publisher.subscribe(None);
        assert_eq!(replay.len(), 2);
    }

    #[test]
    fn publish_assigns_and_publishes_atomically() {
        let publisher = EventPublisher::new("sess", 4, 4);
        let event = publisher.publish(
            None,
            event_type(),
            serde_json::json!({}),
            DeliveryKind::DesktopContinuous,
        );
        assert_eq!(event.seq, 1);
        let (replay, _receiver, effective) = publisher.subscribe(Some(0));
        assert_eq!(effective, Some(0));
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].event_id, "evt_1");
    }
}
