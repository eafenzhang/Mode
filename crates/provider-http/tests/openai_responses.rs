//! OpenAI Responses 适配的流式契约测试（C2 · Task 5）：
//! 本地 `TcpListener` 回放固定 SSE 字节（created、text delta、function_call
//! added/delta/done、completed/incomplete/failed、error），断言请求体全量映射、
//! `ModelDelta` 序列、终态 `ModelResponse`、请求路径/头与错误显式性。
//!
//! 不用 sleep 作正确性证明：server 在写响应前先写捕获区，client 的 future 结束时
//! 捕获必然可见（happens-before）；`timeout` 只做失败上限，防止回归挂死测试进程。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::provider::DeltaSink;
use agent_core::{
    ModelDelta, ModelMessage, ModelProvider, ModelRequest, ModelResponse, ModelToolCall,
    ModelToolDefinition, ModelUsage, ProviderError, ProviderErrorKind,
};
use provider_http::{
    build_responses_request, OpenAiResponsesConfig, OpenAiResponsesProvider, INSTRUCTIONS_SEPARATOR,
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

/// 带 `event:` 行的事件（OpenAI 线上形态；解码器忽略 field，载荷的 type 是事实）。
fn event_sse(event: &str, payload: &str) -> Vec<u8> {
    format!("event: {event}\ndata: {payload}\n\n").into_bytes()
}

fn test_config(base_url: String) -> OpenAiResponsesConfig {
    OpenAiResponsesConfig {
        base_url,
        api_key: "sk-test".to_string(),
        model: "gpt-test".to_string(),
        max_output_tokens: None,
        extra_headers: Vec::new(),
        store: None,
    }
}

fn simple_request(messages: Vec<ModelMessage>, tools: Vec<ModelToolDefinition>) -> ModelRequest {
    ModelRequest { messages, tools }
}

/// 失败上限（防回归挂死），不是正确性证明的一部分。
async fn run_stream(
    provider: &OpenAiResponsesProvider,
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
fn request_body_maps_instructions_input_tools_and_token_limit() {
    let request = ModelRequest {
        messages: vec![
            ModelMessage::System("你是编码助手".to_string()),
            ModelMessage::User("读文件".to_string()),
            ModelMessage::Assistant {
                text: Some("我来读".to_string()),
                tool_calls: vec![ModelToolCall {
                    tool_call_id: "call_1".to_string(),
                    tool_name: "Read".to_string(),
                    input: json!({ "path": "a.txt" }),
                }],
            },
            ModelMessage::ToolResult {
                tool_call_id: "call_1".to_string(),
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
    let config = OpenAiResponsesConfig {
        base_url: "http://127.0.0.1:9".to_string(),
        api_key: "sk-test".to_string(),
        model: "gpt-test".to_string(),
        max_output_tokens: Some(4096),
        extra_headers: Vec::new(),
        store: None,
    };

    assert_eq!(
        build_responses_request(&request, &config),
        json!({
            "model": "gpt-test",
            "instructions": "你是编码助手",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "读文件" }],
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "我来读" }],
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "Read",
                    "arguments": "{\"path\":\"a.txt\"}",
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "文件内容",
                },
            ],
            "tools": [
                {
                    "type": "function",
                    "name": "Read",
                    "parameters": {
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                    },
                },
                {
                    "type": "function",
                    "name": "Bash",
                    "description": "运行命令",
                    "parameters": { "type": "object" },
                },
            ],
            "max_output_tokens": 4096,
            "stream": true,
        }),
        "全量请求体（无 store：配置 None 时省略键）"
    );
}

