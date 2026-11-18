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

/// provider 错误的分类（C3 T2：重试决策只看分类，不解析错误文案）。
///
/// 分类来自观察到的两类路径（`docs/specs/agent-c3-contract.md` §2）：
/// - HTTP 状态错误（5xx retryable、401/403 非 retryable）；
/// - 传输/流中断（retryable；是否进入 stream recovery 由会话层按
///   「本尝试是否已提交 delta」决定，而不是只看这个分类）。
///
/// 未观察到的状态码（403/400/429 等）按最小规则分类，登记在契约 §2.7。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    /// 没被归类过的错误（脚本化 fake / 宿主自定义）。
    Unknown,
    /// 非 2xx HTTP 应答里没有更具体分类的状态（4xx 等）。
    HttpStatus,
    /// 5xx。
    Server,
    /// 401 / 403。
    Auth,
    /// 429。
    RateLimited,
    /// 传输失败（连接、TLS、DNS、请求未发出）。
    Transport,
    /// 流已开始但中途中断（读流失败、缺少终止事件）。
    StreamAbort,
    /// 响应/流不符合契约（非法 JSON、缺关键字段）。
    Malformed,
    /// 本地错误（delta sink 已关闭、请求体序列化失败）。
    Local,
    /// 流内 error 事件（供应商在 200 流里报错）。
    Api,
}

impl ProviderErrorKind {
    /// 线上 `turn.failed.attribution.reason` 的近似取值；未观察到的分类
    /// 用稳定的小写串（登记在契约 §2.7）。
    pub fn reason(self) -> &'static str {
        match self {
            Self::Unknown => "provider_error",
            Self::HttpStatus => "http_error",
            Self::Server => "server_error",
            Self::Auth => "auth_failed",
            Self::RateLimited => "rate_limited",
            Self::Transport | Self::StreamAbort => "network_error",
            Self::Malformed => "invalid_response",
            Self::Local => "local_error",
            Self::Api => "api_error",
        }
    }
}

/// 一次模型请求失败。
///
/// `message` 保持既有的可诊断文案（宿主直接进 `turn.failed`）；`kind`/`status`/
/// `retryable` 是重试决策字段。构造器是唯一推荐入口：字段默认值保证旧的
/// `message`-only 调用方语义不变（`Unknown` + 非 retryable）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub message: String,
    pub kind: ProviderErrorKind,
    pub status: Option<u16>,
    pub retryable: bool,
    /// 供应商正文/根因（不含适配器前缀）；`turn.failed.underlyingErrorMessage` 的来源。
    /// 未分类/本地错误为 `None`。
    pub underlying_message: Option<String>,
}

impl Default for ProviderError {
    fn default() -> Self {
        Self {
            message: String::new(),
            kind: ProviderErrorKind::Unknown,
            status: None,
            retryable: false,
            underlying_message: None,
        }
    }
}

impl ProviderError {
    /// 未分类错误（非 retryable，保持旧行为）。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Self::default()
        }
    }

    /// 非 2xx HTTP 应答。分类只依赖状态码：
    /// - 401/403 → auth（非 retryable）；
    /// - 408 → transport（retryable；请求超时，未观察到，登记）；
    /// - 429 → rate limited（retryable；未观察到，登记）；
    /// - 5xx → server（retryable）；
    /// - 其余 4xx/3xx → HTTP status（非 retryable）。
    pub fn http_status(status: u16, message: impl Into<String>) -> Self {
        let (kind, retryable) = match status {
            401 | 403 => (ProviderErrorKind::Auth, false),
            408 => (ProviderErrorKind::Transport, true),
            429 => (ProviderErrorKind::RateLimited, true),
            500..=599 => (ProviderErrorKind::Server, true),
            _ => (ProviderErrorKind::HttpStatus, false),
        };
        Self {
            message: message.into(),
            kind,
            status: Some(status),
            retryable,
            underlying_message: None,
        }
    }

    /// 认证/权限失败（401/403）。`message` 作为 underlying 保留（原始供应商文案）。
    pub fn auth(status: u16, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            underlying_message: Some(message.clone()),
            message,
            kind: ProviderErrorKind::Auth,
            status: Some(status),
            retryable: false,
        }
    }

    /// 传输失败（请求未发出/连接中断；retryable）。
    pub fn transport(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ProviderErrorKind::Transport,
            status: None,
            retryable: true,
            underlying_message: None,
        }
    }

    /// 流中断（已开始读流后的失败；retryable，会话层按已提交 delta 决定
    /// 走 stream recovery 还是 adapter 重试）。
    pub fn stream_abort(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ProviderErrorKind::StreamAbort,
            status: None,
            retryable: true,
            underlying_message: None,
        }
    }

    /// 响应/流不合契约（非 retryable）。
    pub fn malformed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ProviderErrorKind::Malformed,
            status: None,
            retryable: false,
            underlying_message: None,
        }
    }

    /// 本地错误（非 retryable）。
    pub fn local(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ProviderErrorKind::Local,
            status: None,
            retryable: false,
            underlying_message: None,
        }
    }

    /// 流内 error 事件（非 retryable；供应商在成功 HTTP 流里报错）。
    pub fn api(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ProviderErrorKind::Api,
            status: None,
            retryable: false,
            underlying_message: None,
        }
    }

    /// 保留既有文案，只替换分类字段（适配器按供应商错误类型细分时用）。
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// 记录供应商原始正文/根因（`turn.failed.underlyingErrorMessage`）。
    pub fn with_underlying(mut self, underlying: impl Into<String>) -> Self {
        self.underlying_message = Some(underlying.into());
        self
    }

    pub fn is_auth(&self) -> bool {
        self.kind == ProviderErrorKind::Auth
    }

    /// `turn.failed.attribution.reason` 的稳定取值。
    pub fn reason(&self) -> &'static str {
        self.kind.reason()
    }

    /// 线上事件/终态的规范文案（夹具 §2.3/§2.4）；原始细节留在 `message` /
    /// `underlying_message`，事件面不直接透传 `HTTP 503: …` 这类适配器前缀。
    pub fn canonical_message(&self) -> &str {
        match self.kind {
            ProviderErrorKind::Auth => "Provider authentication failed.",
            ProviderErrorKind::Server => "Provider returned a server error.",
            ProviderErrorKind::Transport | ProviderErrorKind::StreamAbort => {
                "Network connection failed for the provider request."
            }
            ProviderErrorKind::RateLimited => "Provider rate limited the request.",
            _ => &self.message,
        }
    }
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
                .ok_or_else(|| ProviderError::new("scripted provider: 剧本为空"))?;
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
    deltas
        .send(delta)
        .map_err(|_| ProviderError::local("scripted provider: delta sink 已关闭"))
}

fn assistant_count(messages: &[ModelMessage]) -> usize {
    messages
        .iter()
        .filter(|message| matches!(message, ModelMessage::Assistant { .. }))
        .count()
}
