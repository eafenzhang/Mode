//! OpenAI Chat Completions 流式适配（LT-1 C1 · Task 4）。
//!
//! 契约来源：OpenAI 公开的 Chat Completions API 文档（clean-room：不读任何上游实现）。
//! 覆盖 C1 需要的子集：
//! - 请求：`model` / `messages` / `tools`（function）/ `stream: true` /
//!   `stream_options.include_usage` / `max_tokens`（配置给出时才有）；
//! - 流：`data:` JSON 帧（`choices[].delta.content` / `delta.tool_calls` / `finish_reason`）、
//!   usage 尾帧（`choices` 为空数组）、`data: [DONE]` 哨兵；
//! - 工具调用 delta 按 `tool_calls[].index` 累积，产出
//!   `ToolInputStart → ToolInputDelta… → ToolInputEnd → ToolCall`，
//!   与 `agent-core` 的 `ScriptedProvider` 同一组序；
//! - finish_reason 映射：`tool_calls`（含遗留 `function_call`）→ `tool-calls`；
//!   `stop` / `length` / `content_filter` → `stop`（C1 的会话层只区分这两类）；
//!   整个流都没给 finish_reason 时按已产出内容推断（发过工具调用 → `tool-calls`）。
//!
//! 不做（C3 或后续检查点）：重试/退避、断流恢复、Anthropic/Responses 适配。

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

/// OpenAI 流的结束哨兵。
const DONE_SENTINEL: &str = "[DONE]";

/// OpenAI Chat Completions 端点的连接配置。
///
/// `base_url` 是 API 根（如 `https://api.openai.com/v1`，兼容网关同形），适配器在其后拼
/// `/chat/completions`，末尾多余的 `/` 会被去掉。`extra_headers` 对默认头
/// （`Authorization: Bearer` / `Content-Type: application/json`）是**替换**语义：头名
/// 大小写不敏感地命中默认头时，默认值不发（见 [`stream_once`] 的取舍说明），网关因此可以
/// 自带鉴权头而不必担心默认 Bearer 静默并存；`extra_headers` 之外的同名默认头照常发送。
/// reqwest 的 `header()` 本身是追加语义，同名头在 `extra_headers` 内部出现多次会原样
/// 多发（多值头留给调用方自己负责）。`Debug` 不打印密钥：`api_key` 恒为掩码，
/// `extra_headers` 只显示头名（值可能是另一把凭据）。
#[derive(Clone)]
pub struct OpenAiChatConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_output_tokens: Option<u64>,
    pub extra_headers: Vec<(String, String)>,
}

impl fmt::Debug for OpenAiChatConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiChatConfig")
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
            .finish()
    }
}

/// `ModelProvider` 的 OpenAI Chat Completions 实现：`POST {base_url}/chat/completions`
/// 并把 SSE 流转成 [`ModelDelta`] 序列 + [`ModelResponse`]。
pub struct OpenAiChatProvider {
    client: reqwest::Client,
    config: OpenAiChatConfig,
}

impl OpenAiChatProvider {
    /// 构造 provider。进程级 TLS provider 在这里装：工作空间的 `reqwest` 以
    /// `rustls-no-provider` 编译，生产 HTTPS 需要有人装 provider——与
    /// `coding-plan-http` / `platform::source_url` 同一做法（`install_default` 已装时
    /// 返回 `Err`，忽略即可，重复调用安全）。
    ///
    /// 不设整体超时：流式应答可能持续数分钟，客户端整体超时会杀掉正常长流；
    /// 超时/重试语义归 C3 的故障面（spec §5.6）。
    pub fn new(config: OpenAiChatConfig) -> Result<Self, reqwest::Error> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        Ok(Self {
            client: reqwest::Client::builder().build()?,
            config,
        })
    }
}

impl ModelProvider for OpenAiChatProvider {
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

/// 把 provider 中立的请求映射成 OpenAI Chat Completions 请求体（spec §5.6）。
///
/// 消息映射：
/// - `System` → `{role:"system"}`，`User` → `{role:"user"}`；
/// - `Assistant{text, tool_calls}` → `{role:"assistant"}`，`content` 只在非空文本时出现
///   （`None` 与 `""` 都省略键），`tool_calls` 只在非空时出现（OpenAI 不接受空数组的
///   工具调用）；`arguments` 是入参 JSON 的紧凑序列化字符串；
/// - `ToolResult` → `{role:"tool", tool_call_id, content}`。
///
/// 工具声明 → `{type:"function", function:{name, description?, parameters}}`：
/// `description` 缺失时省略键（不为内置工具编造描述），`parameters` 逐字取
/// `input_schema`。
pub fn build_chat_request(request: &ModelRequest, config: &OpenAiChatConfig) -> Value {
    let messages: Vec<Value> = request.messages.iter().map(chat_message).collect();
    let mut body = Map::new();
    body.insert("model".to_string(), Value::String(config.model.clone()));
    body.insert("messages".to_string(), Value::Array(messages));
    if !request.tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(request.tools.iter().map(function_tool).collect()),
        );
    }
    body.insert("stream".to_string(), Value::Bool(true));
    body.insert(
        "stream_options".to_string(),
        serde_json::json!({ "include_usage": true }),
    );
    if let Some(max_output_tokens) = config.max_output_tokens {
        body.insert("max_tokens".to_string(), Value::from(max_output_tokens));
    }
    Value::Object(body)
}

