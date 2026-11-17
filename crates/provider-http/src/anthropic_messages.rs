//! Anthropic Messages 流式适配（LT-1 C2 · Task 4）。
//!
//! 契约来源：Anthropic 公开的 Messages API 文档（clean-room：不读任何上游实现）。
//! 覆盖 C2 需要的子集：
//! - 请求：`model` / `system`（顶层 text 块数组）/ `messages`（user / assistant /
//!   tool_result）/ `tools`（`name` + 可选 `description` + `input_schema`）/
//!   `max_tokens` / `stream: true`；
//! - 头：`content-type`、`x-api-key`、`anthropic-version`（公开文档当前版本
//!   [`DEFAULT_ANTHROPIC_VERSION`]）；
//! - 流：`message_start`（usage）→ `content_block_start`（text / thinking / tool_use）
//!   → `content_block_delta`（`text_delta` / `thinking_delta` / `signature_delta` /
//!   `input_json_delta`）→ `content_block_stop` → `message_delta`（stop_reason/usage）
//!   → `message_stop`；`ping` 忽略，`error` 显式报错；
//! - 工具调用：`tool_use` 块在 start 时产出 `ToolInputStart`，`input_json_delta`
//!   逐片产出 `ToolInputDelta`，`content_block_stop` 时解析拼接后的入参并产出
//!   `ToolInputEnd → ToolCall`（与 `ScriptedProvider` / OpenAI 适配同一组序）；
//! - stop_reason：`tool_use` → `"tool-calls"`；`end_turn` / `max_tokens` /
//!   `stop_sequence` 等 → `"stop"`（会话层只区分这两类，登记于 C2 §4.1.1）。
//!
//! 已登记的取舍（`docs/specs/agent-c2-context-memory.md` §4.1.1、
//! `docs/specs/tauri-agent-runtime.md` §C1.7）：
//! - `ModelDelta` 没有 reasoning 变体：thinking / redacted_thinking 块照常消费
//!   （含 `thinking_delta` / `signature_delta`），但**不产出** delta；thinking token
//!   已计入 Anthropic 的 `output_tokens`，`ModelUsage.reasoning_tokens` 保持 0。
//! - 请求侧 thinking 配置未建模（没有能力输入），不发 `thinking` 字段。
//! - Anthropic 要求 `max_tokens`：配置未给 `max_output_tokens` 时使用命名常量
//!   [`DEFAULT_MAX_OUTPUT_TOKENS`]（T0 观察到的 oracle 取值 8192），这是 wire 必填项
//!   的文档化回退，不声称模型能力。
//! - 流必须以 `message_stop` 收尾且所有 content block 已 `content_block_stop`；
//!   否则显式报错，不把截断流伪装成成功终态。
//!
//! 不做（C3 或后续检查点）：重试/退避、断流恢复、请求侧 thinking/cache 控制。

use std::collections::BTreeMap;
use std::fmt;

use agent_core::provider::{BoxFuture, DeltaSink};
use agent_core::{
    ModelDelta, ModelMessage, ModelProvider, ModelRequest, ModelResponse, ModelToolCall,
    ModelToolDefinition, ModelUsage, ProviderError,
};
use futures_util::StreamExt;
use serde_json::{Map, Value};

use crate::error::{self, truncate};
use crate::sse::SseDecoder;

/// Anthropic Messages 的默认 API 版本头（公开文档当前版本）。
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

/// `max_tokens` 的文档化回退值。
///
/// Anthropic 的 Messages API 要求该字段必填；模型目录没有给出 `max_output_tokens`
/// 时不能不发。取值来自 T0 对 oracle compact 请求的黑盒观察
/// （`agent-c2-contract.md` §1.4：`max_tokens:8192`），不是对模型能力的推断。
pub const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 8192;

