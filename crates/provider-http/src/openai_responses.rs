//! OpenAI Responses 流式适配（LT-1 C2 · Task 5）。
//!
//! 契约来源：OpenAI 公开的 Responses API 文档（clean-room：不读任何上游实现）。
//! 覆盖 C2 需要的子集：
//! - 请求：`model` / `instructions`（系统）/ `input`（消息与工具结果项）/
//!   `tools`（扁平 `function`）/ `max_output_tokens`（配置给出时）/
//!   `store`（配置给出时）/ `stream: true`；
//! - 头：`content-type`、`authorization: Bearer`（与 Chat Completions 同一替换语义）；
//! - 流：`response.output_text.delta` → `Text`；`response.function_call_arguments.delta`
//!   按 `output_index` 分键累积 → `ToolInputDelta`；`response.output_item.added`
//!   （`function_call`）→ `ToolInputStart`；`response.output_item.done`
//!   （`function_call`）→ `ToolInputEnd → ToolCall`；`response.completed` /
//!   `response.incomplete` 收 usage 与终态；`response.failed` 与 `error` 显式报错；
//! - stop：`completed` → 有工具调用 `"tool-calls"` 否则 `"stop"`；`incomplete`
//!   （任何 `incomplete_details.reason`，含 `max_output_tokens`）一律 `"stop"`
//!   （会话层只区分这两类，登记于 C2 §4.2.1）。
//!
//! 已登记的取舍（`docs/specs/agent-c2-context-memory.md` §4.2.1、
//! `docs/specs/tauri-agent-runtime.md` §C1.7）：
//! - 多条 `System` 用 [`INSTRUCTIONS_SEPARATOR`]（`"\n\n"`）拼接进 `instructions`
//!   （装配恒一条 System；分隔符是登记的确定性选择，非观察值）。
//! - 其余流事件（`response.created` / `response.in_progress`、message / reasoning 项
//!   的 added/done、`response.output_text.done`、
//!   `response.function_call_arguments.done` 等公开流必发事件）安静忽略，不按
//!   未知类型报错。
//! - 工具入参按 `output_index` 分键累积，绝不做字符串全局拼接；收口时优先用已累积
//!   分片解析，分片为空时回退 done 项的 `arguments`。
//! - 流必须以 `response.completed` / `response.incomplete` / `response.failed` 收尾，
//!   否则显式报错，不把截断流伪装成成功终态。
//!
//! 不做（C3 或后续检查点）：重试/退避、断流恢复、请求侧 reasoning/store 之外的
//! 高级参数（`tool_choice` / `previous_response_id` / `conversation` 等）。

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

/// 多条 `System` 消息拼接进 `instructions` 的分隔符。
///
/// 装配（`agent-core::context`）恒产出一条 `System`，该常量只在历史里出现多条
/// 系统段时生效；取 `"\n\n"` 是登记的确定性选择（C2 §4.2.1），不是对 oracle 的观察。
pub const INSTRUCTIONS_SEPARATOR: &str = "\n\n";

/// OpenAI Responses 端点的连接配置。
///
/// `base_url` 是 API 根（如 `https://api.openai.com/v1`，兼容网关同形），适配器在其后拼
/// `/responses`，末尾多余的 `/` 会被去掉。`extra_headers` 对默认头
/// （`Content-Type` / `Authorization: Bearer`）是**替换**语义：头名大小写不敏感地命中
/// 默认头时，默认值不发，同名行只保留 `extra_headers` 的那一条；因此网关可以自带鉴权
/// 头而不必担心默认 Bearer 静默并存。`store` 是 Responses 的请求参数（配置给出时才发，
/// 当前选择面恒 `None`——provider 配置没有该字段，不发明默认值）。`Debug` 不打印密钥：
/// `api_key` 恒为掩码，`extra_headers` 只显示头名（值可能是另一把凭据）。
#[derive(Clone)]
pub struct OpenAiResponsesConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub max_output_tokens: Option<u64>,
    pub extra_headers: Vec<(String, String)>,
    pub store: Option<bool>,
}

impl fmt::Debug for OpenAiResponsesConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiResponsesConfig")
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
            .field("store", &self.store)
            .finish()
    }
}

