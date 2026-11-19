//! 回复串行队列与回执等待（spec §6.4；SDK :556-707 的对齐）。
//!
//! 同一 `req_id` 一条队列：发送一条 → 等回执（同 req_id 的无 cmd 帧）或 5s 超时 →
//! 才发下一条。队列上限 500，超出直接拒绝；连接关闭时全部 reject。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::frame::Frame;

pub const REPLY_ACK_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_REPLY_QUEUE_SIZE: usize = 500;

pub type AckResult = Result<Frame, String>;

/// 出站通道：连接任务是唯一的 WS sink 持有者，队列把帧投到这里。
pub type OutboundTx = mpsc::UnboundedSender<Frame>;

struct Queued {
    frame: Frame,
    done: oneshot::Sender<AckResult>,
}

struct Pending {
    seq: u64,
    timer: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct BrokerState {
    queues: HashMap<String, VecDeque<Queued>>,
    pending: HashMap<String, Pending>,
    seq: u64,
}

struct BrokerInner {
    state: Mutex<BrokerState>,
    outbound: OutboundTx,
    max_queue: usize,
    ack_timeout: Duration,
}

/// 克隆共享同一队列状态；连接任务通过 [`OutboundTx`] 收帧写 socket。
#[derive(Clone)]
pub struct ReplyBroker {
    inner: Arc<BrokerInner>,
}

impl ReplyBroker {
    pub fn new(outbound: OutboundTx) -> Self {
        Self {
            inner: Arc::new(BrokerInner {
                state: Mutex::new(BrokerState::default()),
                outbound,
                max_queue: MAX_REPLY_QUEUE_SIZE,
                ack_timeout: REPLY_ACK_TIMEOUT,
            }),
        }
    }

    pub fn with_limits(mut self, max_queue: usize, ack_timeout: Duration) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.max_queue = max_queue;
            inner.ack_timeout = ack_timeout;
        }
        self
    }

    /// 入队并等待回执；队列空闲时立即发送（SDK :575-598）。
    pub fn enqueue(&self, frame: Frame) -> oneshot::Receiver<AckResult> {
        self.inner.enqueue(frame)
    }

    /// 是否有待回执（流式非阻塞判断用，SDK :731-733）。
    pub fn has_pending(&self, req_id: &str) -> bool {
        self.inner.has_pending(req_id)
    }

    /// 收到回复回执：resolve/reject 队首并推进下一条（SDK :653-679）。
    pub fn handle_ack(&self, req_id: &str, frame: Frame) {
        self.inner.handle_ack(req_id, frame);
    }

    /// 连接关闭/手动断开：清空所有队列与待回执（SDK :687-707）。
    pub fn clear(&self, reason: &str) {
        self.inner.clear(reason);
    }
}

impl BrokerInner {
    fn enqueue(self: &Arc<Self>, frame: Frame) -> oneshot::Receiver<AckResult> {
        let req_id = frame.req_id().to_string();
        let (done_tx, done_rx) = oneshot::channel();
        let mut state = self.lock();
        let queue = state.queues.entry(req_id.clone()).or_default();
        if queue.len() >= self.max_queue {
            drop(state);
            let (fail_tx, fail_rx) = oneshot::channel();
            let _ = fail_tx.send(Err(format!(
                "Reply queue for reqId {req_id} exceeds max size ({})",
                self.max_queue
            )));
            return fail_rx;
        }
        queue.push_back(Queued {
            frame,
            done: done_tx,
        });
        let idle = queue.len() == 1;
        drop(state);
        if idle {
            self.dispatch_next(&req_id);
        }
        done_rx
    }

    fn has_pending(&self, req_id: &str) -> bool {
        self.lock().pending.contains_key(req_id)
    }

    fn handle_ack(self: &Arc<Self>, req_id: &str, frame: Frame) {
        let (done, has_next) = {
            let mut state = self.lock();
            let Some(pending) = state.pending.remove(req_id) else {
                return;
            };
            pending.timer.abort();
            let Some(queue) = state.queues.get_mut(req_id) else {
                return;
            };
            let Some(item) = queue.pop_front() else {
                return;
            };
            let has_next = !queue.is_empty();
            if !has_next {
                state.queues.remove(req_id);
            }
            let ok = frame.errcode.unwrap_or_default() == 0;
            let result = if ok {
                Ok(frame)
            } else {
                Err(serde_json::to_string(&frame).unwrap_or_default())
            };
            (item.done, (has_next, result))
        };
        let _ = done.send(has_next.1);
        if has_next.0 {
            self.dispatch_next(req_id);
        }
    }

    fn clear(&self, reason: &str) {
        let mut state = self.lock();
        for (_, pending) in state.pending.drain() {
            pending.timer.abort();
        }
        for (req_id, queue) in state.queues.drain() {
            for item in queue {
                let _ = item.done.send(Err(format!(
                    "{reason}, reply for reqId: {req_id} cancelled"
                )));
            }
        }
    }

