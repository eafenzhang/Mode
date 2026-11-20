//! Mode TUI（LT-8）第一段纵切：可渲染的会话列表 + 输入行骨架。
//!
//! 完整 33 场景与 Agent 接线还没迁；本 crate 先固定「视图模型 → 渲染」的边界，
//! 让后续 ratatui 场景可以独立测试，不把终端 IO 混进模型。

pub mod agent_client;
pub mod agent_events;
pub mod agent_wire;
pub mod model;
pub mod render;

pub use agent_client::AgentClient;
pub use agent_events::{apply_delta, apply_snapshot, session_from_summary};
pub use agent_wire::apply_agent_notification;
pub use model::{SessionPhase, TuiModel, TuiSession};
pub use render::render;