/// Anthropic Messages 端点的连接配置。
///
/// `base_url` 是 API 根（如 `https://api.anthropic.com`，兼容网关同形），适配器在其后拼
/// `/v1/messages`，末尾多余的 `/` 会被去掉。`extra_headers` 对默认头
/// （`Content-Type` / `x-api-key` / `anthropic-version`）是**替换**语义：头名大小写
/// 不敏感地命中默认头时，默认值不发，同名行只保留 `extra_headers` 的那一条；因此网关
/// 可以自带鉴权头而不必担心默认 key 静默并存。`anthropic_version` 是版本头的显式配置；
/// `extra_headers` 里的 `anthropic-version` 优先级更高。`Debug` 不打印密钥：`api_key`
/// 恒为掩码，`extra_headers` 只显示头名（值可能是另一把凭据）。
#[derive(Clone)]
pub struct AnthropicMessagesConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_output_tokens: Option<u64>,
    pub extra_headers: Vec<(String, String)>,
    pub anthropic_version: Option<String>,
}

impl fmt::Debug for AnthropicMessagesConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicMessagesConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .field("model", &self.model)
            .field("max_output_tokens", &self.max_output_tokens)
            .field(
                "extra_headers",
                &self
                    .extra_headers
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("anthropic_version", &self.anthropic_version)
            .finish()
    }
}

/// `ModelProvider` 的 Anthropic Messages 实现：`POST {base_url}/v1/messages`
/// 并把 SSE 流转成 [`ModelDelta`] 序列 + [`ModelResponse`]。
pub struct AnthropicMessagesProvider {
    client: reqwest::Client,
    config: AnthropicMessagesConfig,
}

impl AnthropicMessagesProvider {
    /// 构造 provider。进程级 TLS provider 在这里装（与 `OpenAiChatProvider` 同一做法：
    /// 工作空间的 `reqwest` 以 `rustls-no-provider` 编译，重复安装安全）。
    ///
    /// 不设整体超时：流式应答可能持续数分钟；超时/重试语义归 C3（spec §5.6）。
    pub fn new(config: AnthropicMessagesConfig) -> Result<Self, reqwest::Error> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        Ok(Self {
            client: reqwest::Client::builder().build()?,
            config,
        })
    }
}

impl ModelProvider for AnthropicMessagesProvider {
    fn stream(
        &self,
        request: ModelRequest,
        deltas: DeltaSink,
    ) -> BoxFuture<'static, Result<ModelResponse, ProviderError>> {
        // future 必须 'static：client 内部是 Arc 可 Clone，配置克隆一份，不借用 &self。
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { stream_once(&client, &config, &request, &deltas).await })
    }
}

/// 把 provider 中立的请求映射成 Anthropic Messages 请求体（C2 §4.1）。
///
/// - 所有 `System` 消息保序映射为顶层 `system` 的 text 块数组（公开 API 接受字符串或
///   块数组；块数组保留多条系统段的边界），没有系统消息时省略键；
/// - `User` → `{role:"user", content:[{type:"text",text}]}`；
/// - `Assistant` → text 块（有文本时）+ 每个调用的 `tool_use` 块；
/// - `ToolResult` → `{role:"user", content:[{type:"tool_result",tool_use_id,content}]}`；
/// - 工具声明 → `{name, description?, input_schema}`（没有描述时省略键，不编造）；
/// - `max_tokens` 必填：配置缺失时用 [`DEFAULT_MAX_OUTPUT_TOKENS`]（见常量注释）；
/// - `stream: true`。
pub fn build_messages_request(request: &ModelRequest, config: &AnthropicMessagesConfig) -> Value {
    let mut body = Map::new();
    body.insert("model".to_string(), Value::String(config.model.clone()));

    let system: Vec<Value> = request
        .messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::System(text) => Some(serde_json::json!({
                "type": "text",
                "text": text,
            })),
            _ => None,
        })
        .collect();
    if !system.is_empty() {
        body.insert("system".to_string(), Value::Array(system));
    }

    let messages: Vec<Value> = request
        .messages
        .iter()
        .filter_map(anthropic_message)
        .collect();
    body.insert("messages".to_string(), Value::Array(messages));

    if !request.tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(request.tools.iter().map(anthropic_tool).collect()),
        );
    }
    body.insert(
        "max_tokens".to_string(),
        Value::from(
            config
                .max_output_tokens
                .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
        ),
    );
    body.insert("stream".to_string(), Value::Bool(true));
    Value::Object(body)
}

