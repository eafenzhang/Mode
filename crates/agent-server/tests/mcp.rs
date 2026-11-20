//! C4 T1：MCP 会话面的集成测试。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §1 与 `fixtures/c4/mcp-*.json`。
//! 覆盖：stdio 建连/工具目录/调用/子进程、两类 stdio 失败、http 404 诊断、
//! `mcp/list` 两模式、会话工具源的首 turn 惰性建连。
//!
//! stdio server 用 Node 脚本扮演（仓库与 CI 均带 Node）；缺失时跳过并打印原因。

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agent_core::session::{Session, ToolCallRequest};
use agent_core::PluginToolSource;
use agent_protocol::mcp::{
    McpProtocolEra, McpServerFailureKind, McpServerStatusKind, McpTransportKind,
};
use agent_protocol::session_create::{
    McpEntry, McpServer, McpServerHttp, McpServerStdio, McpTransportType,
};
use agent_server::{AgentServer, McpHub};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// 临时目录 + Node 可用性
// ---------------------------------------------------------------------------

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-agent-server-mcp-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 最小 stdio MCP server：initialize / tools/list / echo / fail / garbage 模式。
fn write_node_server(root: &Path, mode: &str) -> PathBuf {
    let path = root.join(format!("{mode}-mcp-server.mjs"));
    let source = match mode {
        "crash" => "process.exit(3);\n".to_string(),
        "garbage" => "console.log('this is not json');\nsetInterval(() => {}, 1000);\n".to_string(),
        _ => [
            "import { createInterface } from 'node:readline';",
            "const rl = createInterface({ input: process.stdin });",
            "const send = (value) => process.stdout.write(JSON.stringify(value) + '\\n');",
            "rl.on('line', (line) => {",
            "  let message; try { message = JSON.parse(line); } catch { return; }",
            "  if (message.id === undefined) return;",
            "  if (message.method === 'initialize') {",
            "    send({ jsonrpc: '2.0', id: message.id, result: { protocolVersion: '2024-11-05', capabilities: { tools: {} }, serverInfo: { name: 'probe', version: '1.0.0' } } });",
            "    return;",
            "  }",
            "  if (message.method === 'notifications/initialized') return;",
            "  if (message.method === 'tools/list') {",
            "    send({ jsonrpc: '2.0', id: message.id, result: { tools: [",
            "      { name: 'echo', description: 'Echo the given text back.', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'], additionalProperties: false } },",
            "      { name: 'fail', description: 'Always fails on purpose.', inputSchema: { type: 'object', properties: {}, additionalProperties: false } },",
            "      { name: 'slow', description: 'Replies after a delay.', inputSchema: { type: 'object', properties: { ms: { type: 'number' } }, additionalProperties: false } }",
            "    ] } });",
            "    return;",
            "  }",
            "  if (message.method === 'tools/call') {",
            "    const name = message.params && message.params.name;",
            "    if (name === 'fail') {",
            "      send({ jsonrpc: '2.0', id: message.id, result: { isError: true, content: [{ type: 'text', text: 'probe-mcp deliberate failure' }] } });",
            "      return;",
            "    }",
            "    const text = String((message.params && message.params.arguments && message.params.arguments.text) || '');",
            "    send({ jsonrpc: '2.0', id: message.id, result: { content: [{ type: 'text', text: 'echo:' + text }] } });",
            "    return;",
            "  }",
            "  send({ jsonrpc: '2.0', id: message.id, error: { code: -32601, message: 'method not found: ' + message.method } });",
            "});",
            "",
        ]
        .join("\n"),
    };
    fs::write(&path, source).expect("write node server");
    path
}

fn stdio_server(name: &str, script: &Path, timeout_ms: Option<u64>) -> McpServer {
    McpServer::Stdio(McpServerStdio {
        name: name.to_string(),
        command: "node".to_string(),
        args: vec![script.display().to_string()],
        env: Vec::<McpEntry>::new(),
        isolation: None,
        protocol_version: None,
        timeout_ms,
    })
}