/// 多条 `System` 按登记分隔符拼接；`max_output_tokens`/`store`/`tools` 仅在配置
/// 给出时出现；防御形态 assistant（无文本无调用）不产出 input 项。
#[test]
fn optional_keys_follow_config_and_multi_system_joins_deterministically() {
    let request = ModelRequest {
        messages: vec![
            ModelMessage::System("第一段".to_string()),
            ModelMessage::System("第二段".to_string()),
            ModelMessage::User("hi".to_string()),
            ModelMessage::Assistant {
                text: None,
                tool_calls: Vec::new(),
            },
            ModelMessage::User("继续".to_string()),
        ],
        tools: Vec::new(),
    };
    let mut config = test_config("http://127.0.0.1:9".to_string());
    let body = build_responses_request(&request, &config);
    assert_eq!(
        body["instructions"],
        json!(format!("第一段{INSTRUCTIONS_SEPARATOR}第二段")),
        "多条 System 用登记的分隔符保序拼接"
    );
    assert_eq!(
        body["instructions"],
        json!("第一段\n\n第二段"),
        "分隔符改动必须同步 C2 §4.2.1"
    );
    assert!(body.get("max_output_tokens").is_none(), "None 时不发上限");
    assert!(body.get("store").is_none(), "None 时不发 store");
    assert!(body.get("tools").is_none(), "没有工具声明时不发 tools");
    assert_eq!(body["stream"], json!(true));
    assert_eq!(
        body["input"],
        json!([
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "hi" }] },
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "继续" }] },
        ]),
        "无文本无调用的 assistant 不产出 input 项"
    );

    config.store = Some(false);
    config.max_output_tokens = Some(1024);
    let body = build_responses_request(&request, &config);
    assert_eq!(body["store"], json!(false), "Some 时 store 进请求体");
    assert_eq!(body["max_output_tokens"], json!(1024));
}

/// 多工具轮次：`function_call` 与 `function_call_output` 各自独立成项（Responses
/// **不做** Anthropic 那种同轮合并），项序 = 历史序。
#[test]
fn multi_tool_round_keeps_call_and_output_items_in_history_order() {
    let request = ModelRequest {
        messages: vec![
            ModelMessage::User("读两个文件".to_string()),
            ModelMessage::Assistant {
                text: None,
                tool_calls: vec![
                    ModelToolCall {
                        tool_call_id: "call_a".to_string(),
                        tool_name: "Read".to_string(),
                        input: json!({ "path": "a.txt" }),
                    },
                    ModelToolCall {
                        tool_call_id: "call_b".to_string(),
                        tool_name: "Read".to_string(),
                        input: json!({ "path": "b.txt" }),
                    },
                ],
            },
            ModelMessage::ToolResult {
                tool_call_id: "call_a".to_string(),
                content: "A".to_string(),
            },
            ModelMessage::ToolResult {
                tool_call_id: "call_b".to_string(),
                content: "B".to_string(),
            },
            ModelMessage::User("继续".to_string()),
        ],
        tools: Vec::new(),
    };
    let body = build_responses_request(&request, &test_config("http://127.0.0.1:9".to_string()));
    assert_eq!(
        body["input"],
        json!([
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "读两个文件" }] },
            { "type": "function_call", "call_id": "call_a", "name": "Read", "arguments": "{\"path\":\"a.txt\"}" },
            { "type": "function_call", "call_id": "call_b", "name": "Read", "arguments": "{\"path\":\"b.txt\"}" },
            { "type": "function_call_output", "call_id": "call_a", "output": "A" },
            { "type": "function_call_output", "call_id": "call_b", "output": "B" },
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "继续" }] },
        ])
    );
}

