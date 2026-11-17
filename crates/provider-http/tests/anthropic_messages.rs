//! Anthropic Messages 适配的流式契约测试（C2 · Task 4）：
//! 本地 `TcpListener` 回放固定 SSE 字节（message_start、text/thinking/tool_use 块、
//! 分片 input_json_delta、message_delta、message_stop、error），断言 `ModelDelta` 序列、
//! 终态 `ModelResponse`、请求路径/头与请求体映射、错误显式性。
//!
//! 不用 sleep 作正确性证明：server 在写响应前先写捕获区，client 的 future 结束时
//! 捕获必然可见（happens-before）；`timeout` 只做失败上限，防止回归挂死测试进程。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::provider::DeltaSink;
use agent_core::{
    ModelDelta, ModelMessage, ModelProvider, ModelRequest, ModelResponse, ModelToolCall,
    ModelToolDefinition, ModelUsage, ProviderError,
};
use provider_http::{
    build_messages_request, AnthropicMessagesConfig, AnthropicMessagesProvider,
    DEFAULT_ANTHROPIC_VERSION, DEFAULT_MAX_OUTPUT_TOKENS,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

// ---------- 本地 SSE server ----------

/// 捕获到的一次 HTTP 请求（头名字统一小写）。
#[derive(Debug)]
struct CapturedRequest {
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

impl CapturedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name == name)
            .map(|(_, value)| value.as_str())
    }

    /// 同名头的行数（默认头被 extra_headers 覆盖时必须恰好 1，不允许重复行）。
    fn header_count(&self, name: &str) -> usize {
        self.headers
            .iter()
            .filter(|(header_name, _)| header_name == name)
            .count()
    }
}

/// 起一个一次性 loopback server：读一个请求存进捕获区，回写固定字节后关连接。
async fn spawn_server(
    status_line: &str,
    content_type: &str,
    chunks: Vec<Vec<u8>>,
) -> (std::net::SocketAddr, Arc<Mutex<Option<CapturedRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback bind");
    let address = listener.local_addr().expect("local addr");
    let captured: Arc<Mutex<Option<CapturedRequest>>> = Arc::new(Mutex::new(None));
    let server_captured = Arc::clone(&captured);
    let status_line = status_line.to_string();
    let content_type = content_type.to_string();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept one request");
        let request = read_http_request(&mut stream).await;
        // 先捕获再写响应：client 拿到响应时捕获必然已就位，断言不需要轮询。
        *server_captured.lock().expect("capture lock") = Some(request);
        let mut response =
            format!("{status_line}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n")
                .into_bytes();
        response.extend(chunks.iter().flatten());
        stream.write_all(&response).await.expect("write response");
        let _ = stream.shutdown().await;
    });
    (address, captured)
}

