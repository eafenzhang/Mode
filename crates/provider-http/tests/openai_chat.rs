//! OpenAI Chat Completions 适配的流式契约测试（Task 4 Step 3）：
//! 本地 `TcpListener` 回放固定 SSE 字节（text delta、按 index/id 累积的工具调用分片、
//! finish_reason、usage 尾帧），断言 `ModelDelta` 序列与终态 `ModelResponse`；
//! 请求面断言 URL 路径、鉴权头与请求体映射。
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
use provider_http::{build_chat_request, OpenAiChatConfig, OpenAiChatProvider};
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
}

/// 起一个一次性 loopback server：读一个请求存进捕获区，回写固定字节后关连接。
/// 返回监听地址与捕获区的共享句柄。
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

/// 一条标准 `data:` 事件（以空行收尾）。
fn sse(payload: &str) -> Vec<u8> {
    format!("data: {payload}\n\n").into_bytes()
}

fn test_config(base_url: String) -> OpenAiChatConfig {
    OpenAiChatConfig {
        base_url,
        api_key: "sk-test".to_string(),
        model: "glm-4.6".to_string(),
        max_output_tokens: None,
        extra_headers: Vec::new(),
    }
}

fn simple_request(messages: Vec<ModelMessage>, tools: Vec<ModelToolDefinition>) -> ModelRequest {
    ModelRequest { messages, tools }
}