// ---------- 流式应答 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_stream_emits_deltas_usage_and_stop_response() {
    let chunks = vec![
        // 公开流必发的非消费事件：必须安静通过。
        event_sse(
            "response.created",
            r#"{"type":"response.created","response":{"id":"resp_1","status":"in_progress"}}"#,
        ),
        event_sse(
            "response.output_item.added",
            r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"msg_1","type":"message","status":"in_progress","role":"assistant","content":[]}}"#,
        ),
        event_sse(
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"你好"}"#,
        ),
        event_sse(
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"，世界"}"#,
        ),
        event_sse(
            "response.output_item.done",
            r#"{"type":"response.output_item.done","output_index":0,"item":{"id":"msg_1","type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"你好，世界"}]}}"#,
        ),
        event_sse(
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"resp_1","object":"response","status":"completed","incomplete_details":null,"usage":{"input_tokens":12,"output_tokens":7,"total_tokens":19,"input_tokens_details":{"cached_tokens":3,"cache_write_tokens":0},"output_tokens_details":{"reasoning_tokens":2}}}}"#,
        ),
    ];
    let (address, captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}/")))
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
        ],
        "message 项的 added/done 与 response.created 不产出 delta"
    );
    assert_eq!(
        response,
        ModelResponse {
            usage: ModelUsage {
                input_tokens: 12,
                output_tokens: 7,
                cache_read_tokens: 3,
                cache_write_tokens: 0,
                reasoning_tokens: 2,
            },
            stop_reason: "stop",
        },
        "usage 按公开字段映射：cached→cache_read、reasoning_tokens→reasoning"
    );

    let request = captured
        .lock()
        .expect("capture lock")
        .take()
        .expect("server 已捕获请求");
    assert_eq!(request.path, "/responses", "base_url 尾 / 去重后拼路径");
    assert_eq!(request.header_count("content-type"), 1);
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(
        request.header_count("authorization"),
        1,
        "默认 Bearer 恰好一条，不允许重复行"
    );
    assert_eq!(request.header("authorization"), Some("Bearer sk-test"));
    assert_eq!(request.body["model"], json!("gpt-test"));
    assert_eq!(request.body["stream"], json!(true));
    assert_eq!(request.body["instructions"], json!("你是编码助手"));
    assert_eq!(
        request.body["input"],
        json!([
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "打个招呼" }] },
        ])
    );
    assert!(
        request.body.get("max_output_tokens").is_none(),
        "配置未给上限时不发字段"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fragmented_tool_arguments_accumulate_into_ordered_deltas() {
    let chunks = vec![
        event_sse(
            "response.output_item.added",
            r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_a","name":"Read","arguments":"","status":"in_progress"}}"#,
        ),
        // 入参拆成两个分片（分片边界刻意落在 JSON 字符串中间）。
        event_sse(
            "response.function_call_arguments.delta",
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"{\"path\": \"src/ma"}"#,
        ),
        event_sse(
            "response.function_call_arguments.delta",
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"in.rs\"}"}"#,
        ),
        event_sse(
            "response.output_item.done",
            r#"{"type":"response.output_item.done","output_index":0,"item":{"id":"fc_1","type":"function_call","call_id":"call_a","name":"Read","arguments":"{\"path\": \"src/main.rs\"}","status":"completed"}}"#,
        ),
        event_sse(
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":10,"output_tokens":20,"total_tokens":30,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}}"#,
        ),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let response = run_stream(
        &provider,
        simple_request(
            vec![ModelMessage::User("读文件".to_string())],
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
                tool_call_id: "call_a".to_string(),
                tool_name: "Read".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "call_a".to_string(),
                delta: "{\"path\": \"src/ma".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "call_a".to_string(),
                delta: "in.rs\"}".to_string()
            },
            ModelDelta::ToolInputEnd {
                tool_call_id: "call_a".to_string()
            },
            ModelDelta::ToolCall {
                tool_call_id: "call_a".to_string(),
                tool_name: "Read".to_string(),
                input: json!({ "path": "src/main.rs" }),
            },
        ],
        "Start → 分片 Delta… → End → ToolCall，参数按 done 项收口"
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

/// 入参累积必须按 `output_index` 分键：两个 function_call 交错发分片时，
/// 每个调用的分片只进自己的缓冲（字符串全局拼接会把两组参数搅在一起）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interleaved_tool_argument_deltas_are_scoped_by_output_index() {
    let chunks = vec![
        event_sse(
            "response.output_item.added",
            r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"fc_a","type":"function_call","call_id":"call_a","name":"Read","arguments":"","status":"in_progress"}}"#,
        ),
        event_sse(
            "response.output_item.added",
            r#"{"type":"response.output_item.added","output_index":1,"item":{"id":"fc_b","type":"function_call","call_id":"call_b","name":"Bash","arguments":"","status":"in_progress"}}"#,
        ),
        event_sse(
            "response.function_call_arguments.delta",
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_a","output_index":0,"delta":"{\"path\":\"a"}"#,
        ),
        event_sse(
            "response.function_call_arguments.delta",
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_b","output_index":1,"delta":"{\"command\":\"ls\"}"}"#,
        ),
        event_sse(
            "response.function_call_arguments.delta",
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_a","output_index":0,"delta":".txt\"}"}"#,
        ),
        event_sse(
            "response.output_item.done",
            r#"{"type":"response.output_item.done","output_index":0,"item":{"id":"fc_a","type":"function_call","call_id":"call_a","name":"Read","arguments":"{\"path\":\"a.txt\"}","status":"completed"}}"#,
        ),
        event_sse(
            "response.output_item.done",
            r#"{"type":"response.output_item.done","output_index":1,"item":{"id":"fc_b","type":"function_call","call_id":"call_b","name":"Bash","arguments":"{\"command\":\"ls\"}","status":"completed"}}"#,
        ),
        event_sse(
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":5,"output_tokens":9,"total_tokens":14,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}}"#,
        ),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let response = run_stream(
        &provider,
        simple_request(
            vec![ModelMessage::User("读文件并列目录".to_string())],
            Vec::new(),
        ),
        tx,
    )
    .await
    .expect("流式应答成功");

    assert_eq!(
        collect_deltas(&mut rx),
        vec![
            ModelDelta::ToolInputStart {
                tool_call_id: "call_a".to_string(),
                tool_name: "Read".to_string()
            },
            ModelDelta::ToolInputStart {
                tool_call_id: "call_b".to_string(),
                tool_name: "Bash".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "call_a".to_string(),
                delta: "{\"path\":\"a".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "call_b".to_string(),
                delta: "{\"command\":\"ls\"}".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "call_a".to_string(),
                delta: ".txt\"}".to_string()
            },
            ModelDelta::ToolInputEnd {
                tool_call_id: "call_a".to_string()
            },
            ModelDelta::ToolCall {
                tool_call_id: "call_a".to_string(),
                tool_name: "Read".to_string(),
                input: json!({ "path": "a.txt" }),
            },
            ModelDelta::ToolInputEnd {
                tool_call_id: "call_b".to_string()
            },
            ModelDelta::ToolCall {
                tool_call_id: "call_b".to_string(),
                tool_name: "Bash".to_string(),
                input: json!({ "command": "ls" }),
            },
        ],
        "分片按 output_index 各自归属，收口顺序 = done 顺序"
    );
    assert_eq!(response.stop_reason, "tool-calls");
}