// ---------------------------------------------------------------------------
// stdio：成功路径
// ---------------------------------------------------------------------------

#[test]
fn stdio_connect_lists_tools_calls_echo_and_lists_child_process() {
    if !node_available() {
        eprintln!("skip: node 不可用");
        return;
    }
    let root = TempRoot::new("happy");
    let script = write_node_server(root.path(), "happy");
    let hub = McpHub::new();
    let server = stdio_server("probe-mcp", &script, None);

    // create/status 阶段不建连：没有子进程。
    assert!(hub.child_processes().is_empty());

    let connection = hub.connect(&server);
    let status = connection.status();
    assert_eq!(status.status, McpServerStatusKind::Connected);
    assert_eq!(status.transport, McpTransportKind::Stdio);
    assert_eq!(status.tool_count, 3);
    assert_eq!(status.protocol_era, Some(McpProtocolEra::Legacy));
    assert!(status.error.is_none());
    assert!(status.failure_kind.is_none());

    let tools = connection.tools();
    let names: Vec<_> = tools
        .iter()
        .map(|tool| tool.qualified_name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "mcp__probe-mcp__echo",
            "mcp__probe-mcp__fail",
            "mcp__probe-mcp__slow"
        ]
    );

    let echo = tools
        .iter()
        .find(|tool| tool.tool_name == "echo")
        .expect("echo tool");
    let result = connection
        .call_tool(echo, &json!({ "text": "roundtrip" }))
        .expect("echo call");
    assert_eq!(result["content"], json!("echo:roundtrip"));
    assert_eq!(result["display"]["kind"], json!("mcp_tool"));
    assert_eq!(result["display"]["serverName"], json!("probe-mcp"));
    assert_eq!(result["display"]["toolName"], json!("echo"));

    let fail = tools
        .iter()
        .find(|tool| tool.tool_name == "fail")
        .expect("fail tool");
    let result = connection
        .call_tool(fail, &json!({}))
        .expect("fail call keeps success envelope");
    assert_eq!(
        result["content"],
        json!("MCP tool returned an error:\nprobe-mcp deliberate failure")
    );

    let processes = hub.child_processes();
    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].server_name, "probe-mcp");

    // 连接幂等：再 connect 不新增子进程。
    hub.connect(&server);
    assert_eq!(hub.child_processes().len(), 1);
}

// ---------------------------------------------------------------------------
// stdio：失败分类
// ---------------------------------------------------------------------------

#[test]
fn stdio_start_failure_is_process_start_failed() {
    if !node_available() {
        eprintln!("skip: node 不可用");
        return;
    }
    let root = TempRoot::new("crash");
    let script = write_node_server(root.path(), "crash");
    let hub = McpHub::new();
    let connection = hub.connect(&stdio_server("probe-crash", &script, Some(5_000)));
    let status = connection.status();
    assert_eq!(status.status, McpServerStatusKind::Failed);
    assert_eq!(
        status.failure_kind,
        Some(McpServerFailureKind::ProcessStartFailed)
    );
    assert_eq!(status.error.as_deref(), Some("Connection closed"));
    // 失败后不留子进程。
    assert!(hub.child_processes().is_empty());
}

#[test]
fn stdio_non_json_output_is_connection_timeout() {
    if !node_available() {
        eprintln!("skip: node 不可用");
        return;
    }
    let root = TempRoot::new("garbage");
    let script = write_node_server(root.path(), "garbage");
    let hub = McpHub::new();
    let connection = hub.connect(&stdio_server("probe-garbage", &script, Some(300)));
    let status = connection.status();
    assert_eq!(status.status, McpServerStatusKind::Failed);
    assert_eq!(
        status.failure_kind,
        Some(McpServerFailureKind::ConnectionTimeout)
    );
    assert_eq!(
        status.error.as_deref(),
        Some("MCP server probe-garbage connection timed out after 300ms")
    );
}

// ---------------------------------------------------------------------------
// http：404 诊断
// ---------------------------------------------------------------------------