/// 失败上限（防回归挂死），不是正确性证明的一部分。
async fn run_stream(
    provider: &OpenAiChatProvider,
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

// ---------- 流式应答 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_stream_emits_deltas_usage_and_stop_response() {
    let chunks = vec![
        sse(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":"你好"},"finish_reason":null}]}"#,
        ),
        sse(r#"{"choices":[{"index":0,"delta":{"content":"，世界"},"finish_reason":null}]}"#),
        sse(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#),
        sse(
            r#"{"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18,"prompt_tokens_details":{"cached_tokens":4},"completion_tokens_details":{"reasoning_tokens":3}}}"#,
        ),
        b"data: [DONE]\n\n".to_vec(),
    ];
    let (address, captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider =
        OpenAiChatProvider::new(test_config(format!("http://{address}/v1"))).expect("client build");

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
                input_tokens: 11,
                output_tokens: 7,
                cache_read_tokens: 4,
                cache_write_tokens: 0,
                reasoning_tokens: 3,
            },
            stop_reason: "stop",
        }
    );

    let request = captured
        .lock()
        .expect("capture lock")
        .take()
        .expect("server 已捕获请求");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.header("authorization"), Some("Bearer sk-test"));
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(request.body["model"], json!("glm-4.6"));
    assert_eq!(request.body["stream"], json!(true));
    assert_eq!(request.body["stream_options"]["include_usage"], json!(true));
    assert!(
        request.body.get("max_tokens").is_none(),
        "未配置 max_output_tokens 时不发 max_tokens"
    );
    assert!(
        request.body.get("tools").is_none(),
        "没有工具声明时不发 tools"
    );
    assert_eq!(
        request.body["messages"][0],
        json!({"role": "system", "content": "你是编码助手"})
    );
    assert_eq!(
        request.body["messages"][1],
        json!({"role": "user", "content": "打个招呼"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_call_fragments_accumulate_by_index_into_ordered_deltas() {
    let chunks = vec![
        // 第一个分片：id + 名字 + 空入参串（线上常见形态；空串不产生 ToolInputDelta）。
        sse(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_0","type":"function","function":{"name":"read_file","arguments":""}}]},"finish_reason":null}]}"#,
        ),
        // 入参拆成两个分片（分片边界刻意落在 JSON 字符串中间）。
        sse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\": \"src/ma"}}]},"finish_reason":null}]}"#,
        ),
        sse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"in.rs\"}"}}]},"finish_reason":null}]}"#,
        ),
        // 新 index 开始：上一条调用收口（End + ToolCall），随后第二条调用开始。
        sse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_1","type":"function","function":{"name":"list_dir","arguments":"{\"path\":\".\"}"}}]},"finish_reason":null}]}"#,
        ),
        sse(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#),
        sse(r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":20}}"#),
        b"data: [DONE]\n\n".to_vec(),
    ];
    let (address, captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider =
        OpenAiChatProvider::new(test_config(format!("http://{address}/v1"))).expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let response = run_stream(
        &provider,
        simple_request(
            vec![ModelMessage::User("读文件并列目录".to_string())],
            vec![
                ModelToolDefinition {
                    name: "read_file".to_string(),
                    description: None,
                    input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                },
                ModelToolDefinition {
                    name: "list_dir".to_string(),
                    description: Some("列出目录".to_string()),
                    input_schema: json!({"type": "object"}),
                },
            ],
        ),
        tx,
    )
    .await
    .expect("流式应答成功");

    assert_eq!(
        collect_deltas(&mut rx),
        vec![
            ModelDelta::ToolInputStart {
                tool_call_id: "call_0".to_string(),
                tool_name: "read_file".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "call_0".to_string(),
                delta: "{\"path\": \"src/ma".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "call_0".to_string(),
                delta: "in.rs\"}".to_string()
            },
            ModelDelta::ToolInputEnd {
                tool_call_id: "call_0".to_string()
            },
            ModelDelta::ToolCall {
                tool_call_id: "call_0".to_string(),
                tool_name: "read_file".to_string(),
                input: json!({"path": "src/main.rs"}),
            },
            ModelDelta::ToolInputStart {
                tool_call_id: "call_1".to_string(),
                tool_name: "list_dir".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "call_1".to_string(),
                delta: "{\"path\":\".\"}".to_string()
            },
            ModelDelta::ToolInputEnd {
                tool_call_id: "call_1".to_string()
            },
            ModelDelta::ToolCall {
                tool_call_id: "call_1".to_string(),
                tool_name: "list_dir".to_string(),
                input: json!({"path": "."}),
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

    let request = captured
        .lock()
        .expect("capture lock")
        .take()
        .expect("server 已捕获请求");
    // tools 映射：description 只在提供时出现（不为内置工具编造描述）。
    assert_eq!(
        request.body["tools"],
        json!([
            {"type": "function", "function": {"name": "read_file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}}},
            {"type": "function", "function": {"name": "list_dir", "description": "列出目录", "parameters": {"type": "object"}}},
        ])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_ending_without_done_sentinel_still_yields_usage() {
    let chunks = vec![
        sse(r#"{"choices":[{"index":0,"delta":{"content":"收尾"},"finish_reason":null}]}"#),
        sse(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#),
        // usage 尾帧既没有空行收尾也没有 [DONE]：连接结束时必须补发，统计不能凭空丢失。
        br#"data: {"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":6}}"#.to_vec(),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider =
        OpenAiChatProvider::new(test_config(format!("http://{address}/v1"))).expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let response = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect("流式应答成功");

    assert_eq!(
        collect_deltas(&mut rx),
        vec![ModelDelta::Text {
            text: "收尾".to_string()
        }]
    );
    assert_eq!(response.usage.input_tokens, 5);
    assert_eq!(response.usage.output_tokens, 6);
    assert_eq!(response.stop_reason, "stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_finish_reason_defaults_by_emitted_deltas() {
    // 发过工具调用但没给 finish_reason：按已产出内容推断为 tool-calls。
    let tool_chunks = vec![
        sse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_x","type":"function","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":null}]}"#,
        ),
        b"data: [DONE]\n\n".to_vec(),
    ];
    let (address, _captured) =
        spawn_server("HTTP/1.1 200 OK", "text/event-stream", tool_chunks).await;
    let provider =
        OpenAiChatProvider::new(test_config(format!("http://{address}/v1"))).expect("client build");
    let (tx, _rx) = mpsc::unbounded_channel();
    let response = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect("流式应答成功");
    assert_eq!(response.stop_reason, "tool-calls");

    // 纯文本同样没给 finish_reason：推断为 stop。
    let text_chunks = vec![
        sse(r#"{"choices":[{"index":0,"delta":{"content":"好"},"finish_reason":null}]}"#),
        b"data: [DONE]\n\n".to_vec(),
    ];
    let (address, _captured) =
        spawn_server("HTTP/1.1 200 OK", "text/event-stream", text_chunks).await;
    let provider =
        OpenAiChatProvider::new(test_config(format!("http://{address}/v1"))).expect("client build");
    let (tx, _rx) = mpsc::unbounded_channel();
    let response = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect("流式应答成功");
    assert_eq!(response.stop_reason, "stop");
}

// ---------- 错误映射 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_error_status_maps_to_diagnosable_provider_error() {
    let body =
        br#"{"error":{"message":"invalid api key","type":"invalid_request_error"}}"#.to_vec();
    let (address, _captured) =
        spawn_server("HTTP/1.1 401 Unauthorized", "application/json", vec![body]).await;
    let provider =
        OpenAiChatProvider::new(test_config(format!("http://{address}/v1"))).expect("client build");

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
        error.message.contains("invalid api key"),
        "{}",
        error.message
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_json_data_line_maps_to_explicit_error() {
    let chunks = vec![
        sse(r#"{"choices":[{"index":0,"delta":{"content":"部分文本"},"finish_reason":null}]}"#),
        b"data: {oops\n\n".to_vec(),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider =
        OpenAiChatProvider::new(test_config(format!("http://{address}/v1"))).expect("client build");

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
    // 错误前的 delta 已经送出（流式语义）；后续兜底归会话层 turn.failed（Task 6）。
    assert_eq!(
        collect_deltas(&mut rx),
        vec![ModelDelta::Text {
            text: "部分文本".to_string()
        }]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unparseable_tool_arguments_map_to_explicit_error() {
    let chunks = vec![
        sse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_bad","type":"function","function":{"name":"read_file","arguments":"not-json{"}}]},"finish_reason":null}]}"#,
        ),
        sse(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#),
        b"data: [DONE]\n\n".to_vec(),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider =
        OpenAiChatProvider::new(test_config(format!("http://{address}/v1"))).expect("client build");

    let (tx, _rx) = mpsc::unbounded_channel();
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
    assert!(error.message.contains("call_bad"), "{}", error.message);
    assert!(error.message.contains("not-json{"), "{}", error.message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_delta_sink_maps_to_explicit_error() {
    let chunks = vec![
        sse(r#"{"choices":[{"index":0,"delta":{"content":"文本"},"finish_reason":null}]}"#),
        b"data: [DONE]\n\n".to_vec(),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider =
        OpenAiChatProvider::new(test_config(format!("http://{address}/v1"))).expect("client build");

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_stream_error_frame_maps_to_explicit_error() {
    // 兼容网关的形态：HTTP 200 但数据面以 error 帧收场。
    let chunks = vec![sse(
        r#"{"error":{"message":"context length exceeded","code":"context_length_exceeded"}}"#,
    )];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider =
        OpenAiChatProvider::new(test_config(format!("http://{address}/v1"))).expect("client build");

    let (tx, _rx) = mpsc::unbounded_channel();
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("流内 error 帧必须显式报错");
    assert!(error.message.contains("流内 error 帧"), "{}", error.message);
    assert!(
        error.message.contains("context length exceeded"),
        "{}",
        error.message
    );
}

// ---------- 请求面 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extra_headers_and_trailing_slash_base_url_reach_the_wire() {
    let chunks = vec![
        sse(r#"{"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}"#),
        b"data: [DONE]\n\n".to_vec(),
    ];
    let (address, captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let config = OpenAiChatConfig {
        base_url: format!("http://{address}/v1/"), // 末尾多余的 / 应被去掉再拼路径
        api_key: "sk-test".to_string(),
        model: "glm-4.6".to_string(),
        max_output_tokens: Some(1024),
        extra_headers: vec![("X-Test-Header".to_string(), "provider-http".to_string())],
    };
    let provider = OpenAiChatProvider::new(config).expect("client build");

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
    assert_eq!(
        request.path, "/v1/chat/completions",
        "base_url 末尾 / 去重后拼路径"
    );
    assert_eq!(
        request.header("x-test-header"),
        Some("provider-http"),
        "extra_headers 必须到达线上"
    );
    assert_eq!(
        request.body["max_tokens"],
        json!(1024),
        "配置了 max_output_tokens 时必须带 max_tokens"
    );
}

// ---------- 请求体映射（纯函数） ----------

#[test]
fn build_chat_request_maps_messages_tools_and_limits() {
    let config = OpenAiChatConfig {
        base_url: "https://api.example.com/v1".to_string(),
        api_key: "k".to_string(),
        model: "m".to_string(),
        max_output_tokens: Some(2048),
        extra_headers: Vec::new(),
    };
    let request = simple_request(
        vec![
            ModelMessage::System("系统提示".to_string()),
            ModelMessage::User("输入".to_string()),
            ModelMessage::Assistant {
                text: Some("好的".to_string()),
                tool_calls: vec![ModelToolCall {
                    tool_call_id: "call_1".to_string(),
                    tool_name: "read_file".to_string(),
                    input: json!({"path": "a"}),
                }],
            },
            ModelMessage::ToolResult {
                tool_call_id: "call_1".to_string(),
                content: "文件内容".to_string(),
            },
        ],
        vec![ModelToolDefinition {
            name: "read_file".to_string(),
            description: Some("读文件".to_string()),
            input_schema: json!({"type": "object"}),
        }],
    );

    assert_eq!(
        build_chat_request(&request, &config),
        json!({
            "model": "m",
            "messages": [
                {"role": "system", "content": "系统提示"},
                {"role": "user", "content": "输入"},
                {"role": "assistant", "content": "好的", "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"a\"}"}},
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "文件内容"},
            ],
            "tools": [
                {"type": "function", "function": {"name": "read_file", "description": "读文件", "parameters": {"type": "object"}}},
            ],
            "stream": true,
            "stream_options": {"include_usage": true},
            "max_tokens": 2048,
        })
    );
}

#[test]
fn build_chat_request_omits_empty_assistant_content_and_optional_keys() {
    let config = test_config("https://api.example.com/v1".to_string());
    let request = simple_request(
        vec![
            ModelMessage::Assistant {
                text: None,
                tool_calls: Vec::new(),
            },
            ModelMessage::Assistant {
                text: Some(String::new()),
                tool_calls: Vec::new(),
            },
        ],
        Vec::new(),
    );

    let body = build_chat_request(&request, &config);
    assert_eq!(
        body["messages"][0],
        json!({"role": "assistant"}),
        "None 文本省略 content 与 tool_calls"
    );
    assert_eq!(
        body["messages"][1],
        json!({"role": "assistant"}),
        "空文本同样省略 content"
    );
    assert!(body.get("tools").is_none(), "没有工具声明时不发 tools");
    assert!(
        body.get("max_tokens").is_none(),
        "未配置 max_output_tokens 时不发 max_tokens"
    );
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["stream_options"]["include_usage"], json!(true));
}