/// `ModelProvider` 的 OpenAI Responses 实现：`POST {base_url}/responses`
/// 并把 SSE 流转成 [`ModelDelta`] 序列 + [`ModelResponse`]。
pub struct OpenAiResponsesProvider {
    client: reqwest::Client,
    config: OpenAiResponsesConfig,
}

impl OpenAiResponsesProvider {
    /// 构造 provider。进程级 TLS provider 在这里装（与 `OpenAiChatProvider` 同一做法：
    /// 工作空间的 `reqwest` 以 `rustls-no-provider` 编译，重复安装安全）。
    ///
    /// 不设整体超时：流式应答可能持续数分钟；超时/重试语义归 C3（spec §5.6）。
    pub fn new(config: OpenAiResponsesConfig) -> Result<Self, reqwest::Error> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        Ok(Self {
            client: reqwest::Client::builder().build()?,
            config,
        })
    }
}

impl ModelProvider for OpenAiResponsesProvider {
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

/// 把 provider 中立的请求映射成 OpenAI Responses 请求体（C2 §4.2）。
///
/// - 所有 `System` 保序用 [`INSTRUCTIONS_SEPARATOR`] 拼接进顶层 `instructions`
///   （没有 `System` 时省略键；`System` 不进 `input`）；
/// - `User` → `{type:"message", role:"user", content:[{type:"input_text", text}]}`；
/// - `Assistant` 有非空文本 → `{type:"message", role:"assistant",
///   content:[{type:"output_text", text}]}`；每个调用 → `{type:"function_call",
///   call_id, name, arguments}`（`arguments` 是入参 JSON 字符串）；
/// - `ToolResult` → `{type:"function_call_output", call_id, output}`；项序 = 历史序；
/// - 工具声明 → 扁平 `{type:"function", name, description?, parameters}`
///   （`description` 缺失时省略键，不编造）；
/// - `max_output_tokens` 仅在配置 `Some` 时发；`store` 仅在 `Some` 时发；
/// - `stream: true`。
pub fn build_responses_request(request: &ModelRequest, config: &OpenAiResponsesConfig) -> Value {
    let mut body = Map::new();
    body.insert("model".to_string(), Value::String(config.model.clone()));

    let system: Vec<&str> = request
        .messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::System(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    if !system.is_empty() {
        body.insert(
            "instructions".to_string(),
            Value::String(system.join(INSTRUCTIONS_SEPARATOR)),
        );
    }

    let mut input: Vec<Value> = Vec::with_capacity(request.messages.len());
    for message in &request.messages {
        push_input_items(message, &mut input);
    }
    body.insert("input".to_string(), Value::Array(input));

    if !request.tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(request.tools.iter().map(function_tool).collect()),
        );
    }
    if let Some(max_output_tokens) = config.max_output_tokens {
        body.insert(
            "max_output_tokens".to_string(),
            Value::from(max_output_tokens),
        );
    }
    if let Some(store) = config.store {
        body.insert("store".to_string(), Value::Bool(store));
    }
    body.insert("stream".to_string(), Value::Bool(true));
    Value::Object(body)
}

/// 一条中立消息 → 0..n 条 Responses `input` 项（按历史序展开）。
///
/// - `System` 不进 `input`（已在 `instructions` 处理）；
/// - `Assistant` 的文本项在前、调用项在后；无文本且无调用（历史恢复的防御形态）
///   不产出项——Responses 没有「空 assistant 消息」的有用表达，丢掉不改变语义。
fn push_input_items(message: &ModelMessage, out: &mut Vec<Value>) {
    match message {
        ModelMessage::System(_) => {}
        ModelMessage::User(text) => out.push(serde_json::json!({
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": text }],
        })),
        ModelMessage::Assistant { text, tool_calls } => {
            if let Some(text) = text.as_deref().filter(|text| !text.is_empty()) {
                out.push(serde_json::json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": text }],
                }));
            }
            for call in tool_calls {
                out.push(function_call_item(call));
            }
        }
        ModelMessage::ToolResult {
            tool_call_id,
            content,
        } => out.push(serde_json::json!({
            "type": "function_call_output",
            "call_id": tool_call_id,
            "output": content,
        })),
    }
}