/// `response.incomplete`（`max_output_tokens`）→ 显式终态且 stop 恒为 `"stop"`
/// （登记的简化，C2 §4.2.1），usage 照常取。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incomplete_max_output_tokens_maps_to_stop_with_usage() {
    let chunks = vec![
        event_sse(
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"被截断"}"#,
        ),
        event_sse(
            "response.incomplete",
            r#"{"type":"response.incomplete","response":{"id":"resp_1","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":8,"output_tokens":4,"total_tokens":12,"input_tokens_details":{"cached_tokens":1},"output_tokens_details":{"reasoning_tokens":3}}}}"#,
        ),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let response = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect("incomplete 是显式终态，不是错误");

    assert_eq!(
        collect_deltas(&mut rx),
        vec![ModelDelta::Text {
            text: "被截断".to_string()
        }],
        "错误/终态前的 delta 保留"
    );
    assert_eq!(response.stop_reason, "stop");
    assert_eq!(
        response.usage,
        ModelUsage {
            input_tokens: 8,
            output_tokens: 4,
            cache_read_tokens: 1,
            reasoning_tokens: 3,
            ..ModelUsage::default()
        }
    );
}

/// `response.failed` → 显式 `ProviderError`（带 `response.error.code/message`）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_response_maps_to_explicit_error() {
    let chunks = vec![
        event_sse(
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"部分文本"}"#,
        ),
        event_sse(
            "response.failed",
            r#"{"type":"response.failed","response":{"id":"resp_1","status":"failed","error":{"code":"server_error","message":"The model failed to generate a response."}}}"#,
        ),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("response.failed 必须显式报错");
    assert!(error.message.contains("server_error"), "{}", error.message);
    assert!(
        error.message.contains("The model failed"),
        "{}",
        error.message
    );
    assert_eq!(
        collect_deltas(&mut rx),
        vec![ModelDelta::Text {
            text: "部分文本".to_string()
        }],
        "失败前的 delta 保留"
    );
}

