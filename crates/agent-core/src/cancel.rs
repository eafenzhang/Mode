//! 最小取消令牌（LT-1 C1 · 控制器裁决 4）。
//!
//! C1 的 stop 语义要求：运行中的 turn 可被中断，取消发生在**边界**上——
//! - 模型流中：到达下一条 delta 边界即停止消费（已到达的 delta 不回滚）；
//! - 模型步之间 / 工具批次之后：下一步模型请求开始前直接退出。
//!
//! 单个工具是同步调用，批次一旦开始就跑完（`tool.updated/batch` 的帧形状不因取消缺帧）；
//! 取消粒度见 [`crate::session::Session::run_turn`] 的边界检查。
//!
//! 实现用 `watch` 通道而不是 `Notify`：`borrow_and_update` + `changed` 天然无竞态
//! （先读当前值再等变化，「检查时刚好取消」不会漏唤醒），也不需要自己维护标志位。

use tokio::sync::watch;

/// 一次 turn 的取消开关。克隆共享同一个状态；`cancel` 幂等。
#[derive(Debug, Clone)]
pub struct CancelToken {
    sender: watch::Sender<bool>,
    // 初始 receiver 必须活着：`watch::Sender::send` 在**所有** receiver 都被丢弃后
    // 返回 Err，标志位就再也置不上。`cancelled()` 各自 `subscribe()` 出新的 receiver。
    _receiver: watch::Receiver<bool>,
}

impl CancelToken {
    pub fn new() -> Self {
        let (sender, receiver) = watch::channel(false);
        Self {
            sender,
            _receiver: receiver,
        }
    }

    /// 置位取消。之后的 [`Self::cancelled`] 立即就绪。
    pub fn cancel(&self) {
        let _ = self.sender.send(true);
    }

    /// 是否已取消（同步检查；`run_turn` 在步边界用它短路）。
    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    /// 等待取消置位。已取消时立即返回；全部 `CancelToken` 克隆被丢弃后也返回
    /// （此时不可能再有人取消，继续等没有意义）。
    pub async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}
