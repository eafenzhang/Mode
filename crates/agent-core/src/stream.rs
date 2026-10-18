//! 会话事件的投递抽象与 turn 级错误（M2 Task 5A）。
//!
//! `desktop-continuous` 的实时语义先落地：事件产生即交给 sink，**不保留历史**。
//! `web-remote-replayable` 的 snapshot/queue 归 Host/relay（`docs/plans/2026-10-18-m2-session-stream.md`
//! 的 DR-T5-1），本模块不复制。

use agent_protocol::session_event::SessionEvent;

/// 会话事件的消费端。
pub trait EventSink {
    fn emit(&mut self, event: SessionEvent);
}

/// 测试与简单调用方可直接把事件收进 `Vec`。
impl EventSink for Vec<SessionEvent> {
    fn emit(&mut self, event: SessionEvent) {
        self.push(event);
    }
}

/// 一个 turn 的失败（provider 出错等）。
///
/// 线上 `turn.failed` 的完整载荷尚未观察，所以这里只把错误交回调用方，
/// **不编造** `turn_phase` / `error.code` 等字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnError {
    pub message: String,
}

impl TurnError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