fn function_call_item(call: &ModelToolCall) -> Value {
    serde_json::json!({
        "type": "function_call",
        "call_id": call.tool_call_id,
        "name": call.tool_name,
        "arguments": call.input.to_string(),
    })
}

fn function_tool(definition: &ModelToolDefinition) -> Value {
    let mut tool = Map::new();
    tool.insert("type".to_string(), Value::String("function".to_string()));
    tool.insert("name".to_string(), Value::String(definition.name.clone()));
    if let Some(description) = &definition.description {
        tool.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
    }
    tool.insert("parameters".to_string(), definition.input_schema.clone());
    Value::Object(tool)
}

/// `{base_url}` + `/responses`；base_url 末尾多余的 `/` 去掉再拼（公开文档端点）。
fn responses_url(base_url: &str) -> String {
    format!("{}/responses", base_url.trim_end_matches('/'))
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
    config: &OpenAiResponsesConfig,
    request: &ModelRequest,
    deltas: &DeltaSink,
) -> Result<ModelResponse, ProviderError> {
    let body = build_responses_request(request, config);
    let url = responses_url(&config.base_url);
    let payload = serde_json::to_vec(&body)
        .map_err(|error| error::local(format!("模型请求体序列化失败：{error}")))?;
    // 默认头与 extra_headers 的取舍与 Chat Completions 适配同口径：reqwest 的
    // `header()` 是追加语义，默认头被 extra_headers 大小写不敏感命中时直接跳过，
    // 同名行只发 extra_headers 的那一条（新增默认头时必须在这里登记取舍）。
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
    let mut tracker = StreamTracker::default();
    let mut finished = false;
    while !finished {
        let chunk = match chunks.next().await {
            Some(chunk) => chunk,
            None => break,
        };
        let chunk = chunk.map_err(|error| error::stream_abort(describe_transport(&error)))?;
        finished = ingest_payloads(decoder.push(&chunk), &mut tracker, deltas)?;
    }
    if !finished {
        // 连接直接结束：`finish` 把未以空行收尾的尾部事件补出来（例如终态事件正好
        // 落在最后一个 chunk 且没有尾随空行），再判定流是否完整。
        finished = ingest_payloads(decoder.finish(), &mut tracker, deltas)?;
    }
    if !finished {
        // 连接在终态事件前关闭 = 流中断（可重试）。
        return Err(error::stream_abort(
            "OpenAI Responses 流在终态事件（response.completed/incomplete/failed）前结束（连接已关闭）"
                .to_string(),
        ));
    }
    Ok(tracker.into_response())
}

/// 处理一批 SSE `data` 载荷；返回是否遇到终态事件（其后的载荷是协议违规，忽略）。
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

/// 事件的 `output_index`：added / done / arguments delta 三个事件都必带（公开 schema）。
/// 它是工具入参累积的分键（C2 §4.2.1：绝不做字符串全局拼接）。
fn output_index(value: &Value) -> Result<u64, ProviderError> {
    value
        .get("output_index")
        .and_then(Value::as_u64)
        .ok_or_else(|| error::malformed("流事件缺少 output_index".to_string()))
}

/// usage 字段合并（公开 API：`input_tokens` / `output_tokens` /
/// `input_tokens_details.cached_tokens` → cache_read /
/// `input_tokens_details.cache_write_tokens` → cache_write /
/// `output_tokens_details.reasoning_tokens` → reasoning）。只覆盖实际出现的字段，
/// 缺失保持原值。
fn merge_usage(target: &mut ModelUsage, usage: &Value) {
    let number = |parent: &Value, key: &str| parent.get(key).and_then(Value::as_u64);
    if let Some(value) = number(usage, "input_tokens") {
        target.input_tokens = value;
    }
    if let Some(value) = number(usage, "output_tokens") {
        target.output_tokens = value;
    }
    if let Some(details) = usage.get("input_tokens_details").filter(|d| !d.is_null()) {
        target.cache_read_tokens = number(details, "cached_tokens").unwrap_or(0);
        target.cache_write_tokens = number(details, "cache_write_tokens").unwrap_or(0);
    }
    if let Some(details) = usage.get("output_tokens_details").filter(|d| !d.is_null()) {
        target.reasoning_tokens = number(details, "reasoning_tokens").unwrap_or(0);
    }
}