#[test]
fn http_404_is_network_unreachable_with_post_error() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let address = listener.local_addr().expect("local addr");
    let http = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buffer = [0u8; 8192];
        let _ = stream.read(&mut buffer);
        let body = "{\"error\":\"not_found\"}";
        let response = format!(
            "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
    });
    let hub = McpHub::new();
    let server = McpServer::Http(McpServerHttp {
        name: "probe-http".to_string(),
        transport: McpTransportType::Http,
        url: format!("http://{address}/mcp"),
        headers: Vec::new(),
        oauth: None,
        isolation: None,
        protocol_version: None,
        timeout_ms: Some(5_000),
    });
    let connection = hub.connect(&server);
    let status = connection.status();
    assert_eq!(status.status, McpServerStatusKind::Failed);
    assert_eq!(
        status.failure_kind,
        Some(McpServerFailureKind::NetworkUnreachable)
    );
    assert_eq!(
        status.error.as_deref(),
        Some("Error POSTing to endpoint: {\"error\":\"not_found\"}")
    );
    assert_eq!(status.transport, McpTransportKind::Http);
    assert!(hub.child_processes().is_empty());
    let _ = http.join();
}

// ---------------------------------------------------------------------------
// mcp/list：status 不建连、connect 幂等
// ---------------------------------------------------------------------------

fn workspace(path: &Path) -> Value {
    let path = path.display().to_string();
    json!({
        "workspacePath": path,
        "workspaceKey": path,
    })
}

#[test]
fn mcp_list_status_does_not_connect_and_connect_is_idempotent() {
    if !node_available() {
        eprintln!("skip: node 不可用");
        return;
    }
    let root = TempRoot::new("list");
    let script = write_node_server(root.path(), "happy");
    let mcp_server = stdio_server("probe-mcp", &script, Some(10_000));
    let server = AgentServer::new();
    let params = json!({
        "workspace": workspace(root.path()),
        "mcpServers": [serde_json::to_value(&mcp_server).expect("server json")],
        "mode": "status",
    });

    let status_only = server
        .dispatch("mcp/list", params.clone())
        .expect("status dispatch");
    let statuses = &status_only["statuses"]["probe-mcp"];
    assert_eq!(statuses["status"], json!("disconnected"));
    assert_eq!(statuses["transport"], json!("stdio"));
    assert!(server.mcp_hub().child_processes().is_empty());

    let connected = server
        .dispatch(
            "mcp/list",
            json!({
                "workspace": workspace(root.path()),
                "mcpServers": [serde_json::to_value(&mcp_server).expect("server json")],
                "mode": "connect",
            }),
        )
        .expect("connect dispatch");
    assert_eq!(
        connected["statuses"]["probe-mcp"]["status"],
        json!("connected")
    );
    assert_eq!(
        connected["statuses"]["probe-mcp"]["protocolEra"],
        json!("legacy")
    );
    assert_eq!(connected["statuses"]["probe-mcp"]["toolCount"], json!(3));

    // 重复 connect 不新增子进程。
    let again = server
        .dispatch(
            "mcp/list",
            json!({
                "workspace": workspace(root.path()),
                "mcpServers": [serde_json::to_value(&mcp_server).expect("server json")],
                "mode": "connect",
            }),
        )
        .expect("connect again");
    assert_eq!(again["statuses"]["probe-mcp"]["status"], json!("connected"));
    assert_eq!(server.mcp_hub().child_processes().len(), 1);

    let processes = server
        .dispatch("process/childProcesses", json!({}))
        .expect("child processes dispatch");
    assert_eq!(processes["processes"].as_array().expect("array").len(), 1);
    assert_eq!(processes["processes"][0]["serverName"], json!("probe-mcp"));
    assert_eq!(processes["processes"][0]["mcpSource"], json!("custom"));

    // 严格参数：未知键 -32602。
    let error = server
        .dispatch("process/childProcesses", json!({ "unexpected": true }))
        .expect_err("unknown key must fail");
    assert_eq!(error.code(), -32602);
}