/// 一条非系统消息；`System` 已经在顶层 `system` 处理，返回 `None` 从 `messages` 排除。
fn anthropic_message(message: &ModelMessage) -> Option<Value> {
    match message {
        ModelMessage::System(_) => None,
        ModelMessage::User(text) => Some(serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": text }],
        })),
        ModelMessage::Assistant { text, tool_calls } => Some(assistant_message(text, tool_calls)),
        ModelMessage::ToolResult {
            tool_call_id,
            content,
        } => Some(serde_json::json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_call_id,
                "content": content,
            }],
        })),
    }
}

fn assistant_message(text: &Option<String>, tool_calls: &[ModelToolCall]) -> Value {
    let mut content: Vec<Value> = Vec::new();
    // 空文本块只在它是唯一内容时保留（否则省略，避免无意义的空 text 块）。
    if let Some(text) = text {
        if !text.is_empty() || tool_calls.is_empty() {
            content.push(serde_json::json!({ "type": "text", "text": text }));
        }
    }
    for call in tool_calls {
        content.push(serde_json::json!({
            "type": "tool_use",
            "id": call.tool_call_id,
            "name": call.tool_name,
            "input": call.input,
        }));
    }
    if content.is_empty() {
        // 中立消息允许 `Assistant{text: None, tool_calls: []}`（历史恢复的防御形态）：
        // Anthropic 不接受空 content 数组，退化成空 text 块而不是丢掉整条消息。
        content.push(serde_json::json!({ "type": "text", "text": "" }));
    }
    serde_json::json!({ "role": "assistant", "content": content })
}

fn anthropic_tool(definition: &ModelToolDefinition) -> Value {
    let mut tool = Map::new();
    tool.insert("name".to_string(), Value::String(definition.name.clone()));
    if let Some(description) = &definition.description {
        tool.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
    }
    tool.insert("input_schema".to_string(), definition.input_schema.clone());
    Value::Object(tool)
}

/// `{base_url}` + `/v1/messages`；base_url 末尾多余的 `/` 去掉再拼（公开文档端点）。
fn messages_url(base_url: &str) -> String {
    format!("{}/v1/messages", base_url.trim_end_matches('/'))
}