/// 流内 `error` 事件 → 显式 `ProviderError`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_stream_error_event_maps_to_explicit_error() {
    let chunks = vec![
        event_sse(
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"文本"}"#,
        ),
        event_sse(
            "error",
            r#"{"type":"error","code":"rate_limit_exceeded","message":"Slow down","param":null}"#,
        ),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
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
        error.message.contains("rate_limit_exceeded"),
        "{}",
        error.message
    );
    assert!(error.message.contains("Slow down"), "{}", error.message);
    assert_eq!(
        collect_deltas(&mut rx),
        vec![ModelDelta::Text {
            text: "文本".to_string()
        }]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_json_payload_preserves_prior_deltas_and_reports_explicitly() {
    let chunks = vec![
        event_sse(
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"部分文本"}"#,
        ),
        b"data: {oops\n\n".to_vec(),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
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
    assert_eq!(
        collect_deltas(&mut rx),
        vec![ModelDelta::Text {
            text: "部分文本".to_string()
        }],
        "错误前的 delta 已经送出（流式语义）"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn truncated_stream_without_terminal_event_is_an_error() {
    let chunks = vec![
        event_sse(
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"被截断"}"#,
        ),
        // 没有 response.completed/incomplete/failed：连接直接关闭。
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let error = run_stream(
        &provider,
        simple_request(vec![ModelMessage::User("hi".to_string())], Vec::new()),
        tx,
    )
    .await
    .expect_err("截断流不能伪装成成功终态");
    assert!(error.message.contains("终态事件"), "{}", error.message);
    assert_eq!(error.kind, ProviderErrorKind::StreamAbort);
    assert!(
        error.retryable,
        "流中断必须可重试（stream recovery 的入口）"
    );
    assert_eq!(
        collect_deltas(&mut rx),
        vec![ModelDelta::Text {
            text: "被截断".to_string()
        }],
        "错误前的 delta 保留"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unparseable_tool_arguments_map_to_explicit_error() {
    let chunks = vec![
        event_sse(
            "response.output_item.added",
            r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"fc_bad","type":"function_call","call_id":"call_bad","name":"Read","arguments":"","status":"in_progress"}}"#,
        ),
        event_sse(
            "response.function_call_arguments.delta",
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_bad","output_index":0,"delta":"not-json{"}"#,
        ),
        event_sse(
            "response.output_item.done",
            r#"{"type":"response.output_item.done","output_index":0,"item":{"id":"fc_bad","type":"function_call","call_id":"call_bad","name":"Read","arguments":"not-json{","status":"completed"}}"#,
        ),
        event_sse(
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}}"#,
        ),
    ];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
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
    assert!(error.message.contains("call_bad"), "{}", error.message);
    assert!(error.message.contains("not-json{"), "{}", error.message);
    assert_eq!(
        collect_deltas(&mut rx),
        vec![
            ModelDelta::ToolInputStart {
                tool_call_id: "call_bad".to_string(),
                tool_name: "Read".to_string()
            },
            ModelDelta::ToolInputDelta {
                tool_call_id: "call_bad".to_string(),
                delta: "not-json{".to_string()
            },
            ModelDelta::ToolInputEnd {
                tool_call_id: "call_bad".to_string()
            },
        ],
        "错误前的 start/delta/end 保留，ToolCall 不发出"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_delta_sink_maps_to_explicit_error() {
    let chunks = vec![event_sse(
        "response.output_text.delta",
        r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"文本"}"#,
    )];
    let (address, _captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
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

/// HTTP 错误状态：OpenAI 形态 `{"error":{"message":…}}` 提取消息，非 JSON 回退截断。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_error_status_maps_openai_error_message() {
    let body =
        br#"{"error":{"message":"Invalid API key","type":"invalid_request_error"}}"#.to_vec();
    let (address, _captured) =
        spawn_server("HTTP/1.1 401 Unauthorized", "application/json", vec![body]).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
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
        error.message.contains("Invalid API key"),
        "{}",
        error.message
    );
    assert_eq!(error.kind, ProviderErrorKind::Auth);
    assert_eq!(error.status, Some(401));
    assert!(!error.retryable, "401 必须是非 retryable");

    let (address, _captured) = spawn_server(
        "HTTP/1.1 500 Internal Server Error",
        "text/plain",
        vec![b"upstream boom".to_vec()],
    )
    .await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
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
    assert_eq!(error.kind, ProviderErrorKind::Server);
    assert_eq!(error.status, Some(500));
    assert!(error.retryable, "5xx 必须可重试");
}

// ---------- 请求头 ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extra_headers_replace_same_name_defaults_without_duplicates() {
    let chunks = vec![event_sse(
        "response.completed",
        r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}}"#,
    )];
    let (address, captured) =
        spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks.clone()).await;
    let mut config = test_config(format!("http://{address}/"));
    config.extra_headers = vec![
        (
            "Authorization".to_string(),
            "Bearer gateway-key".to_string(),
        ),
        (
            "Content-Type".to_string(),
            "application/json; charset=utf-8".to_string(),
        ),
        ("X-Test-Header".to_string(), "provider-http".to_string()),
    ];
    let provider = OpenAiResponsesProvider::new(config).expect("client build");

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
    assert_eq!(request.path, "/responses", "base_url 末尾 / 去重后拼路径");
    assert_eq!(
        request.header_count("authorization"),
        1,
        "authorization 被覆盖时只发 extra_headers 一条"
    );
    assert_eq!(request.header("authorization"), Some("Bearer gateway-key"));
    assert_eq!(request.header_count("content-type"), 1);
    assert_eq!(
        request.header("content-type"),
        Some("application/json; charset=utf-8")
    );
    assert_eq!(request.header("x-test-header"), Some("provider-http"));

    // 未覆盖：默认 Bearer 用配置的 api_key。
    let (address, captured) = spawn_server("HTTP/1.1 200 OK", "text/event-stream", chunks).await;
    let provider = OpenAiResponsesProvider::new(test_config(format!("http://{address}")))
        .expect("client build");
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
    assert_eq!(request.header_count("authorization"), 1);
    assert_eq!(request.header("authorization"), Some("Bearer sk-test"));
}

/// `Debug` 掩码纪律：key 恒为掩码，头值（可能是另一把凭据）只显示头名。
#[test]
fn config_debug_masks_api_key_and_header_values() {
    let config = OpenAiResponsesConfig {
        base_url: "https://mock.example/v1".to_string(),
        api_key: "super-secret-key".to_string(),
        model: "gpt-test".to_string(),
        max_output_tokens: Some(4096),
        extra_headers: vec![("Authorization".to_string(), "another-secret".to_string())],
        store: Some(false),
    };
    let rendered = format!("{config:?}");
    assert!(!rendered.contains("super-secret-key"), "{rendered}");
    assert!(!rendered.contains("another-secret"), "{rendered}");
    assert!(rendered.contains("gpt-test"), "{rendered}");
    assert!(rendered.contains("Authorization"), "{rendered}");
}