/// `response.failed` 的 `response.error`（对象 `{code, message}` 或 null）。
fn failed_error(value: &Value) -> ProviderError {
    let detail = value
        .get("response")
        .and_then(|response| response.get("error"))
        .filter(|error| !error.is_null());
    match detail {
        Some(error) => {
            let code = error.get("code").and_then(Value::as_str);
            let message = error.get("message").and_then(Value::as_str).unwrap_or("");
            match code {
                Some(code) => ProviderError::api(format!(
                    "OpenAI Responses response.failed（{code}）：{}",
                    truncate(message)
                )),
                None => ProviderError::api(format!(
                    "OpenAI Responses response.failed：{}",
                    truncate(message)
                )),
            }
        }
        None => ProviderError::api("OpenAI Responses response.failed 缺少 response.error"),
    }
}

/// 流内 `error` 事件（`{type:"error", code, message, param}`）。
fn in_stream_error(value: &Value) -> ProviderError {
    let code = value.get("code").and_then(Value::as_str);
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty());
    match (code, message) {
        (Some(code), Some(message)) => ProviderError::api(format!(
            "OpenAI Responses 流内 error 事件（{code}）：{}",
            truncate(message)
        )),
        (Some(code), None) => {
            ProviderError::api(format!("OpenAI Responses 流内 error 事件（{code}）"))
        }
        (None, Some(message)) => ProviderError::api(format!(
            "OpenAI Responses 流内 error 事件：{}",
            truncate(message)
        )),
        (None, None) => {
            error::malformed("OpenAI Responses 流内 error 事件缺少 message".to_string())
        }
    }
}

/// 一条工具调用在流内的累积状态（按 `output_index` 分键）。
#[derive(Default)]
struct CallState {
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    /// 已累积的入参分片（收口时优先用它解析，保证与已转发的 delta 一致）。
    args: String,
    /// 已累积但还没作为 `ToolInputDelta` 发出的分片（added 之前的分片等 call_id
    /// 齐了再发——`ToolInputDelta` 必须带 call_id）。
    unsent: String,
    started: bool,
}

/// 流内事实的累积器：delta 逐条送 sink，usage / 终态 / 工具状态留在本地，
/// 最后一次汇总成 [`ModelResponse`]。
#[derive(Default)]
struct StreamTracker {
    /// 收到的终态：`completed`（按已产出调用推断 stop）或 `incomplete`（恒 stop）。
    terminal: Option<Terminal>,
    usage: ModelUsage,
    calls: BTreeMap<u64, CallState>,
    emitted_calls: usize,
}

#[derive(Clone, Copy)]
enum Terminal {
    Completed,
    Incomplete,
}

