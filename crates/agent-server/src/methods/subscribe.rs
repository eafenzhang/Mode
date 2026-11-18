//! `session/subscribe`：replay + live 的事件订阅。
//!
//! 应答 `{sessionId, eventSeq, events[], snapshot?}`：replay 来自 runtime 的内存
//! 事件日志（`afterSeq` 过滤）；live 段是 broadcast receiver——runtime 的
//! `subscribe()` 在同一把锁内「读日志 + 建 receiver」，两段拼接无重复、无缺口
//! （Task 6 的运行时用例已锁）。**live 转发不能在 response 之前发出**：handler
//! 把转发任务挂到 [`crate::dispatch::AgentServer`] 的 pending 队列，stdio 循环在
//! 写完本 response 后再 spawn（`crate::server::run_stdio`）。
//!
//! `includeSnapshot` 时返回会话快照，且 `runtime.deliveryKind` 记**订阅的**
//! deliveryKind（Task 0 观察：subscribe 快照的 runtime 多 `deliveryKind:
//! "desktop-continuous"`，create 快照没有）。

use agent_protocol::session_read::{SessionSubscribeParams, SessionSubscribeResult};
use serde_json::Value;

use crate::dispatch::PendingLiveForward;
use crate::server::ServerError;

use super::parse_params;

pub(crate) fn handle(
    server: &crate::dispatch::AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: SessionSubscribeParams =
        parse_params(crate::methods::SESSION_SUBSCRIBE_METHOD, params)?;
    let runtime = server.runtime(&params.session_id)?;

    // 旧 runtime 的游标（afterSeq > 当前 eventSeq）按重置处理：replay 与 live 都不
    // 过滤，否则 resume 后的新事件会被静默丢掉（最终评审 I10）。
    let (replay, receiver, effective_after_seq) = runtime.subscribe_with_cursor(params.after_seq);
    // 高水位在订阅**之后**读：只会高估不会低估（eventSeq 以下的 seq 都已投递过；
    // 事件去重按 eventId，多余的高估无害；低估会让宿主漏看事件）。
    let event_seq = runtime.event_seq();
    let snapshot = if params.include_snapshot {
        // 订阅快照的 runtime.deliveryKind 记**订阅的**链路（Task 0 观察）。
        let mut snapshot = runtime.snapshot();
        snapshot.runtime.delivery_kind = Some(params.delivery_kind);
        Some(snapshot)
    } else {
        None
    };

    server.enqueue_live_forward(PendingLiveForward {
        after_seq: effective_after_seq,
        receiver,
    });

    let result = SessionSubscribeResult {
        session_id: params.session_id,
        event_seq,
        events: replay,
        snapshot,
    };
    serde_json::to_value(result).map_err(|error| ServerError::internal(error.to_string()))
}
