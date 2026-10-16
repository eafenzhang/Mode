//! `agent-core`：M2 会话运行时的 Rust 侧骨架。
//!
//! 规格：`docs/specs/tauri-agent-runtime.md`（oracle 契约、三层对拍口径、合法差异白名单）。
//! 纪律：`docs/specs/clean-room-policy.md`——本 crate 全部新写，只按 spec 与 oracle 观察取材，
//! 不读旧运行时的实现体。
//!
//! **Task 0 只落占位**，理由写在 spec 的验收口径里：这一阶段的意义是「以旧运行时的实际行为为
//! 规格」，凭印象发明事件字段会让后面的对拍从「比行为」退化成「比谁更早猜」。
//! 演进顺序：
//!
//! 1. Task 1 落 `crates/agent-protocol` 的 v4 镜像类型后，`SessionEvent` 换成协议的判别联合；
//! 2. Task 2 接上工具注册表与最小会话循环；
//! 3. Task 5 把 `Runtime::run` 换成流式（`impl Stream<Item = SessionEvent>`）并补双链路重放。

/// 一次 turn 产出的事件种类（占位）。
///
/// 取值口径来自 oracle 观察（spec §1.2 的实测事实）：会话事件的 `type` 就是线上取值，
/// `turn.started` / `turn.completed` / `turn.failed` 是一轮对话的三个端到端锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEventKind {
    TurnStarted,
    TurnCompleted,
    TurnFailed,
    /// 尚未建模的事件种类；Task 1 逐步补齐后应当清零。
    Other(String),
}

/// 一条会话事件（占位）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEvent {
    pub kind: SessionEventKind,
}

/// 会话运行时：把用户输入跑成**按顺序**的事件序列。
///
/// Task 5 会把签名改成流式（`-> impl Stream<Item = SessionEvent>`，见规格 `Runtime::run`）；
/// Task 0 只钉住最小契约：输入是 prompt，输出是按发生顺序排列的事件序列。
pub trait Runtime {
    fn run(&mut self, prompt: &str) -> Vec<SessionEvent>;
}

#[cfg(test)]
mod tests {
    use super::{Runtime, SessionEvent, SessionEventKind};

    /// 占位实现：证明 trait 可用（真正的实现从 Task 2 开始接）。
    struct StubRuntime;

    impl Runtime for StubRuntime {
        fn run(&mut self, prompt: &str) -> Vec<SessionEvent> {
            vec![
                SessionEvent {
                    kind: SessionEventKind::TurnStarted,
                },
                SessionEvent {
                    kind: SessionEventKind::Other(format!("echo:{prompt}")),
                },
            ]
        }
    }

    #[test]
    fn runtime_trait_yields_ordered_events() {
        let mut runtime = StubRuntime;
        let events = runtime.run("hi");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, SessionEventKind::TurnStarted);
        assert_eq!(
            events[1].kind,
            SessionEventKind::Other("echo:hi".to_string())
        );
    }
}
