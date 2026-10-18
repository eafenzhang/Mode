//! 模型 provider 的抽象与脚本化 fake（M2 Task 5A 的地基）。
//!
//! 规格：`docs/plans/2026-10-18-m2-session-stream.md` 的 DR-T5-2 与
//! `docs/specs/tauri-agent-runtime.md` §3.5 的事件形状。
//!
//! **边界**：本模块不接真实供应商（HTTP/SSE 适配另立任务）。fake 的剧本是**请求内容的纯函数**
//! （同 TS mock provider 的确定性口径）：下标 = 请求里 `Assistant` 消息的条数。
//!
//! 产出的 delta 序列与 spec §3.5 的 `model.streaming` 顺序一致：
//! `Text` 或 `ToolInputStart → ToolInputDelta → ToolInputEnd → ToolCall`。

use serde_json::Value;

/// 一条工具调用（模型侧产出）。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelToolCall {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: Value,
}

/// 模型看到的一条消息。只放本轮事件契约需要的形状，不搬完整会话存储。
#[derive(Debug, Clone, PartialEq)]
pub enum ModelMessage {
    User(String),
    Assistant {
        text: Option<String>,
        tool_calls: Vec<ModelToolCall>,
    },
    ToolResult {
        tool_call_id: String,
        content: String,
    },
}

/// 一次模型请求。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    pub messages: Vec<ModelMessage>,
    /// 可用的工具名（schema 由会话层在 Task 2 接）。
    pub tools: Vec<String>,
}

/// 模型流的一步。
#[derive(Debug, Clone, PartialEq)]
pub enum ModelDelta {
    Text {
        text: String,
    },
    ToolInputStart {
        tool_call_id: String,
        tool_name: String,
    },
    ToolInputDelta {
        tool_call_id: String,
        delta: String,
    },
    ToolInputEnd {
        tool_call_id: String,
    },
    ToolCall {
        tool_call_id: String,
        tool_name: String,
        input: Value,
    },
}

/// 模型统计（`turn.completed.usage` 的来源；字段名与线上一致，映射在会话层）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
}

/// 一次模型应答：按顺序的 delta + 统计 + 停止原因（`stop` / `tool-calls`）。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelResponse {
    pub deltas: Vec<ModelDelta>,
    pub usage: ModelUsage,
    pub stop_reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub message: String,
}

pub trait ModelProvider {
    fn stream(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError>;
}

/// 剧本的一步。
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptStep {
    Text(String),
    ToolCalls(Vec<ScriptToolCall>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptToolCall {
    pub name: String,
    pub input: Value,
}

/// 脚本化 fake：下标 = 请求里 `Assistant` 消息的条数；越界时重复最后一步。
pub struct ScriptedProvider {
    steps: Vec<ScriptStep>,
}

impl ScriptedProvider {
    pub fn new(steps: Vec<ScriptStep>) -> Self {
        Self { steps }
    }
}

impl ModelProvider for ScriptedProvider {
    fn stream(&self, request: &ModelRequest) -> Result<ModelResponse, ProviderError> {
        let index = assistant_count(&request.messages);
        let step = self
            .steps
            .get(index)
            .or_else(|| self.steps.last())
            .ok_or_else(|| ProviderError {
                message: "scripted provider: 剧本为空".to_string(),
            })?;
        match step {
            ScriptStep::Text(text) => Ok(ModelResponse {
                deltas: vec![ModelDelta::Text { text: text.clone() }],
                usage: ModelUsage {
                    input_tokens: 10,
                    output_tokens: 12,
                    ..ModelUsage::default()
                },
                stop_reason: "stop",
            }),
            ScriptStep::ToolCalls(calls) => {
                let mut deltas = Vec::with_capacity(calls.len() * 4);
                for (position, call) in calls.iter().enumerate() {
                    // 与 TS mock provider 相同的确定性 id 形状。
                    let tool_call_id = format!("toolu_mock_{index}_{position}");
                    let input_json = serde_json::to_string(&call.input).expect("入参可序列化");
                    deltas.push(ModelDelta::ToolInputStart {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: call.name.clone(),
                    });
                    deltas.push(ModelDelta::ToolInputDelta {
                        tool_call_id: tool_call_id.clone(),
                        delta: input_json,
                    });
                    deltas.push(ModelDelta::ToolInputEnd {
                        tool_call_id: tool_call_id.clone(),
                    });
                    deltas.push(ModelDelta::ToolCall {
                        tool_call_id,
                        tool_name: call.name.clone(),
                        input: call.input.clone(),
                    });
                }
                Ok(ModelResponse {
                    deltas,
                    usage: ModelUsage {
                        input_tokens: 10,
                        output_tokens: 20,
                        ..ModelUsage::default()
                    },
                    stop_reason: "tool-calls",
                })
            }
        }
    }
}

fn assistant_count(messages: &[ModelMessage]) -> usize {
    messages
        .iter()
        .filter(|message| matches!(message, ModelMessage::Assistant { .. }))
        .count()
}