/// 读完一个 HTTP 请求（头按 `\r\n\r\n` 定界，body 按 `content-length` 定界）。
async fn read_http_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 2048];
    let header_end = loop {
        if let Some(position) = find_subsequence(&buffer, b"\r\n\r\n") {
            break position;
        }
        let read = stream.read(&mut chunk).await.expect("read request head");
        assert!(read > 0, "连接在请求头结束前被对端关闭");
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8(buffer[..header_end].to_vec()).expect("请求头是 ASCII");
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let content_length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = stream.read(&mut chunk).await.expect("read request body");
        assert!(read > 0, "连接在请求体读完前被对端关闭");
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = serde_json::from_slice(&buffer[body_start..body_start + content_length])
        .expect("请求体是 JSON");
    CapturedRequest {
        path,
        headers,
        body,
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ---------- 测试夹具 ----------

/// 带 `event:` 行的事件（Anthropic 线上形态；解码器忽略 field，载荷的 type 是事实）。
fn event_sse(event: &str, payload: &str) -> Vec<u8> {
    format!("event: {event}\ndata: {payload}\n\n").into_bytes()
}

fn test_config(base_url: String) -> AnthropicMessagesConfig {
    AnthropicMessagesConfig {
        base_url,
        api_key: "sk-ant-test".to_string(),
        model: "claude-test".to_string(),
        max_output_tokens: None,
        extra_headers: Vec::new(),
        anthropic_version: None,
    }
}

fn simple_request(messages: Vec<ModelMessage>, tools: Vec<ModelToolDefinition>) -> ModelRequest {
    ModelRequest { messages, tools }
}

/// 失败上限（防回归挂死），不是正确性证明的一部分。
async fn run_stream(
    provider: &AnthropicMessagesProvider,
    request: ModelRequest,
    deltas: DeltaSink,
) -> Result<ModelResponse, ProviderError> {
    tokio::time::timeout(Duration::from_secs(5), provider.stream(request, deltas))
        .await
        .expect("provider 流在 5s 内完成")
}

/// future 结束后一次性排空 delta 通道（provider 契约：所有 delta 在返回前送达）。
fn collect_deltas(rx: &mut mpsc::UnboundedReceiver<ModelDelta>) -> Vec<ModelDelta> {
    let mut deltas = Vec::new();
    while let Ok(delta) = rx.try_recv() {
        deltas.push(delta);
    }
    deltas
}

// ---------- 请求体映射 ----------

#[test]
fn request_body_maps_system_messages_tools_and_token_limit() {
    let request = ModelRequest {
        messages: vec![
            ModelMessage::System("你是编码助手".to_string()),
            ModelMessage::User("读文件".to_string()),
            ModelMessage::Assistant {
                text: Some("我来读".to_string()),
                tool_calls: vec![ModelToolCall {
                    tool_call_id: "toolu_1".to_string(),
                    tool_name: "Read".to_string(),
                    input: json!({ "path": "a.txt" }),
                }],
            },
            ModelMessage::ToolResult {
                tool_call_id: "toolu_1".to_string(),
                content: "文件内容".to_string(),
            },
        ],
        tools: vec![
            ModelToolDefinition {
                name: "Read".to_string(),
                description: None,
                input_schema: json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                }),
            },
            ModelToolDefinition {
                name: "Bash".to_string(),
                description: Some("运行命令".to_string()),
                input_schema: json!({ "type": "object" }),
            },
        ],
    };
    let config = AnthropicMessagesConfig {
        base_url: "http://127.0.0.1:9".to_string(),
        api_key: "sk-ant-test".to_string(),
        model: "claude-test".to_string(),
        max_output_tokens: Some(4096),
        extra_headers: Vec::new(),
        anthropic_version: None,
    };

    assert_eq!(
        build_messages_request(&request, &config),
        json!({
            "model": "claude-test",
            "system": [{ "type": "text", "text": "你是编码助手" }],
            "messages": [
                {
                    "role": "user",
                    "content": [{ "type": "text", "text": "读文件" }],
                },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "我来读" },
                        {
                            "type": "tool_use",
                            "id": "toolu_1",
                            "name": "Read",
                            "input": { "path": "a.txt" },
                        },
                    ],
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": "文件内容",
                    }],
                },
            ],
            "tools": [
                {
                    "name": "Read",
                    "input_schema": {
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                    },
                },
                {
                    "name": "Bash",
                    "description": "运行命令",
                    "input_schema": { "type": "object" },
                },
            ],
            "max_tokens": 4096,
            "stream": true,
        })
    );
}

