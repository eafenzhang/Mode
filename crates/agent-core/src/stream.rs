//! 会话事件的投递抽象与 turn 级错误（M2 Task 5A）。
//!
//! `desktop-continuous` 的实时语义先落地：事件产生即交给 sink，**不保留历史**。
//! `web-remote-replayable` 的 snapshot/queue 归 Host/relay（`docs/plans/2026-10-18-m2-session-stream.md`
//! 的 DR-T5-1），本模块不复制。

use agent_protocol::session_event::SessionEvent;

use crate::provider::ModelUsage;

/// 失败时刻**已知**的 turn 内累计事实（LT-1 C1 Task 7 控制器裁决 6）。
///
/// provider 在 turn 中途失败时，之前成功请求的用量与请求/工具计数仍然已知；
/// `turn_usage` 行按这里的值记录，而不是一律清零。取消路径的终态由 `run_turn`
/// 自带 usage，不走这里。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnProgress {
    pub usage: ModelUsage,
    pub model_requests: u64,
    pub tool_calls: u64,
}

/// 会话事件的消费端。
///
/// `emit` 是同步调用：turn 循环在 provider delta **到达时**就调用它，而不是等模型流
/// 结束。sink 的实现若需要异步投递，自己负责缓冲；这里不引入二次状态。
///
/// `Send` 上界（LT-1 C1 控制器裁决 1）：宿主层把 `run_turn` 的 future 放进 tokio
/// worker，future 捕获 `&mut dyn EventSink`，因此 trait 对象本身必须 `Send`。
pub trait EventSink: Send {
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
    /// 取消导致的退出（LT-1 C1 控制器裁决 4）。此时 `run_turn` 已经发出**唯一**的
    /// 终态事件（`turn.completed` + `resultType:"cancelled"`，协议注释把用户中断归为
    /// 正常结束），调用方不得再补发第二个终态。provider 错误仍为 `false`——那种
    /// 情况下没有终态，终态由调用方发。
    pub cancelled: bool,
    /// 失败时刻已知的累计事实（Task 7 裁决 6：失败 turn 的 `turn_usage` 记已知值）。
    pub progress: TurnProgress,
}

impl TurnError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
            progress: TurnProgress::default(),
        }
    }

    /// provider 错误 + 已知累计（`run_turn` 在请求失败处构造）。
    pub fn with_progress(message: impl Into<String>, progress: TurnProgress) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
            progress,
        }
    }

    /// 取消退出（终态已由 `run_turn` 发出）。
    pub fn cancelled() -> Self {
        Self {
            message: "turn cancelled".to_string(),
            cancelled: true,
            progress: TurnProgress::default(),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}
