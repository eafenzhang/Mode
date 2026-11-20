//! 子代理执行端口（C4 T5）。
//!
//! `Agent` 工具由 [`crate::session::Session`] 特判：生命周期 `session.updated`
//! 事件由会话发布，子 turn 的执行/持久化由宿主提供的 [`SubagentRunner`] 完成
//! （agent-server 用 provider + session-store 实现）。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §7。

use serde::{Deserialize, Serialize};

/// `Agent` 工具入参（协议里是模型生成 JSON；字段与观察的 inputSchema 对齐）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentRunRequest {
    /// 会话侧生成的 `agent_<uuid>`（lifecycle 事件与 ended 项共用）。
    pub agent_id: String,
    /// 会话侧生成的 `sess_subagent_agent_<uuid>`（C3 read 面依赖该前缀）。
    pub child_session_id: String,
    /// `description`（3-5 词）；用于 lifecycle 事件与 ended 项 title。
    pub description: String,
    /// 父 turn id（child turn_usage 的关联事实）。
    pub turn_id: String,
    /// 子代理任务 prompt（自包含）。
    pub prompt: String,
    /// `general-purpose` / `Explore`（宿主按类型决定工具子集）。
    pub subagent_type: String,
    /// 当前实现仍是同步执行；`true` 时由宿主决定（登记差异）。
    pub run_in_background: bool,
}

/// 子 turn 的记录事实（宿主写 DB 与 parent 工具结果）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentRunOutcome {
    /// `agent_<uuid>`。
    pub agent_id: String,
    /// `sess_subagent_agent_<uuid>`（C3 read 面依赖该前缀）。
    pub child_session_id: String,
    /// 子代理最终文本。
    pub text: String,
    /// 子 turn 的 token 用量（`subagent_tokens`）。
    pub tokens: u64,
    /// 子 turn 的工具调用数（`tool_uses`）。
    pub tool_uses: u64,
    /// 端到端耗时（`duration_ms`）。
    pub duration_ms: u64,
}

/// 宿主实现：启动并等待一个子代理 turn。
pub trait SubagentRunner: Send + Sync {
    fn run(&self, request: &SubagentRunRequest) -> Result<SubagentRunOutcome, String>;
}