/// 多工具轮次：连续 tool_result 必须合并进**一条** user 消息（公开 API 要求同一
/// assistant 轮次的结果住在一条 user 消息里，逐条拆开会 400）；块序 = 结果原序。
#[test]
fn consecutive_tool_results_group_into_one_user_message() {
    let request = ModelRequest {
        messages: vec![
            ModelMessage::User("读两个文件".to_string()),
            ModelMessage::Assistant {
                text: None,
                tool_calls: vec![
                    ModelToolCall {
                        tool_call_id: "toolu_a".to_string(),
                        tool_name: "Read".to_string(),
                        input: json!({ "path": "a.txt" }),
                    },
                    ModelToolCall {
                        tool_call_id: "toolu_b".to_string(),
                        tool_name: "Read".to_string(),
                        input: json!({ "path": "b.txt" }),
                    },
                ],
            },
            ModelMessage::ToolResult {
                tool_call_id: "toolu_a".to_string(),
                content: "A".to_string(),
            },
            ModelMessage::ToolResult {
                tool_call_id: "toolu_b".to_string(),
                content: "B".to_string(),
            },
            // 相邻 user 文本消息保持独立，不与 tool_result 消息合并。
            ModelMessage::User("继续".to_string()),
        ],
        tools: Vec::new(),
    };
    let config = test_config("http://127.0.0.1:9".to_string());
    let body = build_messages_request(&request, &config);

    assert_eq!(
        body["messages"],
        json!([
            { "role": "user", "content": [{ "type": "text", "text": "读两个文件" }] },
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_a",
                        "name": "Read",
                        "input": { "path": "a.txt" },
                    },
                    {
                        "type": "tool_use",
                        "id": "toolu_b",
                        "name": "Read",
                        "input": { "path": "b.txt" },
                    },
                ],
            },
            {
                "role": "user",
                "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_a", "content": "A" },
                    { "type": "tool_result", "tool_use_id": "toolu_b", "content": "B" },
                ],
            },
            { "role": "user", "content": [{ "type": "text", "text": "继续" }] },
        ])
    );
    // 恰好一条带 tool_result 的 user 消息（多工具轮次不逐条拆开）。
    let tool_result_messages = body["messages"]
        .as_array()
        .expect("messages 是数组")
        .iter()
        .filter(|message| message["content"][0]["type"] == json!("tool_result"))
        .count();
    assert_eq!(tool_result_messages, 1);
}

#[test]
fn missing_max_output_tokens_uses_documented_fallback_and_omits_empty_tools() {
    let request = ModelRequest {
        messages: vec![ModelMessage::User("hi".to_string())],
        tools: Vec::new(),
    };
    let config = test_config("http://127.0.0.1:9".to_string());
    let body = build_messages_request(&request, &config);
    assert_eq!(body["max_tokens"], json!(DEFAULT_MAX_OUTPUT_TOKENS));
    assert_eq!(
        body["max_tokens"],
        json!(8192),
        "文档化回退值来自 T0 观察（agent-c2-contract §1.4），改动必须同步 spec"
    );
    assert_eq!(body["stream"], json!(true));
    assert!(body.get("tools").is_none(), "没有工具声明时不发 tools");
    assert!(
        body.get("system").is_none(),
        "没有 System 消息时不发 system"
    );
}

