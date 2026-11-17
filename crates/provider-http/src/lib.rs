//! `provider-http`：模型供应商的 HTTP/SSE 适配（LT-1 C1 · Task 4，C2 · Task 5）。
//!
//! 消费 `agent-core::provider` 的中立类型（[`agent_core::provider::ModelProvider`]、
//! `ModelRequest` / `ModelDelta` / `ModelResponse`），产出 OpenAI Chat Completions、
//! Anthropic Messages 与 OpenAI Responses 三条线协议的流式实现。provider 配置与凭据
//! 解析归 `agent-server::provider_config`，本 crate 不碰（spec `agent-core-parity.md`
//! §2.2 的职责边界）。
//!
//! 模块：
//! - [`sse`]：`text/event-stream` 纯解码器（不依赖网络，任何字节切分都安全）；
//! - [`openai_chat`]：Chat Completions 请求构造与流式适配（`ModelProvider` 实现）；
//! - [`anthropic_messages`]：Anthropic Messages 请求构造与流式适配（C2 · Task 4）；
//! - [`openai_responses`]：OpenAI Responses 请求构造与流式适配（C2 · Task 5）；
//! - [`error`]：统一错误构造（HTTP 状态、传输失败、响应契约违规、本地错误）。

pub mod anthropic_messages;
pub mod error;
pub mod openai_chat;
pub mod openai_responses;
pub mod sse;

pub use anthropic_messages::{
    build_messages_request, AnthropicMessagesConfig, AnthropicMessagesProvider,
    DEFAULT_ANTHROPIC_VERSION, DEFAULT_MAX_OUTPUT_TOKENS,
};
pub use openai_chat::{build_chat_request, OpenAiChatConfig, OpenAiChatProvider};
pub use openai_responses::{
    build_responses_request, OpenAiResponsesConfig, OpenAiResponsesProvider, INSTRUCTIONS_SEPARATOR,
};
pub use sse::SseDecoder;
