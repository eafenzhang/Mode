//! MCP 会话面（LT-1 C4 T1）：stdio/http 连接、工具目录、调用与失败诊断。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §1 与
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c4/mcp-*.json`。
//!
//! # 所有权
//!
//! [`McpHub`] 是连接的唯一所有者：`mcp/list` 与 per-session turn 都通过同一个
//! `name → Arc<McpConnection>` 表读写状态，不在会话里复制第二份连接/目录。
//! 会话只持有自己的 server 清单（[`McpSession`]），首 turn 才按需建连
//! （夹具：create 后 `process/childProcesses` 为空）。
//!
//! # 线程模型
//!
//! - stdio：`std::process` + 读线程 + 请求 id → channel 表；调用方阻塞等待应答。
//! - http：`reqwest::blocking`，每次请求放到独立线程（dispatch/turn 都可能在 tokio
//!   运行时线程上，blocking client 不能在 async 上下文里直接构造/析构）。
//! - 所有阻塞点都只在 worker 的 `spawn_blocking` 线程或 stdio dispatch 线程上，
//!   不占 tokio worker。
//!
//! # 失败分类（观察值）
//!
//! | 场景                                   | `failureKind`        | `error`                                                    |
//! | -------------------------------------- | -------------------- | ---------------------------------------------------------- |
//! | 进程启动失败/握手前退出                | `process_start_failed` | `Connection closed`                                        |
//! | 30s 内没有有效应答                     | `connection_timeout`   | `MCP server <name> connection timed out after 30000ms`     |
//! | http 非 2xx / 连接失败                 | `network_unreachable`  | `Error POSTing to endpoint: <响应正文>`                     |
//! | `tools/list` 失败                      | `tool_list_failed`     | 原错误文本                                                 |

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::time::Duration;

use agent_core::{PluginToolDeclaration, PluginToolError, PluginToolSource};
use agent_protocol::mcp::{
    McpProtocolEra, McpServerFailureKind, McpServerStatusKind, McpServerStatusSnapshot,
    McpTransportKind,
};
use agent_protocol::session_create::{McpServer, McpServerHttp, McpServerStdio, McpTransportType};
use agent_protocol::workspace::{ChildProcess, McpSource};
use serde_json::{json, Value};

/// 观察值：握手 30s 超时（`probe-c4-mcp` 的非 JSON server 夹具）。
pub const DEFAULT_CONNECTION_TIMEOUT_MS: u64 = 30_000;

/// 协议版本：观察到的 legacy 时代版本。
const LEGACY_PROTOCOL_VERSION: &str = "2024-11-05";

/// 一个 MCP 工具（原始名 + 目录限定名）。
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolDescriptor {
    pub server_name: String,
    pub tool_name: String,
    /// `mcp__<server>__<tool>`（观察；插件 MCP 由 T2 另用 `mcp__plugin_…__…`）。
    pub qualified_name: String,
    pub description: String,
    pub input_schema: Value,
}

impl McpToolDescriptor {
    fn from_tool(server_name: &str, tool: &Value) -> Option<Self> {
        let tool_name = tool.get("name").and_then(Value::as_str)?.to_string();
        let description = tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let input_schema = tool
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object" }));
        Some(Self {
            qualified_name: format!("mcp__{server_name}__{tool_name}"),
            server_name: server_name.to_string(),
            tool_name,
            description,
            input_schema,
        })
    }

    fn display(&self) -> Value {
        json!({
            "kind": "mcp_tool",
            "serverName": self.server_name,
            "toolName": self.tool_name,
            "description": self.description,
        })
    }
}

/// 请求失败分类（只在本模块使用；对外的投影是 `failureKind`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestFailure {
    Closed,
    Timeout,
    Network,
}

#[derive(Debug, Clone)]
struct McpRequestError {
    failure: RequestFailure,
    message: String,
}

impl McpRequestError {
    fn closed() -> Self {
        Self {
            failure: RequestFailure::Closed,
            message: "Connection closed".to_string(),
        }
    }

    fn timeout(server: &str, timeout_ms: u64) -> Self {
        Self {
            failure: RequestFailure::Timeout,
            message: format!("MCP server {server} connection timed out after {timeout_ms}ms"),
        }
    }

    fn network(message: impl Into<String>) -> Self {
        Self {
            failure: RequestFailure::Network,
            message: message.into(),
        }
    }
}

type PendingMap = Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, McpRequestError>>>>>;

struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    pending: PendingMap,
}

/// 连接状态（stdin/子进程只在 stdio 下存在）。
struct ConnectionState {
    stdio: Option<StdioTransport>,
    connected: bool,
    protocol_era: Option<McpProtocolEra>,
}

/// 一个 MCP server 的连接与状态（`McpHub` 的唯一子项）。
pub struct McpConnection {
    server: McpServer,
    transport: McpTransportKind,
    state: Mutex<ConnectionState>,
    /// 建连串行化：两个调用方同时 connect 时只跑一次握手。
    connect_lock: Mutex<()>,
    status: RwLock<McpServerStatusSnapshot>,
    tools: RwLock<Vec<McpToolDescriptor>>,
    next_id: AtomicU64,
}

impl std::fmt::Debug for McpConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpConnection")
            .field("name", &self.name())
            .field("transport", &self.transport)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl McpConnection {
    fn new(server: McpServer) -> Self {
        let transport = transport_of(&server);
        let now = iso8601_now();
        Self {
            status: RwLock::new(McpServerStatusSnapshot {
                status: McpServerStatusKind::Disconnected,
                transport,
                tool_count: 0,
                updated_at: now,
                error: None,
                failure_kind: None,
                server_request_id: None,
                protocol_era: None,
                authorization: None,
            }),
            server,
            transport,
            state: Mutex::new(ConnectionState {
                stdio: None,
                connected: false,
                protocol_era: None,
            }),
            connect_lock: Mutex::new(()),
            tools: RwLock::new(Vec::new()),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn name(&self) -> String {
        match &self.server {
            McpServer::Stdio(server) => server.name.clone(),
            McpServer::Http(server) => server.name.clone(),
        }
    }

    pub fn transport(&self) -> McpTransportKind {
        self.transport
    }

    pub fn status(&self) -> McpServerStatusSnapshot {
        self.status
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn tools(&self) -> Vec<McpToolDescriptor> {
        self.tools
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn tool_by_qualified(&self, qualified: &str) -> Option<McpToolDescriptor> {
        self.tools()
            .into_iter()
            .find(|tool| tool.qualified_name == qualified)
    }

    /// 幂等建连；失败时把状态写成 `failed` 并返回 `false`（不 panic、不中断会话）。
    pub fn connect(&self) -> bool {
        let _guard = self
            .connect_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.status().status == McpServerStatusKind::Connected {
            return true;
        }
        let outcome = match &self.server {
            McpServer::Stdio(server) => self.connect_stdio(server),
            McpServer::Http(server) => self.connect_http(server),
        };
        match outcome {
            Ok((era, tools)) => {
                self.store_connected(era, tools);
                true
            }
            Err(error) => {
                self.kill_stdio();
                self.store_failure(error);
                false
            }
        }
    }

    /// 失败后回收 stdio 子进程，避免失败 server 留孤儿进程。
    fn kill_stdio(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(mut transport) = state.stdio.take() {
            let _ = transport.child.kill();
            let _ = transport.child.wait();
        }
        state.connected = false;
    }

    fn store_connected(&self, era: McpProtocolEra, tools: Vec<McpToolDescriptor>) {
        let tool_count = tools.len() as u64;
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.connected = true;
            state.protocol_era = Some(era);
        }
        *self
            .tools
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = tools;
        *self
            .status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = McpServerStatusSnapshot {
            status: McpServerStatusKind::Connected,
            transport: self.transport,
            tool_count,
            updated_at: iso8601_now(),
            error: None,
            failure_kind: None,
            server_request_id: None,
            protocol_era: Some(era),
            authorization: None,
        };
    }

    fn store_failure(&self, error: McpRequestError) {
        let failure_kind = match error.failure {
            RequestFailure::Closed => McpServerFailureKind::ProcessStartFailed,
            RequestFailure::Timeout => McpServerFailureKind::ConnectionTimeout,
            RequestFailure::Network => McpServerFailureKind::NetworkUnreachable,
        };
        *self
            .status
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = McpServerStatusSnapshot {
            status: McpServerStatusKind::Failed,
            transport: self.transport,
            tool_count: 0,
            updated_at: iso8601_now(),
            error: Some(error.message),
            failure_kind: Some(failure_kind),
            server_request_id: None,
            protocol_era: None,
            authorization: None,
        };
    }

    fn connect_stdio(
        &self,
        server: &McpServerStdio,
    ) -> Result<(McpProtocolEra, Vec<McpToolDescriptor>), McpRequestError> {
        let timeout = timeout_of(server.timeout_ms);
        let mut command = Command::new(&server.command);
        command
            .args(&server.args)
            .envs(server.env.iter().map(|entry| (&entry.name, &entry.value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| McpRequestError::closed_with(error.to_string()))?;
        let stdin = child.stdin.take().ok_or_else(McpRequestError::closed)?;
        let stdout = child.stdout.take().ok_or_else(McpRequestError::closed)?;
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        spawn_reader(stdout, Arc::clone(&pending));
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.stdio = Some(StdioTransport {
                child,
                stdin,
                pending,
            });
        }
        let era = self.handshake_stdio(&server.name, timeout)?;
        let tools = self.list_tools_stdio(&server.name, timeout)?;
        Ok((era, tools))
    }

    fn connect_http(
        &self,
        server: &McpServerHttp,
    ) -> Result<(McpProtocolEra, Vec<McpToolDescriptor>), McpRequestError> {
        let timeout = timeout_of(server.timeout_ms);
        let initialize = self.http_request(
            server,
            "initialize",
            json!({
                "protocolVersion": LEGACY_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "clientInfo": { "name": "mode-agent", "version": "0.0.0" },
            }),
            timeout,
        )?;
        let version = initialize
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(LEGACY_PROTOCOL_VERSION);
        let era = protocol_era_of(version);
        self.http_notify(server, "notifications/initialized", timeout)?;
        let result = self.http_request(server, "tools/list", json!({}), timeout)?;
        Ok((era, parse_tools(&server.name, &result)))
    }

    fn handshake_stdio(
        &self,
        server_name: &str,
        timeout: Duration,
    ) -> Result<McpProtocolEra, McpRequestError> {
        let result = self.stdio_request(
            "initialize",
            json!({
                "protocolVersion": LEGACY_PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "clientInfo": { "name": "mode-agent", "version": "0.0.0" },
            }),
            server_name,
            timeout,
        )?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(LEGACY_PROTOCOL_VERSION);
        self.stdio_notify("notifications/initialized")?;
        Ok(protocol_era_of(version))
    }

    fn list_tools_stdio(
        &self,
        server_name: &str,
        timeout: Duration,
    ) -> Result<Vec<McpToolDescriptor>, McpRequestError> {
        let result = self
            .stdio_request("tools/list", json!({}), server_name, timeout)
            .map_err(|error| McpRequestError {
                failure: error.failure,
                message: error.message,
            })?;
        Ok(parse_tools(server_name, &result))
    }

    fn stdio_request(
        &self,
        method: &str,
        params: Value,
        server_name: &str,
        timeout: Duration,
    ) -> Result<Value, McpRequestError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = mpsc::channel();
        let pending = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let transport = state.stdio.as_ref().ok_or_else(McpRequestError::closed)?;
            transport
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(id, sender);
            Arc::clone(&transport.pending)
        };
        let write_result = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(transport) = state.stdio.as_mut() else {
                return Err(McpRequestError::closed());
            };
            let line = serde_json::to_string(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }))
            .unwrap_or_default();
            writeln!(transport.stdin, "{line}")
                .and_then(|_| transport.stdin.flush())
                .map_err(|error| McpRequestError::closed_with(error.to_string()))
        };
        if let Err(error) = write_result {
            pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&id);
            return Err(error);
        }
        let response = match receiver.recv_timeout(timeout) {
            Ok(response) => response,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(McpRequestError::timeout(
                server_name,
                timeout.as_millis() as u64,
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(McpRequestError::closed()),
        };
        pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&id);
        let response = response?;
        if let Some(error) = response.get("error") {
            return Err(McpRequestError::network(format!(
                "MCP error: {}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown JSON-RPC error")
            )));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    fn stdio_notify(&self, method: &str) -> Result<(), McpRequestError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(transport) = state.stdio.as_mut() else {
            return Err(McpRequestError::closed());
        };
        let line = serde_json::to_string(&json!({ "jsonrpc": "2.0", "method": method }))
            .unwrap_or_default();
        writeln!(transport.stdin, "{line}")
            .and_then(|_| transport.stdin.flush())
            .map_err(|error| McpRequestError::closed_with(error.to_string()))
    }

    fn http_notify(
        &self,
        server: &McpServerHttp,
        method: &str,
        timeout: Duration,
    ) -> Result<(), McpRequestError> {
        // JSON-RPC 通知没有 id、不要求应答；非 2xx 仍按网络失败处理。
        let url = server.url.clone();
        let headers: Vec<(String, String)> = server
            .headers
            .iter()
            .map(|entry| (entry.name.clone(), entry.value.clone()))
            .collect();
        let body = json!({ "jsonrpc": "2.0", "method": method });
        let body_text = serde_json::to_string(&body).unwrap_or_default();
        let server_name = server.name.clone();
        let timeout_ms = timeout.as_millis() as u64;
        let join = std::thread::spawn(move || {
            ensure_tls_provider();
            let client = reqwest::blocking::Client::builder()
                .timeout(timeout)
                .build()
                .map_err(|error| McpRequestError::network(error.to_string()))?;
            let mut request = client.post(&url).header("content-type", "application/json");
            for (name, value) in &headers {
                request = request.header(name.as_str(), value.as_str());
            }
            let response = request.body(body_text).send().map_err(|error| {
                if error.is_timeout() {
                    McpRequestError::timeout(&server_name, timeout_ms)
                } else {
                    McpRequestError::network(format!("Error POSTing to endpoint: {error}"))
                }
            })?;
            if !response.status().is_success() {
                let text = response.text().unwrap_or_default();
                return Err(McpRequestError::network(format!(
                    "Error POSTing to endpoint: {}",
                    text.trim()
                )));
            }
            Ok(())
        });
        join.join().unwrap_or_else(|_| {
            Err(McpRequestError::network(
                "MCP http request thread panicked".to_string(),
            ))
        })
    }

    fn http_request(
        &self,
        server: &McpServerHttp,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, McpRequestError> {
        let url = server.url.clone();
        let headers: Vec<(String, String)> = server
            .headers
            .iter()
            .map(|entry| (entry.name.clone(), entry.value.clone()))
            .collect();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let body_text = serde_json::to_string(&body).unwrap_or_default();
        let timeout_ms = timeout.as_millis() as u64;
        let server_name = server.name.clone();
        // reqwest::blocking 不能在 tokio 运行时上下文中构造/析构；放独立线程。
        let join = std::thread::spawn(move || {
            ensure_tls_provider();
            let client = reqwest::blocking::Client::builder()
                .timeout(timeout)
                .build()
                .map_err(|error| McpRequestError::network(error.to_string()))?;
            let mut request = client
                .post(&url)
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream");
            for (name, value) in &headers {
                request = request.header(name.as_str(), value.as_str());
            }
            let response = request.body(body_text).send().map_err(|error| {
                if error.is_timeout() {
                    McpRequestError::timeout(&server_name, timeout_ms)
                } else {
                    McpRequestError::network(format!("Error POSTing to endpoint: {error}"))
                }
            })?;
            let status = response.status();
            let text = response.text().unwrap_or_default();
            if !status.is_success() {
                return Err(McpRequestError::network(format!(
                    "Error POSTing to endpoint: {}",
                    text.trim()
                )));
            }
            let value: Value = serde_json::from_str(&text)
                .map_err(|error| McpRequestError::network(error.to_string()))?;
            if let Some(error) = value.get("error") {
                return Err(McpRequestError::network(format!(
                    "MCP error: {}",
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown JSON-RPC error")
                )));
            }
            Ok(value.get("result").cloned().unwrap_or(Value::Null))
        });
        join.join().unwrap_or_else(|_| {
            Err(McpRequestError::network(
                "MCP http request thread panicked".to_string(),
            ))
        })
    }

    /// 调用一个工具；返回 `{content, display}`（`PluginToolSource` 的约定）。
    pub fn call_tool(
        &self,
        tool: &McpToolDescriptor,
        input: &Value,
    ) -> Result<Value, PluginToolError> {
        if !self.connect() {
            let status = self.status();
            return Err(sdk_closed(
                status
                    .error
                    .unwrap_or_else(|| "Connection closed".to_string()),
            ));
        }
        let timeout = timeout_of(match &self.server {
            McpServer::Stdio(server) => server.timeout_ms,
            McpServer::Http(server) => server.timeout_ms,
        });
        let result = match &self.server {
            McpServer::Stdio(server) => self.stdio_request(
                "tools/call",
                json!({ "name": tool.tool_name.clone(), "arguments": input }),
                &server.name,
                timeout,
            ),
            McpServer::Http(server) => self.http_request(
                server,
                "tools/call",
                json!({ "name": tool.tool_name.clone(), "arguments": input }),
                timeout,
            ),
        };
        let result =
            result.map_err(|error| PluginToolError::tool_execution_failed(error.message))?;
        let is_error = result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let text = mcp_content_text(&result);
        let content = if is_error {
            format!("MCP tool returned an error:\n{text}")
        } else {
            text
        };
        Ok(json!({ "content": content, "display": tool.display() }))
    }
}

impl McpRequestError {
    fn closed_with(message: String) -> Self {
        Self {
            failure: RequestFailure::Closed,
            message: if message.is_empty() {
                "Connection closed".to_string()
            } else {
                message
            },
        }
    }
}

fn sdk_closed(message: String) -> PluginToolError {
    PluginToolError {
        error_type: "SdkError".to_string(),
        message,
        code: Some("CONNECTION_CLOSED".to_string()),
    }
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(mut transport) = state.stdio.take() {
                let _ = transport.child.kill();
                let _ = transport.child.wait();
            }
        }
    }
}

/// 读线程：按行解析 JSON-RPC，响应按 `id` 投递；stdout 关闭时把未决请求全部结算。
fn spawn_reader(stdout: impl std::io::Read + Send + 'static, pending: PendingMap) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(id) = value.get("id").and_then(Value::as_u64) else {
                continue;
            };
            let sender = pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&id);
            if let Some(sender) = sender {
                let _ = sender.send(Ok(value));
            }
        }
        // stdout 关闭：所有未决请求都是连接关闭（与观察的 `Connection closed` 一致）。
        let remaining: Vec<_> = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain()
            .map(|(_, sender)| sender)
            .collect();
        for sender in remaining {
            let _ = sender.send(Err(McpRequestError::closed()));
        }
    });
}

/// 连接的唯一所有者：`mcp/list` 与 per-session turn 共用。
#[derive(Default)]
pub struct McpHub {
    connections: Mutex<HashMap<String, Arc<McpConnection>>>,
}

impl std::fmt::Debug for McpHub {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpHub")
            .field("connections", &self.connections.lock().map(|map| map.len()))
            .finish()
    }
}

impl McpHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取得（必要时创建）连接条目；不建连。
    pub fn connection(&self, server: &McpServer) -> Arc<McpConnection> {
        let name = server_name(server);
        let mut connections = self
            .connections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(
            connections
                .entry(name)
                .or_insert_with(|| Arc::new(McpConnection::new(server.clone()))),
        )
    }

    /// 取条目并建连（幂等）；失败不 panic，状态留在条目里。
    pub fn connect(&self, server: &McpServer) -> Arc<McpConnection> {
        let connection = self.connection(server);
        connection.connect();
        connection
    }

    /// `mcp/list` 的 status 投影：只报参数里的 server（顺序按 name 排序）。
    pub fn statuses(&self, servers: &[McpServer]) -> BTreeMap<String, McpServerStatusSnapshot> {
        let mut statuses = BTreeMap::new();
        for server in servers {
            let connection = self.connection(server);
            statuses.insert(server_name(server), connection.status());
        }
        statuses
    }

    /// `process/childProcesses`：已建连的 stdio 子进程。
    pub fn child_processes(&self) -> Vec<ChildProcess> {
        let connections: Vec<Arc<McpConnection>> = self
            .connections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect();
        let mut processes = Vec::new();
        for connection in connections {
            let state = connection
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(transport) = &state.stdio {
                processes.push(ChildProcess {
                    pid: transport.child.id() as u64,
                    server_name: connection.name(),
                    mcp_source: McpSource::Custom,
                    plugin_name: None,
                });
            }
        }
        processes
    }

    /// 为一个会话构造工具源（首 turn 才调用 `list_tools`）。
    ///
    /// 会话与 hub 共享同一张连接表：`mcp/list` 建过的连接、会话首 turn 建的连接都写同一处，
    /// `process/childProcesses` 与后续 `mcp/list status` 看到同一份事实。
    pub fn session(self: &Arc<Self>, servers: Vec<McpServer>) -> Arc<McpSession> {
        Arc::new(McpSession {
            hub: Arc::clone(self),
            servers,
        })
    }
}

/// per-session 的 MCP 工具源：只持有 server 清单，连接仍归 hub。
pub struct McpSession {
    hub: Arc<McpHub>,
    servers: Vec<McpServer>,
}

impl std::fmt::Debug for McpSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpSession")
            .field("servers", &self.servers.len())
            .finish()
    }
}

impl PluginToolSource for McpSession {
    fn list_tools(&self) -> Result<Vec<PluginToolDeclaration>, String> {
        let mut declarations = Vec::new();
        for server in &self.servers {
            let connection = self.hub.connect(server);
            for tool in connection.tools() {
                declarations.push(PluginToolDeclaration {
                    name: tool.qualified_name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                    // 自定义 MCP 的观察档：只读、session 副作用、truncate、medium。
                    risk_level: Some("medium".to_string()),
                    side_effect_scope: Some("session".to_string()),
                    budget_strategy: Some("truncate".to_string()),
                    read_only: Some(true),
                    display_json: Some(tool.display().to_string()),
                });
            }
        }
        Ok(declarations)
    }

    fn call_tool(&self, tool_name: &str, input: &Value) -> Result<Value, PluginToolError> {
        for server in &self.servers {
            let connection = self.hub.connection(server);
            if let Some(tool) = connection.tool_by_qualified(tool_name) {
                return connection.call_tool(&tool, input);
            }
        }
        Err(PluginToolError::tool_execution_failed(format!(
            "Unknown MCP tool: {tool_name}"
        )))
    }
}

fn server_name(server: &McpServer) -> String {
    match server {
        McpServer::Stdio(server) => server.name.clone(),
        McpServer::Http(server) => server.name.clone(),
    }
}

fn transport_of(server: &McpServer) -> McpTransportKind {
    match server {
        McpServer::Stdio(_) => McpTransportKind::Stdio,
        McpServer::Http(server) => match server.transport {
            McpTransportType::Http => McpTransportKind::Http,
            McpTransportType::Sse => McpTransportKind::Sse,
        },
    }
}

fn timeout_of(timeout_ms: Option<u64>) -> Duration {
    Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_CONNECTION_TIMEOUT_MS))
}

fn protocol_era_of(version: &str) -> McpProtocolEra {
    if version == LEGACY_PROTOCOL_VERSION {
        McpProtocolEra::Legacy
    } else {
        McpProtocolEra::Modern
    }
}

fn parse_tools(server_name: &str, result: &Value) -> Vec<McpToolDescriptor> {
    result
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| McpToolDescriptor::from_tool(server_name, tool))
                .collect()
        })
        .unwrap_or_default()
}

/// MCP `result.content` → 正文：text part 用换行连接（观察单 text）。
fn mcp_content_text(result: &Value) -> String {
    let Some(content) = result.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    content
        .iter()
        .filter_map(|part| {
            if part.get("type").and_then(Value::as_str) == Some("text") {
                part.get("text").and_then(Value::as_str)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn iso8601_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    data_root::iso8601_utc(now)
}

/// reqwest 以 `rustls-no-provider` 编译；首次 https 请求前装 ring provider
/// （与 `provider-http` / `platform::source_url` 同一做法，重复调用安全）。
fn ensure_tls_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_name_and_display_shape() {
        let tool = McpToolDescriptor::from_tool(
            "probe-mcp",
            &json!({
                "name": "echo",
                "description": "Echo the given text back.",
                "inputSchema": { "type": "object" },
            }),
        )
        .expect("tool");
        assert_eq!(tool.qualified_name, "mcp__probe-mcp__echo");
        assert_eq!(tool.display()["kind"], json!("mcp_tool"));
        assert_eq!(tool.display()["serverName"], json!("probe-mcp"));
        assert_eq!(tool.display()["toolName"], json!("echo"));
    }

    #[test]
    fn mcp_content_joins_text_parts() {
        assert_eq!(
            mcp_content_text(&json!({
                "content": [
                    { "type": "text", "text": "a" },
                    { "type": "text", "text": "b" }
                ]
            })),
            "a\nb"
        );
    }

    #[test]
    fn protocol_era_mapping() {
        assert_eq!(protocol_era_of("2024-11-05"), McpProtocolEra::Legacy);
        assert_eq!(protocol_era_of("2026-07-28"), McpProtocolEra::Modern);
    }
}