impl StreamTracker {
    /// 消费一帧 Responses 流事件；返回 `true` 表示收到终态（其后载荷忽略）。
    fn ingest(&mut self, value: &Value, deltas: &DeltaSink) -> Result<bool, ProviderError> {
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| error::malformed("OpenAI Responses 流事件缺少 type 字段".to_string()))?;
        match event_type {
            "response.output_text.delta" => {
                let text = required_str(value, "delta")?;
                if !text.is_empty() {
                    send_delta(
                        deltas,
                        ModelDelta::Text {
                            text: text.to_string(),
                        },
                    )?;
                }
                Ok(false)
            }
            "response.function_call_arguments.delta" => {
                let index = output_index(value)?;
                let fragment = required_str(value, "delta")?;
                if fragment.is_empty() {
                    return Ok(false);
                }
                let state = self.calls.entry(index).or_default();
                state.args.push_str(fragment);
                if state.started {
                    // started ⇒ call_id 已知（added 或补收口时写入）。
                    let tool_call_id = state.tool_call_id.clone().ok_or_else(|| {
                        error::malformed(format!("output_index {index} 缺少 call_id"))
                    })?;
                    send_delta(
                        deltas,
                        ModelDelta::ToolInputDelta {
                            tool_call_id,
                            delta: fragment.to_string(),
                        },
                    )?;
                } else {
                    // call_id 还没到（added 晚于 delta）：先攒着，start 后一次性补发。
                    state.unsent.push_str(fragment);
                }
                Ok(false)
            }
            "response.output_item.added" => {
                let item = value
                    .get("item")
                    .ok_or_else(|| error::malformed("output_item.added 缺少 item".to_string()))?;
                if item_type(item)? != "function_call" {
                    // message / reasoning 等项的 added 安静忽略（C2 §4.2.1）。
                    return Ok(false);
                }
                let index = output_index(value)?;
                let call_id = required_str(item, "call_id")?.to_string();
                let tool_name = required_str(item, "name")?.to_string();
                let state = self.calls.entry(index).or_default();
                if state.started {
                    return Err(error::malformed(format!(
                        "function_call 项（output_index {index}）重复 added"
                    )));
                }
                state.tool_call_id = Some(call_id.clone());
                state.tool_name = Some(tool_name.clone());
                send_delta(
                    deltas,
                    ModelDelta::ToolInputStart {
                        tool_call_id: call_id.clone(),
                        tool_name,
                    },
                )?;
                state.started = true;
                flush_unsent(&call_id, state, deltas)?;
                Ok(false)
            }
            "response.output_item.done" => {
                let item = value
                    .get("item")
                    .ok_or_else(|| error::malformed("output_item.done 缺少 item".to_string()))?;
                if item_type(item)? != "function_call" {
                    return Ok(false);
                }
                let index = output_index(value)?;
                let Some(mut state) = self.calls.remove(&index) else {
                    // added 被跳过（不合规但可恢复）：按 done 项补建状态收口。
                    return self.close_with_item(item, deltas);
                };
                // 已有状态优先用自己发出去的 call_id/name（与已发 delta 的 id 一致）；
                // added 缺失的防御路径才回退读 done 项。
                let call_id = match state.tool_call_id.clone() {
                    Some(call_id) => call_id,
                    None => required_str(item, "call_id")?.to_string(),
                };
                let tool_name = match state.tool_name.clone() {
                    Some(tool_name) => tool_name,
                    None => required_str(item, "name")?.to_string(),
                };
                state.tool_call_id = Some(call_id.clone());
                state.tool_name = Some(tool_name.clone());
                if !state.started {
                    send_delta(
                        deltas,
                        ModelDelta::ToolInputStart {
                            tool_call_id: call_id.clone(),
                            tool_name: tool_name.clone(),
                        },
                    )?;
                    state.started = true;
                    flush_unsent(&call_id, &mut state, deltas)?;
                }
                finish_call(&call_id, &tool_name, &state.args, item, deltas)?;
                self.emitted_calls += 1;
                Ok(false)
            }
            "response.completed" => {
                self.take_usage(value);
                self.close_open_calls(deltas)?;
                self.terminal = Some(Terminal::Completed);
                Ok(true)
            }
            "response.incomplete" => {
                self.take_usage(value);
                self.close_open_calls(deltas)?;
                self.terminal = Some(Terminal::Incomplete);
                Ok(true)
            }
            "response.failed" => Err(failed_error(value)),
            "error" => Err(in_stream_error(value)),
            // 其余事件（response.created / response.in_progress、message / reasoning 项、
            // output_text.done、function_call_arguments.done 等）安静忽略——公开流
            // 必发这些事件，按未知类型报错会让每条真实流都失败（C2 §4.2.1）。
            _ => Ok(false),
        }
    }

    /// 从 `response.completed` / `response.incomplete` 的 `response` 取 usage。
    fn take_usage(&mut self, value: &Value) {
        if let Some(usage) = value
            .get("response")
            .and_then(|response| response.get("usage"))
            .filter(|usage| !usage.is_null())
        {
            merge_usage(&mut self.usage, usage);
        }
    }

    /// 终态到达时仍未 `output_item.done` 的 function_call 按已累积入参补收口
    /// （与 Chat Completions 的 `close_all` 同口径；入参不可解析同样显式报错）。
    fn close_open_calls(&mut self, deltas: &DeltaSink) -> Result<(), ProviderError> {
        let indexes: Vec<u64> = self.calls.keys().copied().collect();
        for index in indexes {
            let Some(mut state) = self.calls.remove(&index) else {
                continue;
            };
            let (Some(call_id), Some(tool_name)) =
                (state.tool_call_id.clone(), state.tool_name.clone())
            else {
                return Err(error::malformed(format!(
                    "function_call（output_index {index}）在终态前缺少 call_id/name"
                )));
            };
            if !state.started {
                send_delta(
                    deltas,
                    ModelDelta::ToolInputStart {
                        tool_call_id: call_id.clone(),
                        tool_name: tool_name.clone(),
                    },
                )?;
                state.started = true;
                flush_unsent(&call_id, &mut state, deltas)?;
            }
            finish_call(&call_id, &tool_name, &state.args, &Value::Null, deltas)?;
            self.emitted_calls += 1;
        }
        Ok(())
    }

    /// `output_item.done` 的补收口路径（没有打开状态时直接用 done 项收口）。
    fn close_with_item(&mut self, item: &Value, deltas: &DeltaSink) -> Result<bool, ProviderError> {
        let call_id = required_str(item, "call_id")?.to_string();
        let tool_name = required_str(item, "name")?.to_string();
        send_delta(
            deltas,
            ModelDelta::ToolInputStart {
                tool_call_id: call_id.clone(),
                tool_name: tool_name.clone(),
            },
        )?;
        finish_call(&call_id, &tool_name, "", item, deltas)?;
        self.emitted_calls += 1;
        Ok(false)
    }

    fn into_response(self) -> ModelResponse {
        // 会话层只区分 stop / tool-calls（C2 §4.2.1）：
        // - completed：按已产出工具调用推断（公开流没有独立 stop_reason 字段）；
        // - incomplete（任何 reason，含 max_output_tokens）：一律 stop——登记的简化，
        //   会话层按 delta 决定工具批次，stop_reason 只是终态标注；
        // - 终态缺失（None）在 stream_once 已判为截断流错误，这里是防御。
        let stop_reason = match self.terminal {
            Some(Terminal::Incomplete) => "stop",
            Some(Terminal::Completed) | None if self.emitted_calls > 0 => "tool-calls",
            Some(Terminal::Completed) | None => "stop",
        };
        ModelResponse {
            usage: self.usage,
            stop_reason,
        }
    }
}