// ---------------------------------------------------------------------------
// 会话工具源：首 turn 惰性建连
// ---------------------------------------------------------------------------

#[test]
fn session_tool_source_connects_lazily_and_calls_through_hub() {
    if !node_available() {
        eprintln!("skip: node 不可用");
        return;
    }
    let root = TempRoot::new("session");
    let script = write_node_server(root.path(), "happy");
    let hub = Arc::new(McpHub::new());
    let session = hub.session(vec![stdio_server("probe-mcp", &script, Some(10_000))]);

    // 构造工具源不等于建连（create 后子进程为空）。
    assert!(hub.child_processes().is_empty());

    let declarations = session.list_tools().expect("list tools");
    assert_eq!(declarations.len(), 3);
    assert_eq!(hub.child_processes().len(), 1);

    let echo = declarations
        .iter()
        .find(|declaration| declaration.name == "mcp__probe-mcp__echo")
        .expect("echo declaration");
    assert_eq!(echo.read_only, Some(true));
    assert_eq!(echo.side_effect_scope.as_deref(), Some("session"));
    assert_eq!(echo.budget_strategy.as_deref(), Some("truncate"));
    let display: Value = serde_json::from_str(
        echo.display_json
            .as_deref()
            .expect("mcp declaration carries display"),
    )
    .expect("display is valid JSON");
    assert_eq!(
        display,
        json!({
            "kind": "mcp_tool",
            "serverName": "probe-mcp",
            "toolName": "echo",
            "description": "Echo the given text back.",
        })
    );

    let result = session
        .call_tool("mcp__probe-mcp__echo", &json!({ "text": "via-session" }))
        .expect("call through session");
    assert_eq!(result["content"], json!("echo:via-session"));

    let unknown = session
        .call_tool("mcp__probe-mcp__missing", &json!({}))
        .expect_err("unknown tool");
    assert_eq!(unknown.error_type, "tool_execution_failed");
}

// ---------------------------------------------------------------------------
// 失败 server 不阻断其他 server 的工具目录
// ---------------------------------------------------------------------------

#[test]
fn failed_server_does_not_block_other_mcp_tools() {
    if !node_available() {
        eprintln!("skip: node 不可用");
        return;
    }
    let root = TempRoot::new("mixed");
    let happy = write_node_server(root.path(), "happy");
    let crash = write_node_server(root.path(), "crash");
    let hub = Arc::new(McpHub::new());
    let session = hub.session(vec![
        stdio_server("probe-crash", &crash, Some(5_000)),
        stdio_server("probe-mcp", &happy, Some(10_000)),
    ]);

    let declarations = session.list_tools().expect("list tools");
    assert_eq!(declarations.len(), 3);
    assert!(declarations
        .iter()
        .all(|declaration| declaration.name.contains("probe-mcp")));
    // 失败 server 的状态留在 hub 里，会话继续可用。
    let statuses = hub.statuses(&[stdio_server("probe-crash", &crash, Some(5_000))]);
    assert_eq!(
        statuses["probe-crash"].failure_kind,
        Some(McpServerFailureKind::ProcessStartFailed)
    );
}

// ---------------------------------------------------------------------------
// 会话循环：MCP 工具目录 + tool.updated 事件形状
// ---------------------------------------------------------------------------