fn chat_message(message: &ModelMessage) -> Value {
    match message {
        ModelMessage::System(text) => serde_json::json!({ "role": "system", "content": text }),
        ModelMessage::User(text) => serde_json::json!({ "role": "user", "content": text }),
        ModelMessage::Assistant { text, tool_calls } => {
            let mut message = Map::new();
            message.insert("role".to_string(), Value::String("assistant".to_string()));
            if let Some(text) = text.as_deref().filter(|text| !text.is_empty()) {
                message.insert("content".to_string(), Value::String(text.to_string()));
            }
            if !tool_calls.is_empty() {
                message.insert(
                    "tool_calls".to_string(),
                    Value::Array(tool_calls.iter().map(outgoing_tool_call).collect()),
                );
            }
            Value::Object(message)
        }
        ModelMessage::ToolResult {
            tool_call_id,
            content,
        } => serde_json::json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": content,
        }),
    }
}

fn outgoing_tool_call(call: &ModelToolCall) -> Value {
    serde_json::json!({
        "id": call.tool_call_id,
        "type": "function",
        "function": {
            "name": call.tool_name,
            "arguments": call.input.to_string(),
        },
    })
}

fn function_tool(definition: &ModelToolDefinition) -> Value {
    let mut function = Map::new();
    function.insert("name".to_string(), Value::String(definition.name.clone()));
    if let Some(description) = &definition.description {
        function.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
    }
    function.insert("parameters".to_string(), definition.input_schema.clone());
    serde_json::json!({ "type": "function", "function": Value::Object(function) })
}

/// `{base_url}` + `/chat/completions`；base_url 末尾多余的 `/` 去掉再拼。
fn chat_completions_url(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
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
    config: &OpenAiChatConfig,
    request: &ModelRequest,
    deltas: &DeltaSink,
) -> Result<ModelResponse, ProviderError> {
    let body = build_chat_request(request, config);
    let url = chat_completions_url(&config.base_url);
    let payload = serde_json::to_vec(&body)
        .map_err(|error| error::local(format!("模型请求体序列化失败：{error}")))?;
    // 默认头与 extra_headers 的取舍在构造请求前决定，而不是依赖追加顺序「覆盖」：
    // reqwest 的 `header()` 是追加语义，同名头会真的发出两条（HTTP/1.1 服务端通常取
    // 第一条），网关自带 Authorization 时就会静默用错凭据。因此默认头（当前只有
    // authorization / content-type 两个）被 extra_headers 大小写不敏感命中时直接跳过，
    // 同名行只发 extra_headers 的那一条；新增默认头时必须同时在这里登记取舍。
    let mut builder = client.post(&url).body(payload);
    if !overrides_default(&config.extra_headers, "content-type") {
        builder = builder.header(reqwest::header::CONTENT_TYPE, "application/json");
    }
    if !overrides_default(&config.extra_headers, "authorization") {
        builder = builder.bearer_auth(&config.api_key);
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
        return Err(error::http_status(status.as_u16(), &body));
    }

    let mut chunks = response.bytes_stream();
    let mut decoder = SseDecoder::new();
    let mut tracker = ChunkTracker::default();
    let mut done = false;
    while !done {
        let chunk = match chunks.next().await {
            Some(chunk) => chunk,
            None => break,
        };
        let chunk = chunk.map_err(|error| error::stream_abort(describe_transport(&error)))?;
        done = ingest_payloads(decoder.push(&chunk), &mut tracker, deltas)?;
    }
    if !done {
        // 流没以 [DONE] 收尾（连接直接结束）：`finish` 把未以空行收尾的尾部事件补出来，
        // 否则 usage 尾帧等最后一条载荷会被静默丢掉。
        done = ingest_payloads(decoder.finish(), &mut tracker, deltas)?;
    }
    // 既没有 [DONE] 也没有 finish_reason：连接在终止事件前关闭 = 流中断（可重试）。
    // 有 finish_reason 的兼容端点（只在连接结束时补 usage 尾帧）仍是成功。
    if !done && tracker.finish_reason.is_none() {
        return Err(error::stream_abort(
            "OpenAI Chat 流在 [DONE]/finish_reason 前结束（连接已关闭）".to_string(),
        ));
    }
    tracker.close_all(deltas)?;
    Ok(tracker.into_response())
}