/// `extra_headers` 是否覆盖了某个默认头（头名按 HTTP 语义大小写不敏感比较）。
fn overrides_default(extra_headers: &[(String, String)], name: &str) -> bool {
    extra_headers
        .iter()
        .any(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
}

/// 一次完整流式请求：发请求 → 读 SSE → 累积 delta → 收口工具调用 → 汇总终态。
async fn stream_once(
    client: &reqwest::Client,
    config: &AnthropicMessagesConfig,
    request: &ModelRequest,
    deltas: &DeltaSink,
) -> Result<ModelResponse, ProviderError> {
    let body = build_messages_request(request, config);
    let url = messages_url(&config.base_url);
    let payload = serde_json::to_vec(&body)
        .map_err(|error| error::local(format!("模型请求体序列化失败：{error}")))?;
    // 默认头与 extra_headers 的取舍与 Chat Completions 适配同口径：reqwest 的
    // `header()` 是追加语义，默认头被 extra_headers 大小写不敏感命中时直接跳过，
    // 同名行只发 extra_headers 的那一条（新增默认头时必须在这里登记取舍）。
    let mut builder = client.post(&url).body(payload);
    if !overrides_default(&config.extra_headers, "content-type") {
        builder = builder.header(reqwest::header::CONTENT_TYPE, "application/json");
    }
    if !overrides_default(&config.extra_headers, "x-api-key") {
        builder = builder.header("x-api-key", &config.api_key);
    }
    if !overrides_default(&config.extra_headers, "anthropic-version") {
        builder = builder.header(
            "anthropic-version",
            config
                .anthropic_version
                .as_deref()
                .unwrap_or(DEFAULT_ANTHROPIC_VERSION),
        );
    }
    for (name, value) in &config.extra_headers {
        builder = builder.header(name, value);
    }
    let response = builder
        .send()
        .await
        .map_err(|error| error::transport(describe_transport(&error)))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(status_error(status.as_u16(), &body));
    }

    let mut chunks = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    let mut tracker = StreamTracker::default();
    let mut finished = false;
    while !finished {
        let chunk = match chunks.next().await {
            Some(chunk) => chunk,
            None => break,
        };
        let chunk = chunk.map_err(|error| error::transport(describe_transport(&error)))?;
        finished = ingest_payloads(decoder.push(&chunk), &mut tracker, deltas)?;
    }
    if !finished {
        // 连接直接结束：`finish` 把未以空行收尾的尾部事件补出来（例如 `message_stop`
        // 正好落在最后一个 chunk 且没有尾随空行），再判定流是否完整。
        finished = ingest_payloads(decoder.finish(), &mut tracker, deltas)?;
    }
    if !finished {
        return Err(error::malformed(
            "Anthropic 流在 message_stop 前结束（连接已关闭）".to_string(),
        ));
    }
    Ok(tracker.into_response())
}

/// 处理一批 SSE `data` 载荷；返回是否遇到 `message_stop`（其后的载荷是协议违规，忽略）。
fn ingest_payloads(
    payloads: Vec<String>,
    tracker: &mut StreamTracker,
    deltas: &DeltaSink,
) -> Result<bool, ProviderError> {
    for payload in payloads {
        let value = serde_json::from_str::<Value>(&payload).map_err(|error| {
            error::malformed(format!(
                "SSE data 不是合法 JSON：{error}；原文：{}",
                truncate(&payload)
            ))
        })?;
        if tracker.ingest(&value, deltas)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// reqwest 顶层文案不含根因（如证书被拒挂在 `source()` 链上），把整条链拼出来才可诊断。
fn describe_transport(error: &reqwest::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        parts.push(cause.to_string());
        source = cause.source();
    }
    parts.join(" <- ")
}

/// 把一条 delta 送进 sink；消费端已关闭时显式报错（与 `ScriptedProvider` 同一口径）。
fn send_delta(deltas: &DeltaSink, delta: ModelDelta) -> Result<(), ProviderError> {
    deltas
        .send(delta)
        .map_err(|_| error::local("delta sink 已关闭".to_string()))
}

/// 取一个「字符串 | null | 缺失」的字段；其它形状是契约违规，显式报错而不是静默吞。
fn optional_str<'a>(parent: &'a Value, key: &str) -> Result<Option<&'a str>, ProviderError> {
    match parent.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.as_str())),
        Some(other) => Err(error::malformed(format!("{key} 字段不是字符串：{other}"))),
    }
}

fn required_str<'a>(parent: &'a Value, key: &str) -> Result<&'a str, ProviderError> {
    optional_str(parent, key)?
        .filter(|text| !text.is_empty())
        .ok_or_else(|| error::malformed(format!("缺少 {key} 字段或为空")))
}

fn block_index(value: &Value) -> Result<u64, ProviderError> {
    value
        .get("index")
        .and_then(Value::as_u64)
        .ok_or_else(|| error::malformed("content block 事件缺少 index".to_string()))
}