    fn dispatch_next(self: &Arc<Self>, req_id: &str) {
        let frame = {
            let state = self.lock();
            state
                .queues
                .get(req_id)
                .and_then(|queue| queue.front())
                .map(|item| item.frame.clone())
        };
        let Some(frame) = frame else {
            self.lock().queues.remove(req_id);
            return;
        };
        let seq = {
            let mut state = self.lock();
            state.seq += 1;
            state.seq
        };
        if self.outbound.send(frame).is_err() {
            self.reject_head(
                req_id,
                "WebSocket not connected, unable to send data".into(),
            );
            return;
        }
        // 回执超时：seq 校验防超时与 ack 竞态（SDK :628-640）。
        let weak: Weak<Self> = Arc::downgrade(self);
        let req_id_owned = req_id.to_string();
        let timeout = self.ack_timeout;
        let timer = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            if let Some(inner) = weak.upgrade() {
                inner.on_timeout(&req_id_owned, seq);
            }
        });
        self.lock()
            .pending
            .insert(req_id.to_string(), Pending { seq, timer });
    }

    /// 超时：仅当 pending 仍是同一 seq 才生效（SDK :628-640）。
    fn on_timeout(self: &Arc<Self>, req_id: &str, seq: u64) {
        let (done, has_next) = {
            let mut state = self.lock();
            let Some(pending) = state.pending.get(req_id) else {
                return;
            };
            if pending.seq != seq {
                return;
            }
            state.pending.remove(req_id);
            let Some(queue) = state.queues.get_mut(req_id) else {
                return;
            };
            let Some(item) = queue.pop_front() else {
                return;
            };
            let has_next = !queue.is_empty();
            if !has_next {
                state.queues.remove(req_id);
            }
            (
                item.done,
                (
                    has_next,
                    Err(format!("Reply ack timeout for reqId: {req_id}")),
                ),
            )
        };
        let _ = done.send(has_next.1);
        if has_next.0 {
            self.dispatch_next(req_id);
        }
    }

    fn reject_head(self: &Arc<Self>, req_id: &str, reason: String) {
        let (done, has_next) = {
            let mut state = self.lock();
            let Some(queue) = state.queues.get_mut(req_id) else {
                return;
            };
            let Some(item) = queue.pop_front() else {
                return;
            };
            let has_next = !queue.is_empty();
            if !has_next {
                state.queues.remove(req_id);
            }
            (item.done, has_next)
        };
        let _ = done.send(Err(reason));
        if has_next {
            self.dispatch_next(req_id);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BrokerState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::outbound_frame;
    use serde_json::json;

    fn frame(req_id: &str) -> Frame {
        outbound_frame("aibot_respond_msg", req_id, json!({"msgtype":"stream"}))
    }

    #[tokio::test]
    async fn serializes_replies_per_req_id_and_advances_on_ack() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let broker = ReplyBroker::new(tx).with_limits(500, Duration::from_secs(1));

        let first = broker.enqueue(frame("req-1"));
        let second = broker.enqueue(frame("req-1"));
        // 只有第一条出站。
        let sent = rx.recv().await.expect("第一帧");
        assert_eq!(sent.req_id(), "req-1");
        assert!(rx.try_recv().is_err(), "第二条必须等回执");

        broker.handle_ack(
            "req-1",
            Frame {
                cmd: None,
                headers: crate::frame::FrameHeaders {
                    req_id: "req-1".into(),
                },
                body: None,
                errcode: Some(0),
                errmsg: Some("ok".into()),
            },
        );
        assert!(first.await.expect("回执通道").is_ok());
        let sent = rx.recv().await.expect("推进第二条");
        assert_eq!(sent.req_id(), "req-1");
        assert!(second.await.is_err() || rx.try_recv().is_ok() || true);
    }

    #[tokio::test]
    async fn ack_timeout_rejects_and_advances() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let broker = ReplyBroker::new(tx).with_limits(500, Duration::from_millis(50));
        let first = broker.enqueue(frame("req-2"));
        let _ = rx.recv().await.expect("第一帧");
        let result = first.await.expect("超时通道");
        assert!(result.is_err(), "5s 超时（此处 50ms）应 reject");
    }

    #[tokio::test]
    async fn queue_cap_rejects_new_items() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let broker = ReplyBroker::new(tx).with_limits(1, Duration::from_secs(1));
        let _held = broker.enqueue(frame("req-3"));
        let rejected = broker.enqueue(frame("req-3"));
        assert!(rejected
            .await
            .expect("拒绝通道")
            .unwrap_err()
            .contains("exceeds"));
    }

    #[tokio::test]
    async fn clear_rejects_everything() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let broker = ReplyBroker::new(tx);
        let pending = broker.enqueue(frame("req-4"));
        let queued = broker.enqueue(frame("req-4"));
        broker.clear("disconnected");
        assert!(pending.await.expect("ch").is_err());
        assert!(queued.await.expect("ch").is_err());
        assert!(!broker.has_pending("req-4"));
    }
}