// ---------- 流式应答 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_stream_emits_deltas_usage_and_stop_response() {
    let chunks = vec![
        event_sse(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-test","stop_reason":null,"usage":{"input_tokens":12,"output_tokens":1,"cache_read_input_tokens":3,"cache_creation_input_tokens":5}}}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ),
        // ping 是公开的心跳事件：必须安静通过。
        event_sse("ping", r#"{"type":"ping"}"#),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}"#,
        ),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"，世界"}}"#,
        ),
        event_sse(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ),
        event_sse(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":7}}"#,
        ),
        event_sse("message_stop", r#"{"type":"message_stop"}"#),
    ];
    let (address, captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let response = run_stream(
        &provider,
        simple_request(
            vec![
                ModelMessage::System("你是编码助手".to_string()),
                ModelMessage::User("打个招呼".to_string()),
            ],
            Vec::new(),
        ),
        tx,
    )
    .await
    .expect("流式应答成功");

    assert_eq!(
        collect_deltas(&mut rx),
        vec![
            ModelDelta::Text {
                text: "你好".to_string()
            },
            ModelDelta::Text {
                text: "，世界".to_string()
            },
        ]
    );
    assert_eq!(
        response,
        ModelResponse {
            usage: ModelUsage {
                input_tokens: 12,
                output_tokens: 7,
                cache_read_tokens: 3,
                cache_write_tokens: 5,
                reasoning_tokens: 0,
            },
            stop_reason: "stop",
        }
    );

    let request = captured
        .lock()
        .expect("capture lock")
        .take()
        .expect("server 已捕获请求");
    assert_eq!(request.path, "/v1/messages");
    assert_eq!(request.header_count("content-type"), 1);
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(request.header_count("x-api-key"), 1);
    assert_eq!(request.header("x-api-key"), Some("sk-ant-test"));
    assert_eq!(
        request.header_count("anthropic-version"),
        1,
        "默认版本头恰好一条，不允许重复行"
    );
    assert_eq!(
        request.header("anthropic-version"),
        Some(DEFAULT_ANTHROPIC_VERSION)
    );
    assert_eq!(
        request.header("authorization"),
        None,
        "Anthropic 默认用 x-api-key，不发 Bearer"
    );
    assert_eq!(request.body["model"], json!("claude-test"));
    assert_eq!(request.body["stream"], json!(true));
    assert_eq!(request.body["max_tokens"], json!(DEFAULT_MAX_OUTPUT_TOKENS));
    assert_eq!(
        request.body["system"],
        json!([{ "type": "text", "text": "你是编码助手" }])
    );
    assert_eq!(
        request.body["messages"],
        json!([{ "role": "user", "content": [{ "type": "text", "text": "打个招呼" }] }])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thinking_blocks_are_consumed_but_not_forwarded() {
    let chunks = vec![
        event_sse(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_2","type":"message","role":"assistant","content":[],"usage":{"input_tokens":9,"output_tokens":1}}}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        ),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"先想一想"}}"#,
        ),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-abc"}}"#,
        ),
        event_sse(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        ),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"答案"}}"#,
        ),
        event_sse(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":1}"#,
        ),
        event_sse(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":6}}"#,
        ),
        event_sse("message_stop", r#"{"type":"message_stop"}"#),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let response = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect("thinking 块只消费不转发，流仍成功");

    // 已登记：ModelDelta 没有 reasoning 变体，thinking/signature 不产出 delta。
    assert_eq!(
        collect_deltas(&mut rx),
        vec![ModelDelta::Text {
            text: "答案".to_string()
        }]
    );
    assert_eq!(response.usage.input_tokens, 9);
    assert_eq!(response.usage.output_tokens, 6);
    assert_eq!(response.usage.reasoning_tokens, 0);
    assert_eq!(response.stop_reason, "stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fragmented_tool_arguments_accumulate_into_ordered_deltas() {
    let chunks = vec![
        event_sse(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_3","type":"message","role":"assistant","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_a","name":"Read","input":{}}}"#,
        ),
        // 入参拆成两个分片（分片边界刻意落在 JSON 字符串中间）。
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\": \"src/ma"}}"#,
        ),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"in.rs\"}"}}"#,
        ),
        event_sse(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_b","name":"Bash","input":{}}}"#,
        ),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"ls\"}"}}"#,
        ),
        event_sse(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":1}"#,
        ),
        event_sse(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":20}}"#,
        ),
        event_sse("message_stop", r#"{"type":"message_stop"}"#),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let response = run_stream(
        &provider,
        simple_request(
            vec![ModelMessage::User("读文件并列目录".to_string())],
            vec![ModelToolDefinition {
                name: "Read".to_string(),
                description: None,
                input_schema: json!({ "type": "object" }),
            }],
        ),
        tx,
    )
    .await
    .expect("流式应答成功");

    assert_eq!(
        collect_deltas(&mut rx),
        vec![
            ModelDelta::ToolInputStart {
                tool_call_id: "toolu_a".to_string(),
                tool_name: "Read".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "toolu_a".to_string(),
                delta: "{\"path\": \"src/ma".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "toolu_a".to_string(),
                delta: "in.rs\"}".to_string()
            },
            ModelDelta::ToolInputEnd {
                tool_call_id: "toolu_a".to_string()
            },
            ModelDelta::ToolCall {
                tool_call_id: "toolu_a".to_string(),
                tool_name: "Read".to_string(),
                input: json!({ "path": "src/main.rs" }),
            },
            ModelDelta::ToolInputStart {
                tool_call_id: "toolu_b".to_string(),
                tool_name: "Bash".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "toolu_b".to_string(),
                delta: "{\"command\":\"ls\"}".to_string()
            },
            ModelDelta::ToolInputEnd {
                tool_call_id: "toolu_b".to_string()
            },
            ModelDelta::ToolCall {
                tool_call_id: "toolu_b".to_string(),
                tool_name: "Bash".to_string(),
                input: json!({ "command": "ls" }),
            },
        ]
    );
    assert_eq!(
        response.usage,
        ModelUsage {
            input_tokens: 10,
            output_tokens: 20,
            ..ModelUsage::default()
        }
    );
    assert_eq!(response.stop_reason, "tool-calls");
}