/// Anthropic SSE 的 `error` 事件 / 非 2xx 正文里的 `{error:{type,message}}`。
fn error_detail(value: &Value) -> Option<(Option<String>, String)> {
    match value.get("error")? {
        Value::String(text) => Some((None, text.clone())),
        error_object => {
            let message = error_object.get("message").and_then(Value::as_str)?;
            let kind = error_object
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string);
            Some((kind, message.to_string()))
        }
    }
}

/// 非 2xx HTTP 响应：优先按 Anthropic `{error:{type,message}}` 提取类型与消息；
/// 不是该形态时回退既有 `error::http_status`（正文截断，错误必须可诊断）。
fn status_error(status: u16, body: &str) -> ProviderError {
    let parsed = serde_json::from_str::<Value>(body.trim())
        .ok()
        .and_then(|value| error_detail(&value));
    match parsed {
        Some((Some(kind), message)) => ProviderError {
            message: format!("HTTP {status}（{kind}）：{}", truncate(&message)),
        },
        Some((None, message)) => ProviderError {
            message: format!("HTTP {status}：{}", truncate(&message)),
        },
        None => error::http_status(status, body),
    }
}

fn in_stream_error(value: &Value) -> ProviderError {
    match error_detail(value) {
        Some((Some(kind), message)) => ProviderError {
            message: format!(
                "Anthropic 流内 error 事件（{kind}）：{}",
                truncate(&message)
            ),
        },
        Some((None, message)) => ProviderError {
            message: format!("Anthropic 流内 error 事件：{}", truncate(&message)),
        },
        None => error::malformed("Anthropic 流内 error 事件缺少 error.message".to_string()),
    }
}

/// usage 字段合并：Anthropic 在 `message_start.message.usage` 给输入侧统计，
/// 在 `message_delta.usage` 给累计输出；只覆盖实际出现的字段，缺失保持原值。
///
/// `reasoning_tokens` 恒为 0：Anthropic 不单独上报 thinking token（已含在
/// `output_tokens`），不猜字段。
fn merge_usage(target: &mut ModelUsage, usage: &Value) {
    let number = |key: &str| usage.get(key).and_then(Value::as_u64);
    if let Some(value) = number("input_tokens") {
        target.input_tokens = value;
    }
    if let Some(value) = number("output_tokens") {
        target.output_tokens = value;
    }
    if let Some(value) = number("cache_read_input_tokens") {
        target.cache_read_tokens = value;
    }
    if let Some(value) = number("cache_creation_input_tokens") {
        target.cache_write_tokens = value;
    }
}

/// 一个已开始的 content block 的流内状态（按 `index` 分键）。
enum BlockState {
    Text,
    /// thinking / redacted_thinking：内容只消费不转发（见模块注释）。
    Thinking,
    ToolUse {
        tool_call_id: String,
        tool_name: String,
        args: String,
    },
}

/// 流内事实的累积器：delta 逐条送 sink，usage / stop_reason / 工具状态留在本地，
/// 最后一次汇总成 [`ModelResponse`]。
#[derive(Default)]
struct StreamTracker {
    started: bool,
    usage: ModelUsage,
    stop_reason: Option<String>,
    blocks: BTreeMap<u64, BlockState>,
    emitted_calls: usize,
}

