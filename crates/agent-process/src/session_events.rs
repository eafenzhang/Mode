//! `session/subscribe` 与 `session/event` 实时流。
//!
//! 行为真源：`packages/services/src/mode-agent/modeAgentService.ts` 的
//! `onDynamicSessionEvent` / `normalizeSessionEventSeq` / `shouldDeliverLiveSessionEvent`：
//! 订阅先拿 replay（按 seq/timestamp 排序），再接收 `session/event` 通知；同一事件 id 只投递
//! 一次；`seq == 0` 的旧事件按订阅内单调序号补齐。
//!
//! 规范见 `docs/specs/session-write-group.md` §「事件订阅」。

use std::collections::{HashMap, HashSet, VecDeque};

use tokio::sync::{broadcast, mpsc};
use tokio::task::AbortHandle;

use agent_protocol::session_event::SessionEvent;
use agent_protocol::session_read::{
    SessionSubscribeParams, SessionSubscribeResult, SESSION_SUBSCRIBE_METHOD,
};
use agent_protocol::snapshot::{DeliveryKind, SessionStateSnapshot};

use crate::control::{AgentControlClient, AgentWorkspaceTarget, ControlError, RuntimePolicy};

pub const SESSION_EVENT_NOTIFICATION: &str = "session/event";

/// 与 TS `MAX_TRACKED_SESSION_EVENT_IDS` 同量级的有界去重窗口。
const MAX_TRACKED_EVENT_IDS: usize = 2000;
const LIVE_CHANNEL_CAPACITY: usize = 256;

/// 订阅建立后的句柄：replay 快照 + 实时事件流；`Drop` 时停止转发任务。
pub struct SessionSubscription {
    /// `session/subscribe` 返回的补洞事件（已排序、已按 id 去重）。
    pub replay: Vec<SessionEvent>,
    pub snapshot: Option<SessionStateSnapshot>,
    events: mpsc::Receiver<SessionEvent>,
    abort: AbortHandle,
}

impl SessionSubscription {
    /// 等待下一条实时事件；转发任务结束（进程关闭/订阅释放）时返回 `None`。
    pub async fn recv(&mut self) -> Option<SessionEvent> {
        self.events.recv().await
    }
}

impl Drop for SessionSubscription {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

impl AgentControlClient {
    /// `session/subscribe`：建立 replay + 实时事件流。
    ///
    /// 先订阅通知广播再发请求，避免 replay 返回与首条 live 事件之间丢帧。
    pub async fn subscribe_session(
        &self,
        target: &AgentWorkspaceTarget,
        session_id: &str,
        delivery_kind: DeliveryKind,
        after_seq: Option<u64>,
        include_snapshot: bool,
    ) -> Result<SessionSubscription, ControlError> {
        let process = self.client_for(target, RuntimePolicy::StartIfNeeded).await?;
        let mut notifications = process.client().subscribe_notifications();
        let params = SessionSubscribeParams {
            session_id: session_id.to_string(),
            delivery_kind,
            after_seq,
            include_snapshot,
        };
        let params =
            serde_json::to_value(&params).expect("session/subscribe params 序列化不会失败");
        let response = process
            .client()
            .request(SESSION_SUBSCRIBE_METHOD, Some(params))
            .await?;
        let result: SessionSubscribeResult = serde_json::from_value(response).map_err(|error| {
            ControlError::InvalidResult(format!("invalid session/subscribe result: {error}"))
        })?;

        let mut tracker = SequenceTracker::default();
        let mut replay: Vec<SessionEvent> = Vec::with_capacity(result.events.len());
        for event in result.events {
            let event = tracker.normalize(event);
            if tracker.accept(&event.event_id) {
                replay.push(event);
            }
        }
        replay.sort_by(|left, right| {
            left.seq
                .cmp(&right.seq)
                .then(left.timestamp.cmp(&right.timestamp))
        });

        let (tx, rx) = mpsc::channel(LIVE_CHANNEL_CAPACITY);
        let session_id = result.session_id.clone();
        let task = tokio::spawn(async move {
            let mut tracker = tracker;
            loop {
                match notifications.recv().await {
                    Ok(value) => {
                        if value.get("method").and_then(serde_json::Value::as_str)
                            != Some(SESSION_EVENT_NOTIFICATION)
                        {
                            continue;
                        }
                        let Some(params) = value.get("params") else {
                            continue;
                        };
                        let Ok(event) =
                            serde_json::from_value::<SessionEvent>(params.clone())
                        else {
                            continue;
                        };
                        if event.session_id != session_id {
                            continue;
                        }
                        let event = tracker.normalize(event);
                        if !tracker.accept(&event.event_id) {
                            continue;
                        }
                        if tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    // 广播落后只意味着跳过了与订阅无关的通知；继续消费剩余流。
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(SessionSubscription {
            replay,
            snapshot: result.snapshot,
            events: rx,
            abort: task.abort_handle(),
        })
    }
}

/// 与 TS `SessionEventSequenceState` 同构：`seq == 0` 的旧事件补单调序号，
/// 并按有界窗口对 `eventId` 去重。
#[derive(Default)]
struct SequenceTracker {
    last_assigned: u64,
    assigned_by_id: HashMap<String, u64>,
    assigned_order: VecDeque<String>,
    delivered_ids: HashSet<String>,
    delivered_order: VecDeque<String>,
}

impl SequenceTracker {
    fn normalize(&mut self, mut event: SessionEvent) -> SessionEvent {
        if event.seq > 0 {
            self.last_assigned = self.last_assigned.max(event.seq);
            return event;
        }
        if let Some(seq) = self.assigned_by_id.get(&event.event_id) {
            event.seq = *seq;
            return event;
        }
        let next = self.last_assigned + 1;
        self.last_assigned = next;
        self.assigned_by_id.insert(event.event_id.clone(), next);
        self.assigned_order.push_back(event.event_id.clone());
        while self.assigned_order.len() > MAX_TRACKED_EVENT_IDS {
            if let Some(removed) = self.assigned_order.pop_front() {
                self.assigned_by_id.remove(&removed);
            }
        }
        event.seq = next;
        event
    }

    fn accept(&mut self, event_id: &str) -> bool {
        if !self.delivered_ids.insert(event_id.to_string()) {
            return false;
        }
        self.delivered_order.push_back(event_id.to_string());
        while self.delivered_order.len() > MAX_TRACKED_EVENT_IDS {
            if let Some(removed) = self.delivered_order.pop_front() {
                self.delivered_ids.remove(&removed);
            }
        }
        true
    }
}