/// 处理一批 SSE `data` 载荷；返回是否遇到 `[DONE]`（哨兵之后的载荷是协议违规，忽略）。
fn ingest_payloads(
    payloads: Vec<String>,
    tracker: &mut ChunkTracker,
    deltas: &DeltaSink,
) -> Result<bool, ProviderError> {
    for payload in payloads {
        if payload == DONE_SENTINEL {
            return Ok(true);
        }
        let value = serde_json::from_str::<Value>(&payload).map_err(|error| {
            error::malformed(format!(
                "SSE data 不是合法 JSON：{error}；原文：{}",
                truncate(&payload)
            ))
        })?;
        tracker.ingest(&value, deltas)?;
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

/// usage 尾帧 → 中立统计。缺字段按 0 处理（OpenAI 只保证 prompt/completion 两组）。
fn parse_usage(usage: &Value) -> ModelUsage {
    let number = |parent: &Value, key: &str| parent.get(key).and_then(Value::as_u64).unwrap_or(0);
    let mut model_usage = ModelUsage {
        input_tokens: number(usage, "prompt_tokens"),
        output_tokens: number(usage, "completion_tokens"),
        ..ModelUsage::default()
    };
    if let Some(details) = usage.get("prompt_tokens_details") {
        model_usage.cache_read_tokens = number(details, "cached_tokens");
    }
    // OpenAI Chat Completions 自身没有 cache 写入字段；部分 OpenAI 兼容网关会透传
    // Anthropic 系的 `cache_creation_input_tokens`，给了就收，不给保持 0（不猜字段）。
    model_usage.cache_write_tokens = number(usage, "cache_creation_input_tokens");
    if let Some(details) = usage.get("completion_tokens_details") {
        model_usage.reasoning_tokens = number(details, "reasoning_tokens");
    }
    model_usage
}

/// 一条工具调用在流内的累积状态（按 `tool_calls[].index` 分键）。
#[derive(Default)]
struct CallState {
    id: Option<String>,
    name: String,
    args: String,
    /// 已累积但还没作为 `ToolInputDelta` 发出的入参分片（start 之前的分片等名字齐了再发）。
    unsent: String,
    started: bool,
}

/// 流内事实的累积器：delta 逐条送 sink，usage / finish_reason / 工具调用留在本地，
/// 最后一次汇总成 [`ModelResponse`]（delta 不在 response 里重复承载——避免同一批 delta
/// 有两条投递路径，Task 3 的接口收敛）。
#[derive(Default)]
struct ChunkTracker {
    calls: BTreeMap<u64, CallState>,
    finish_reason: Option<String>,
    usage: ModelUsage,
    emitted_calls: usize,
}

impl ChunkTracker {
    /// 消费一帧 Chat Completions JSON。
    fn ingest(&mut self, value: &Value, deltas: &DeltaSink) -> Result<(), ProviderError> {
        // HTTP 200 但数据面出错的兼容形态（网关把错误塞进流里）：显式报错，不吞。
        if let Some(message) = error::payload_message(value) {
            return Err(error::malformed(format!("流内 error 帧：{message}")));
        }
        if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
            self.usage = parse_usage(usage);
        }
        for choice in value
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(reason) = optional_str(choice, "finish_reason")? {
                self.finish_reason = Some(reason.to_string());
            }
            let Some(delta) = choice.get("delta") else {
                continue;
            };
            if let Some(text) = optional_str(delta, "content")? {
                if !text.is_empty() {
                    send_delta(
                        deltas,
                        ModelDelta::Text {
                            text: text.to_string(),
                        },
                    )?;
                }
            }
            for fragment in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                self.ingest_tool_fragment(fragment, deltas)?;
            }
        }
        Ok(())
    }

    /// 消费一个 `tool_calls` 分片：按 index 累积 id / name / arguments，并推进 start 与
    /// delta 的发出时机。
    fn ingest_tool_fragment(
        &mut self,
        fragment: &Value,
        deltas: &DeltaSink,
    ) -> Result<(), ProviderError> {
        let index = fragment
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| error::malformed("tool_calls 分片缺少 index".to_string()))?;
        // 新 index 开始：OpenAI 按 index 顺序发分片，先收口更早的调用再处理新片，
        // 保证 Start/Delta/End/ToolCall 按调用次序成组（与 ScriptedProvider 同序）。
        let open_below: Vec<u64> = self.calls.range(..index).map(|(key, _)| *key).collect();
        for open in open_below {
            self.close_call(open, deltas)?;
        }
        let state = self.calls.entry(index).or_default();
        if let Some(id) = optional_str(fragment, "id")? {
            if !id.is_empty() && state.id.is_none() {
                state.id = Some(id.to_string());
            }
        }
        if let Some(function) = fragment.get("function") {
            if let Some(name) = optional_str(function, "name")? {
                state.name.push_str(name);
            }
            if let Some(arguments) = optional_str(function, "arguments")? {
                if !arguments.is_empty() {
                    state.args.push_str(arguments);
                    state.unsent.push_str(arguments);
                }
            }
        }
        if let Some(tool_call_id) = state.id.clone() {
            advance_streaming(&tool_call_id, state, deltas)?;
        }
        Ok(())
    }

    /// 收口一条工具调用：补发未发出的 start / 分片，然后 End 与终态 ToolCall。
    fn close_call(&mut self, index: u64, deltas: &DeltaSink) -> Result<(), ProviderError> {
        let Some(mut state) = self.calls.remove(&index) else {
            return Ok(());
        };
        let Some(tool_call_id) = state.id.clone() else {
            return Err(error::malformed(format!(
                "工具调用（index {index}）缺少 id"
            )));
        };
        if !state.started {
            // name 迟到或零入参的调用在这里补 start；名字始终缺失是契约违规，显式报错
            //（把无名调用静默丢掉会让下一轮上下文缺一条工具结果，更难查）。
            if state.name.is_empty() {
                return Err(error::malformed(format!(
                    "工具调用 {tool_call_id} 缺少 function.name"
                )));
            }
            send_delta(
                deltas,
                ModelDelta::ToolInputStart {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: state.name.clone(),
                },
            )?;
        }
        flush_unsent(&tool_call_id, &mut state, deltas)?;
        send_delta(
            deltas,
            ModelDelta::ToolInputEnd {
                tool_call_id: tool_call_id.clone(),
            },
        )?;
        let tool_name = state.name;
        let input = parse_tool_input(&tool_call_id, &tool_name, &state.args)?;
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

    /// 流结束时收口所有仍打开的调用（按 index 次序）。
    fn close_all(&mut self, deltas: &DeltaSink) -> Result<(), ProviderError> {
        let indexes: Vec<u64> = self.calls.keys().copied().collect();
        for index in indexes {
            self.close_call(index, deltas)?;
        }
        Ok(())
    }

    fn into_response(self) -> ModelResponse {
        // finish_reason 缺失（流被截断或不合规端点）时按已产出内容推断：
        // 发过工具调用按 tool-calls 收，否则按 stop 收——会话层按 stop_reason
        // 决定是否进工具批次。
        let stop_reason = match self.finish_reason.as_deref() {
            Some("tool_calls") | Some("function_call") => "tool-calls",
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

/// 分片推进：「id 齐 + 名字齐 + 已有入参分片（或已 start）」时保证 ToolInputStart 已发、
/// 未发分片已补成 `ToolInputDelta`。
///
/// 为什么要等第一个入参分片才发 start：工具名与入参在线上都是「先名后参」的多片字符串，
/// 名字没有结束标记；等第一个 arguments 分片到达才认为名字已收齐，避免拆成两片的工具名
/// 被截半发出去（Start 一发就定名，下游按名字路由工具）。名字永远不来 / 没有入参的
/// 调用由 `close_call` 补发或报错。
fn advance_streaming(
    tool_call_id: &str,
    state: &mut CallState,
    deltas: &DeltaSink,
) -> Result<(), ProviderError> {
    if state.name.is_empty() || (state.unsent.is_empty() && !state.started) {
        return Ok(());
    }
    if !state.started {
        send_delta(
            deltas,
            ModelDelta::ToolInputStart {
                tool_call_id: tool_call_id.to_string(),
                tool_name: state.name.clone(),
            },
        )?;
        state.started = true;
    }
    flush_unsent(tool_call_id, state, deltas)
}

/// 把 start 前累积的入参分片一次性补发成一条 `ToolInputDelta`。
fn flush_unsent(
    tool_call_id: &str,
    state: &mut CallState,
    deltas: &DeltaSink,
) -> Result<(), ProviderError> {
    if state.unsent.is_empty() {
        return Ok(());
    }
    let delta = std::mem::take(&mut state.unsent);
    send_delta(
        deltas,
        ModelDelta::ToolInputDelta {
            tool_call_id: tool_call_id.to_string(),
            delta,
        },
    )
}

/// 入参解析：空串/纯空白（线上是 `"arguments": ""` 或缺省）按无参工具处理成 `{}`；
/// 其余必须可解析成 JSON，失败显式报错——静默丢调用会让下一轮上下文缺工具结果。
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