impl StreamTracker {
    /// 消费一帧 Anthropic 流事件；返回 `true` 表示收到 `message_stop`（终态）。
    fn ingest(&mut self, value: &Value, deltas: &DeltaSink) -> Result<bool, ProviderError> {
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| error::malformed("Anthropic 流事件缺少 type 字段".to_string()))?;
        match event_type {
            "ping" => Ok(false),
            "message_start" => {
                if self.started {
                    return Err(error::malformed(
                        "重复的 message_start（流只能有一个消息）".to_string(),
                    ));
                }
                let message = value.get("message").ok_or_else(|| {
                    error::malformed("message_start 缺少 message 字段".to_string())
                })?;
                if let Some(usage) = message.get("usage").filter(|usage| !usage.is_null()) {
                    merge_usage(&mut self.usage, usage);
                }
                self.started = true;
                Ok(false)
            }
            "content_block_start" => {
                self.require_started(event_type)?;
                self.start_block(value, deltas)?;
                Ok(false)
            }
            "content_block_delta" => {
                self.require_started(event_type)?;
                self.block_delta(value, deltas)?;
                Ok(false)
            }
            "content_block_stop" => {
                self.require_started(event_type)?;
                self.stop_block(value, deltas)?;
                Ok(false)
            }
            "message_delta" => {
                self.require_started(event_type)?;
                if let Some(delta) = value.get("delta") {
                    if let Some(reason) = optional_str(delta, "stop_reason")? {
                        self.stop_reason = Some(reason.to_string());
                    }
                }
                if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
                    merge_usage(&mut self.usage, usage);
                }
                Ok(false)
            }
            "message_stop" => {
                self.require_started(event_type)?;
                if !self.blocks.is_empty() {
                    return Err(error::malformed(format!(
                        "message_stop 前仍有 {} 个 content block 未收到 content_block_stop",
                        self.blocks.len()
                    )));
                }
                Ok(true)
            }
            "error" => Err(in_stream_error(value)),
            other => Err(error::malformed(format!(
                "未知的 Anthropic 流事件类型：{other}"
            ))),
        }
    }

    fn require_started(&self, event_type: &str) -> Result<(), ProviderError> {
        if self.started {
            return Ok(());
        }
        Err(error::malformed(format!(
            "{event_type} 出现在 message_start 之前"
        )))
    }

    fn start_block(&mut self, value: &Value, deltas: &DeltaSink) -> Result<(), ProviderError> {
        let index = block_index(value)?;
        if self.blocks.contains_key(&index) {
            return Err(error::malformed(format!("content block {index} 重复开始")));
        }
        let block = value.get("content_block").ok_or_else(|| {
            error::malformed("content_block_start 缺少 content_block 字段".to_string())
        })?;
        let block_type = block
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| error::malformed("content_block 缺少 type 字段".to_string()))?;
        match block_type {
            "text" => {
                // 公开流协议里 start 的 text 恒为空串；非空时如实作为首段文本发出。
                if let Some(text) = optional_str(block, "text")? {
                    if !text.is_empty() {
                        send_delta(
                            deltas,
                            ModelDelta::Text {
                                text: text.to_string(),
                            },
                        )?;
                    }
                }
                self.blocks.insert(index, BlockState::Text);
                Ok(())
            }
            // redacted_thinking 是 thinking 的公开变体（内容在 data 字段里加密）：
            // 与 thinking 一样只消费不转发。
            "thinking" | "redacted_thinking" => {
                self.blocks.insert(index, BlockState::Thinking);
                Ok(())
            }
            "tool_use" => {
                let tool_call_id = required_str(block, "id")?.to_string();
                let tool_name = required_str(block, "name")?.to_string();
                // 公开流协议的 input 在 start 时是空对象占位，真正的入参由
                // input_json_delta 拼出，因此这里不读取 input。
                send_delta(
                    deltas,
                    ModelDelta::ToolInputStart {
                        tool_call_id: tool_call_id.clone(),
                        tool_name: tool_name.clone(),
                    },
                )?;
                self.blocks.insert(
                    index,
                    BlockState::ToolUse {
                        tool_call_id,
                        tool_name,
                        args: String::new(),
                    },
                );
                Ok(())
            }
            other => Err(error::malformed(format!(
                "未知的 content_block 类型：{other}"
            ))),
        }
    }

    fn block_delta(&mut self, value: &Value, deltas: &DeltaSink) -> Result<(), ProviderError> {
        let index = block_index(value)?;
        let delta = value
            .get("delta")
            .ok_or_else(|| error::malformed("content_block_delta 缺少 delta 字段".to_string()))?;
        let delta_type = delta
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| error::malformed("content_block_delta 缺少 delta.type".to_string()))?;
        let state = self.blocks.get_mut(&index).ok_or_else(|| {
            error::malformed(format!(
                "content_block_delta 引用了未开始/已结束的块 {index}"
            ))
        })?;
        match state {
            BlockState::Text => {
                if delta_type != "text_delta" {
                    return Err(error::malformed(format!(
                        "text 块收到了 {delta_type}（应为 text_delta）"
                    )));
                }
                if let Some(text) = optional_str(delta, "text")? {
                    if !text.is_empty() {
                        send_delta(
                            deltas,
                            ModelDelta::Text {
                                text: text.to_string(),
                            },
                        )?;
                    }
                }
                Ok(())
            }
            BlockState::Thinking => {
                if delta_type != "thinking_delta" && delta_type != "signature_delta" {
                    return Err(error::malformed(format!(
                        "thinking 块收到了 {delta_type}（应为 thinking_delta/signature_delta）"
                    )));
                }
                // 已登记：没有 reasoning 变体，thinking/signature 只消费不转发。
                Ok(())
            }
            BlockState::ToolUse {
                tool_call_id, args, ..
            } => {
                if delta_type != "input_json_delta" {
                    return Err(error::malformed(format!(
                        "tool_use 块收到了 {delta_type}（应为 input_json_delta）"
                    )));
                }
                let partial = required_str(delta, "partial_json")?;
                if !partial.is_empty() {
                    args.push_str(partial);
                    send_delta(
                        deltas,
                        ModelDelta::ToolInputDelta {
                            tool_call_id: tool_call_id.clone(),
                            delta: partial.to_string(),
                        },
                    )?;
                }
                Ok(())
            }
        }
    }

    fn stop_block(&mut self, value: &Value, deltas: &DeltaSink) -> Result<(), ProviderError> {
        let index = block_index(value)?;
        let Some(state) = self.blocks.remove(&index) else {
            return Err(error::malformed(format!(
                "content_block_stop 引用了未开始/已结束的块 {index}"
            )));
        };
        let BlockState::ToolUse {
            tool_call_id,
            tool_name,
            args,
        } = state
        else {
            return Ok(());
        };
        send_delta(
            deltas,
            ModelDelta::ToolInputEnd {
                tool_call_id: tool_call_id.clone(),
            },
        )?;
        let input = parse_tool_input(&tool_call_id, &tool_name, &args)?;
        send_delta(
            deltas,
            ModelDelta::ToolCall {
                tool_call_id,
                tool_name,
                input,
            },
        )?;
        self.emitted_calls += 1;
        Ok(())
    }

    fn into_response(self) -> ModelResponse {
        // 会话层只区分 stop / tool-calls：`tool_use` 是唯一进入工具批次的原因，
        // `end_turn` / `max_tokens` / `stop_sequence` 等折叠为 stop（C2 §4.1.1）。
        // stop_reason 缺失（不合规端点）时按已产出内容推断。
        let stop_reason = match self.stop_reason.as_deref() {
            Some("tool_use") => "tool-calls",
            Some(_) => "stop",
            None if self.emitted_calls > 0 => "tool-calls",
            None => "stop",
        };
        ModelResponse {
            usage: self.usage,
            stop_reason,
        }
    }
}

/// 入参解析：空串/纯空白（无参工具没有 `input_json_delta`）按 `{}`；其余必须可解析
/// 成 JSON，失败显式报错——静默丢调用会让下一轮上下文缺工具结果。
fn parse_tool_input(
    tool_call_id: &str,
    tool_name: &str,
    arguments: &str,
) -> Result<Value, ProviderError> {
    if arguments.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(arguments).map_err(|error| {
        error::malformed(format!(
            "工具调用 {tool_call_id}（{tool_name}）的入参不是合法 JSON：{error}；原文：{}",
            truncate(arguments)
        ))
    })
}