#[test]
fn session_tool_batch_uses_mcp_catalog_and_emits_observed_events() {
    if !node_available() {
        eprintln!("skip: node 不可用");
        return;
    }
    let root = TempRoot::new("batch");
    let script = write_node_server(root.path(), "happy");
    let hub = Arc::new(McpHub::new());
    let source = hub.session(vec![stdio_server("probe-mcp", &script, Some(10_000))]);
    let declarations = source.list_tools().expect("list tools");

    let mut session = Session::new("sess_mcp_batch", root.path().display().to_string());
    let tools = declarations
        .into_iter()
        .map(agent_core::PluginToolDeclaration::into_spec)
        .collect();
    session.set_dynamic_tools(tools, source);

    let catalog = session.tools();
    assert!(catalog
        .iter()
        .any(|spec| spec.name == "mcp__probe-mcp__echo"));

    let _started_turn = session.begin_turn("MCP batch");
    let events = session.run_tool_batch(&[ToolCallRequest {
        tool_call_id: "call_mcp_echo".to_string(),
        name: "mcp__probe-mcp__echo".to_string(),
        input: json!({ "text": "batch" }),
    }]);
    let mut scheduled = None;
    let mut started = None;
    let mut result = None;
    for event in &events {
        if event.event_type != agent_core::SessionEventType::ToolUpdated {
            continue;
        }
        match event
            .payload_as::<agent_protocol::session_event::ToolUpdatedPayload>()
            .expect("decode tool payload")
        {
            Some(agent_protocol::session_event::ToolUpdatedPayload::Scheduled(payload)) => {
                scheduled = Some(payload);
            }
            Some(agent_protocol::session_event::ToolUpdatedPayload::Started(payload)) => {
                started = Some(payload);
            }
            Some(agent_protocol::session_event::ToolUpdatedPayload::Result(payload)) => {
                result = Some(payload);
            }
            _ => {}
        }
    }
    let scheduled = scheduled.expect("scheduled event");
    assert_eq!(scheduled.can_run_parallel, Some(true));
    assert_eq!(
        scheduled.extra.get("display"),
        Some(&json!({
            "kind": "mcp_tool",
            "serverName": "probe-mcp",
            "toolName": "echo",
            "description": "Echo the given text back.",
        }))
    );
    let started = started.expect("started event");
    assert_eq!(started.read_only, Some(true));
    assert_eq!(started.side_effect_scope.as_deref(), Some("session"));
    let result = result.expect("result event");
    assert_eq!(result.result.success, Some(true));
    assert_eq!(result.result.content, Some(json!("echo:batch")));
    assert_eq!(
        result.result.extra.get("display"),
        Some(&json!({
            "kind": "mcp_tool",
            "serverName": "probe-mcp",
            "toolName": "echo",
            "description": "Echo the given text back.",
        }))
    );
    assert_eq!(
        result.result.extra.get("budgetStrategy"),
        Some(&json!("truncate"))
    );
}

#[test]
fn plugin_mcp_uses_plugin_tool_name_and_origin() {
    if !node_available() {
        eprintln!("skip: node 不可用");
        return;
    }
    let root = TempRoot::new("plugin-origin");
    let script = write_node_server(root.path(), "happy");
    let hub = Arc::new(McpHub::new());
    let server = stdio_server("plugin:fixture:fixture", &script, Some(10_000));
    let source = hub.session_with_origin(
        vec![server],
        agent_protocol::workspace::McpSource::Plugin,
        "fixture".to_string(),
    );
    let declarations = source.list_tools().expect("list tools");
    let names: Vec<_> = declarations
        .iter()
        .map(|declaration| declaration.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "mcp__plugin_fixture_fixture__echo",
            "mcp__plugin_fixture_fixture__fail",
            "mcp__plugin_fixture_fixture__slow",
        ]
    );
    // 插件 MCP 是 canRunParallel:false / started 非只读（自定义 MCP 相反）。
    assert_eq!(declarations[0].read_only, Some(false));
    assert_eq!(
        declarations[0].side_effect_scope.as_deref(),
        Some("network")
    );
    let display: Value = serde_json::from_str(
        declarations[0]
            .display_json
            .as_deref()
            .expect("display json"),
    )
    .expect("valid display");
    assert_eq!(display["serverName"], json!("plugin:fixture:fixture"));

    let processes = hub.child_processes();
    assert_eq!(processes.len(), 1);
    assert_eq!(
        processes[0].mcp_source,
        agent_protocol::workspace::McpSource::Plugin
    );
    assert_eq!(processes[0].plugin_name.as_deref(), Some("fixture"));
}