// ---------- 错误映射 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_error_status_maps_anthropic_error_type_and_message() {
    let body = br#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#.to_vec();
    let (address, _captured) =
        spawn_server("HTTP/1.1 401 Unauthorized", "application/json", vec![body]).await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, _rx) = mpsc::unbounded_channel();
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("401 必须显式报错");
    assert!(error.message.contains("401"), "{}", error.message);
    assert!(
        error.message.contains("authentication_error"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("invalid x-api-key"),
        "{}",
        error.message
    );

    // 5xx 非 JSON 正文：回退原文截断，仍可诊断。
    let (address, _captured) = spawn_server(
        "HTTP/1.1 500 Internal Server Error",
        "text/plain",
        vec![b"upstream boom".to_vec()],
    )
    .await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");
    let (tx, _rx) = mpsc::unbounded_channel();
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("500 必须显式报错");
    assert!(error.message.contains("500"), "{}", error.message);
    assert!(error.message.contains("upstream boom"), "{}", error.message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_stream_error_event_maps_to_explicit_error() {
    let chunks = vec![
        event_sse(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_err","type":"message","role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ),
        event_sse(
            "error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, _rx) = mpsc::unbounded_channel();
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("流内 error 事件必须显式报错");
    assert!(
        error.message.contains("流内 error 事件"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("overloaded_error"),
        "{}",
        error.message
    );
    assert!(error.message.contains("Overloaded"), "{}", error.message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_json_payload_preserves_prior_deltas_and_reports_explicitly() {
    let chunks = vec![
        event_sse(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_4","type":"message","role":"assistant","content":[],"usage":{"input_tokens":2,"output_tokens":1}}}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"部分文本"}}"#,
        ),
        b"data: {oops\n\n".to_vec(),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("非法 JSON 必须显式报错");
    assert!(error.message.contains("不是合法 JSON"), "{}", error.message);
    assert!(error.message.contains("{oops"), "{}", error.message);
    // 错误前的 delta 已经送出（流式语义）；turn.failed 归会话层。
    assert_eq!(
        collect_deltas(&mut rx),
        vec![ModelDelta::Text {
            text: "部分文本".to_string()
        }]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn truncated_stream_without_message_stop_is_an_error() {
    let chunks = vec![
        event_sse(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_5","type":"message","role":"assistant","content":[],"usage":{"input_tokens":2,"output_tokens":1}}}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"被截断"}}"#,
        ),
        event_sse(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ),
        event_sse(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
        ),
        // 没有 message_stop：连接直接关闭。
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("截断流不能伪装成成功终态");
    assert!(error.message.contains("message_stop"), "{}", error.message);
    assert_eq!(
        collect_deltas(&mut rx),
        vec![ModelDelta::Text {
            text: "被截断".to_string()
        }],
        "错误前的 delta 保留"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn message_stop_with_open_content_block_is_an_error() {
    let chunks = vec![
        event_sse(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_6","type":"message","role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ),
        event_sse("message_stop", r#"{"type":"message_stop"}"#),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, _rx) = mpsc::unbounded_channel();
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("未收口的 content block 必须显式报错");
    assert!(
        error.message.contains("content_block_stop"),
        "{}",
        error.message
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unparseable_tool_arguments_map_to_explicit_error() {
    let chunks = vec![
        event_sse(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_7","type":"message","role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_bad","name":"Read","input":{}}}"#,
        ),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"not-json{"}}"#,
        ),
        event_sse(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ),
        event_sse(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}"#,
        ),
        event_sse("message_stop", r#"{"type":"message_stop"}"#),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("工具入参不可解析必须显式报错，不静默丢调用");
    assert!(
        error.message.contains("入参不是合法 JSON"),
        "{}",
        error.message
    );
    assert!(error.message.contains("toolu_bad"), "{}", error.message);
    assert!(error.message.contains("not-json{"), "{}", error.message);
    // 已完成的 start/delta/end 保留（错误前的流式事实不回收）。
    assert_eq!(
        collect_deltas(&mut rx),
        vec![
            ModelDelta::ToolInputStart {
                tool_call_id: "toolu_bad".to_string(),
                tool_name: "Read".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "toolu_bad".to_string(),
                delta: "not-json{".to_string()
            },
            ModelDelta::ToolInputEnd {
                tool_call_id: "toolu_bad".to_string()
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_delta_sink_maps_to_explicit_error() {
    let chunks = vec![
        event_sse(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_8","type":"message","role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ),
        event_sse(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"文本"}}"#,
        ),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = AnthropicMessagesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, rx) = mpsc::unbounded_channel::<ModelDelta>();
    drop(rx); // 消费端先关：provider 必须显式报错，而不是静默丢 delta
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("sink 已关闭必须显式报错");
    assert!(
        error.message.contains("delta sink 已关闭"),
        "{}",
        error.message
    );
}

// ---------- 请求头 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extra_headers_replace_same_name_defaults_without_duplicates() {
    let chunks = vec![
        event_sse(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_9","type":"message","role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
        ),
        event_sse(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ),
        event_sse(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ),
        event_sse("message_stop", r#"{"type":"message_stop"}"#),
    ];
    let (address, captured) =
        spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks.clone()).await;
    let mut config = test_config(format!("http://{address}/"));
    config.anthropic_version = Some("2023-06-01".to_string());
    config.extra_headers = vec![
        ("X-Api-Key".to_string(), "gateway-key".to_string()),
        ("Anthropic-Version".to_string(), "2024-01-01".to_string()),
        (
            "Content-Type".to_string(),
            "application/json; charset=utf-8".to_string(),
        ),
        ("X-Test-Header".to_string(), "provider-http".to_string()),
    ];
    let provider = AnthropicMessagesProvider::new(config).expect("client build");

    let (tx, _rx) = mpsc::unbounded_channel();
    run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect("流式应答成功");

    let request = captured
        .lock()
        .expect("capture lock")
        .take()
        .expect("server 已捕获请求");
    assert_eq!(request.path, "/v1/messages", "base_url 末尾 / 去重后拼路径");
    assert_eq!(
        request.header_count("x-api-key"),
        1,
        "x-api-key 被覆盖时只发 extra_headers 一条"
    );
    assert_eq!(request.header("x-api-key"), Some("gateway-key"));
    assert_eq!(request.header_count("anthropic-version"), 1);
    assert_eq!(request.header("anthropic-version"), Some("2024-01-01"));
    assert_eq!(request.header_count("content-type"), 1);
    assert_eq!(
        request.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(request.header("x-test-header"), Some("provider-http"));

    // 未覆盖：配置的 anthropic_version 作为默认版本头发送。
    let (address, captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let mut config = test_config(format!("http://{address}"));
    config.anthropic_version = Some("2025-01-01".to_string());
    let provider = AnthropicMessagesProvider::new(config).expect("client build");
    let (tx, _rx) = mpsc::unbounded_channel();
    run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect("流式应答成功");
    let request = captured
        .lock()
        .expect("capture lock")
        .take()
        .expect("server 已捕获请求");
    assert_eq!(request.header_count("anthropic-version"), 1);
    assert_eq!(request.header("anthropic-version"), Some("2025-01-01"));
}
