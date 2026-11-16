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

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

/// provider 的异步应答 future。
///
/// `'static` 是刻意的：provider 不能借用调用方的请求或会话状态，流式适配器必须把
/// 自己需要的句柄（client、累积缓冲等）移进 future（Task 4 的 HTTP/SSE 实现依赖这一点）。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 模型 delta 的增量出口。
///
/// provider 每产出一条 delta 就 `send` 一次；会话循环在 delta 到达时立刻投影成
/// `model.streaming`，不再等整个应答收齐。unbounded 是因为 delta 是背压很小的元数据，
/// 会话侧按顺序消费；真正的网络背压在 provider 读流一侧（Task 4）。
pub type DeltaSink = tokio::sync::mpsc::UnboundedSender<ModelDelta>;

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
    /// 系统提示（上下文装配产出的唯一一条，见 [`crate::context`]）。
    System(String),
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

/// provider 中立的工具声明：名字、可选描述与入参 JSON Schema。
///
/// `description` 只有调用方确知时才填；本仓库不为了填满字段而复制上游提示词。
/// 适配器（如 OpenAI Chat Completions）负责把它映射成供应商自己的 `function` 形状。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

/// 模型能力（token）：上下文窗口与单次最大输出。
///
/// `None` = 模型目录没有给出该值；预算/压缩在 C2 按上限使用保守默认，这里不编数字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelCapabilities {
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

/// 一次模型请求。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    pub messages: Vec<ModelMessage>,
    /// 可用的工具声明（名字 + 可选描述 + 入参 schema）。
    pub tools: Vec<ModelToolDefinition>,
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

/// 一次模型应答的终态：统计 + 停止原因（`stop` / `tool-calls`）。
///
/// delta 不在这里重复承载——provider 已把它们逐条送进 [`DeltaSink`]。应答只描述
/// 「流结束了」时的事实，避免同一批 delta 有两条投递路径（重复状态）。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelResponse {
    pub usage: ModelUsage,
    pub stop_reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub message: String,
}

pub trait ModelProvider: Send + Sync {
    /// 发起一次流式请求：每个 delta 通过 `deltas` 增量送达，future 完成时给出终态。
    ///
    /// 实现方必须在 future 返回前把所有 delta `send` 出去；会话循环会在 future 结束后
    /// 再排空一次队列，因此「最后一条 delta 与返回同时发生」也不会丢尾。
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>>;
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
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        // future 必须 `'static`：把剧本复制进去，不借用 `&self`。
        let steps = self.steps.clone();
        Box::pin(async move {
            let index = assistant_count(&request.messages);
            let step = steps
                .get(index)
                .or_else(|| steps.last())
                .ok_or_else(|| ProviderError {
                    message: "scripted provider: 剧本为空".to_string(),
                })?;
            match step {
                ScriptStep::Text(text) => {
                    send_delta(&deltas, ModelDelta::Text { text: text.clone() })?;
                    Ok(ModelResponse {
                        usage: ModelUsage {
                            input_tokens: 10,
                            output_tokens: 12,
                            ..ModelUsage::default()
                        },
                        stop_reason: "stop",
                    })
                }
                ScriptStep::ToolCalls(calls) => {
                    for (position, call) in calls.iter().enumerate() {
                        // 与 TS mock provider 相同的确定性 id 形状。
                        let tool_call_id = format!("toolu_mock_{index}_{position}");
                        let input_json = serde_json::to_string(&call.input).expect("入参可序列化");
                        send_delta(
                            &deltas,
                            ModelDelta::ToolInputStart {
                                tool_call_id: tool_call_id.clone(),
                                tool_name: call.name.clone(),
                            },
                        )?;
                        send_delta(
                            &deltas,
                            ModelDelta::ToolInputDelta {
                                tool_call_id: tool_call_id.clone(),
                                delta: input_json,
                            },
                        )?;
                        send_delta(
                            &deltas,
                            ModelDelta::ToolInputEnd {
                                tool_call_id: tool_call_id.clone(),
                            },
                        )?;
                        send_delta(
                            &deltas,
                            ModelDelta::ToolCall {
                                tool_call_id,
                                tool_name: call.name.clone(),
                                input: call.input.clone(),
                            },
                        )?;
                    }
                    Ok(ModelResponse {
                        usage: ModelUsage {
                            input_tokens: 10,
                            output_tokens: 20,
                            ..ModelUsage::default()
                        },
                        stop_reason: "tool-calls",
                    })
                }
            }
        })
    }
}

/// 把一条 delta 送进 sink；消费端已关闭时显式报错，而不是静默吞掉整条剧情的输出。
fn send_delta(deltas: &DeltaSink, delta: ModelDelta) -> Result<(), ProviderError> {
    deltas.send(delta).map_err(|_| ProviderError {
        message: "scripted provider: delta sink 已关闭".to_string(),
    })
}

fn assistant_count(messages: &[ModelMessage]) -> usize {
    messages
        .iter()
        .filter(|message| matches!(message, ModelMessage::Assistant { .. }))
        .count()
}