/// 项类型：`item.type` 必须是字符串（契约违规显式报错）。
fn item_type(item: &Value) -> Result<&str, ProviderError> {
    item.get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| error::malformed("output item 缺少 type 字段".to_string()))
}

/// 收口一条工具调用：`ToolInputEnd` → 解析入参 → `ToolCall`。
///
/// 入参来源：已累积分片非空时用它（与已转发的 `ToolInputDelta` 逐字一致）；分片为
/// 空（服务端只在 done 项给全量 `arguments`）时回退 `item.arguments`（`Value::Null`
/// 表示没有 done 项，即终态补收口）。空串/纯空白按无参工具 `{}`；其余必须可解析成
/// JSON，失败显式报错——静默丢调用会让下一轮上下文缺工具结果。
fn finish_call(
    tool_call_id: &str,
    tool_name: &str,
    accumulated: &str,
    item: &Value,
    deltas: &DeltaSink,
) -> Result<(), ProviderError> {
    send_delta(
        deltas,
        ModelDelta::ToolInputEnd {
            tool_call_id: tool_call_id.to_string(),
        },
    )?;
    let fallback = item.get("arguments").and_then(Value::as_str).unwrap_or("");
    let arguments = if accumulated.trim().is_empty() {
        fallback
    } else {
        accumulated
    };
    let input = parse_tool_input(tool_call_id, tool_name, arguments)?;
    send_delta(
        deltas,
        ModelDelta::ToolCall {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            input,
        },
    )?;
    Ok(())
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
    let fragment = std::mem::take(&mut state.unsent);
    send_delta(
        deltas,
        ModelDelta::ToolInputDelta {
            tool_call_id: tool_call_id.to_string(),
            delta: fragment,
        },
    )
}

/// 入参解析：空串/纯空白按 `{}`；其余必须可解析成 JSON，失败显式报错。
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
