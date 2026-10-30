//! Mode Protocol 客户端内核（W4 前置）。
//!
//! 只负责**与进程所有权无关**的那一层：LF 分帧（只认 `\n`，不把 U+2028/U+2029 当换行）、
//! 请求 id 多路复用、超时、通知广播、入站请求的默认 `Method not found` 应答、关闭时把
//! 未完成请求统一失败。进程拉起/进程树终止/`app-server --stdio` 参数属于下一层。
//!
//! 行为真源：`packages/services/src/mode-agent/modeProtocolClient.ts` 与
//! `modeStdioTransport.ts` 的协议部分。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::AbortHandle;

/// 单条请求的默认超时，与 TS `DEFAULT_MODE_PROTOCOL_REQUEST_TIMEOUT_MS` 一致。
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(3 * 60);
/// 单帧上限（防御本地进程异常刷屏）；TS 侧没有显式上限，这里是 Rust 侧的保护。
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;
const NOTIFICATION_CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RequestId {
    Int(i64),
    Str(String),
}

impl RequestId {
    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_i64().map(Self::Int),
            Value::String(text) => Some(Self::Str(text.clone())),
            _ => None,
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Self::Int(number) => Value::from(*number),
            Self::Str(text) => Value::String(text.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    /// 传输已关闭（对端 EOF、显式 dispose、写失败）。
    Closed,
    Transport(String),
    Timeout { method: String },
    Protocol(String),
    /// Agent 返回的协议错误。
    Server {
        code: i64,
        message: String,
        data: Option<Value>,
    },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(formatter, "Mode Protocol client is closed"),
            Self::Transport(message) => write!(formatter, "Mode Protocol transport error: {message}"),
            Self::Timeout { method } => {
                write!(formatter, "Mode Protocol request timed out: {method}")
            }
            Self::Protocol(message) => write!(formatter, "Mode Protocol error: {message}"),
            Self::Server { code, message, .. } => {
                write!(formatter, "Mode Protocol server error {code}: {message}")
            }
        }
    }
}

impl std::error::Error for ClientError {}

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub request_timeout: Duration,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

/// 入站请求处理：返回 `Some(result)` 时回成功帧；`None` 回 `Method not found`。
pub type RequestHandler = Arc<dyn Fn(&Value) -> Option<Value> + Send + Sync>;

type Pending = Mutex<HashMap<RequestId, oneshot::Sender<Result<Value, ClientError>>>>;

struct Shared {
    out: mpsc::UnboundedSender<Option<Value>>,
    pending: Pending,
    next_id: AtomicI64,
    closed: AtomicBool,
    write_closed: AtomicBool,
    notifications: broadcast::Sender<Value>,
    request_handler: Mutex<Option<RequestHandler>>,
    tasks: Mutex<Vec<AbortHandle>>,
    request_timeout: Duration,
}

impl Shared {
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn fail_pending(&self, error: ClientError) {
        let drained: Vec<_> = self
            .pending
            .lock()
            .expect("pending map poisoned")
            .drain()
            .map(|(_, sender)| sender)
            .collect();
        for sender in drained {
            let _ = sender.send(Err(error.clone()));
        }
    }

    fn close_with(&self, error: ClientError) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.fail_pending(error);
    }

    fn send_frame(&self, value: Value) -> Result<(), ClientError> {
        if self.is_closed() || self.write_closed.load(Ordering::SeqCst) {
            return Err(ClientError::Closed);
        }
        self.out
            .send(Some(value))
            .map_err(|_| ClientError::Closed)
    }
}

#[derive(Clone)]
pub struct ProtocolClient {
    shared: Arc<Shared>,
}

impl ProtocolClient {
    /// 在任意双向流上启动客户端。reader/writer 由调用方提供（stdio、duplex、socket 均可）。
    pub fn connect<R, W>(reader: R, writer: W, options: ClientOptions) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out_tx, out_rx) = mpsc::unbounded_channel::<Option<Value>>();
        let (notification_tx, _) = broadcast::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let shared = Arc::new(Shared {
            out: out_tx,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(0),
            closed: AtomicBool::new(false),
            write_closed: AtomicBool::new(false),
            notifications: notification_tx,
            request_handler: Mutex::new(None),
            tasks: Mutex::new(Vec::new()),
            request_timeout: options.request_timeout,
        });

        let writer_shared = Arc::clone(&shared);
        let writer_task = tokio::spawn(async move {
            writer_loop(writer, out_rx, writer_shared).await;
        });
        let reader_shared = Arc::clone(&shared);
        let reader_task = tokio::spawn(async move {
            reader_loop(reader, reader_shared).await;
        });

        let mut tasks = shared.tasks.lock().expect("task list poisoned");
        tasks.push(writer_task.abort_handle());
        tasks.push(reader_task.abort_handle());
        drop(tasks);

        Self { shared }
    }

    /// 注册入站请求处理器（Agent → 客户端方向的请求）。
    pub fn set_request_handler(&self, handler: RequestHandler) {
        *self
            .shared
            .request_handler
            .lock()
            .expect("handler poisoned") = Some(handler);
    }

    pub fn subscribe_notifications(&self) -> broadcast::Receiver<Value> {
        self.shared.notifications.subscribe()
    }

    pub fn is_closed(&self) -> bool {
        self.shared.is_closed()
    }

    /// 只关闭写端（向对端发 EOF），读端继续接收响应/通知。
    /// 用于优雅停止 stdio 子进程：先 EOF，等它自行退出，再决定是否强杀。
    pub fn close_write(&self) {
        if self.shared.write_closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.shared.out.send(None);
    }

    pub fn pending_count(&self) -> usize {
        self.shared.pending.lock().expect("pending map poisoned").len()
    }

    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, ClientError> {
        self.request_with_timeout(method, params, self.shared.request_timeout)
            .await
    }

    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        if self.shared.is_closed() {
            return Err(ClientError::Closed);
        }
        let id = RequestId::Int(self.shared.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        let (sender, receiver) = oneshot::channel();
        self.shared
            .pending
            .lock()
            .expect("pending map poisoned")
            .insert(id.clone(), sender);

        let mut frame = serde_json::Map::new();
        frame.insert("id".into(), id.to_value());
        frame.insert("method".into(), Value::String(method.to_string()));
        if let Some(params) = params {
            frame.insert("params".into(), params);
        }
        if let Err(error) = self.shared.send_frame(Value::Object(frame)) {
            self.shared
                .pending
                .lock()
                .expect("pending map poisoned")
                .remove(&id);
            return Err(error);
        }

        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(result)) => result,
            // sender 被丢弃：连接已关闭且清理路径先失败了这个请求。
            Ok(Err(_)) => Err(ClientError::Closed),
            Err(_) => {
                self.shared
                    .pending
                    .lock()
                    .expect("pending map poisoned")
                    .remove(&id);
                Err(ClientError::Timeout {
                    method: method.to_string(),
                })
            }
        }
    }

    /// 主动关闭：失败所有未完成请求并中止读写任务。
    pub fn dispose(&self) {
        self.shared.close_with(ClientError::Closed);
        let tasks: Vec<AbortHandle> = self
            .shared
            .tasks
            .lock()
            .expect("task list poisoned")
            .drain(..)
            .collect();
        for task in tasks {
            task.abort();
        }
    }
}

async fn writer_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut out: mpsc::UnboundedReceiver<Option<Value>>,
    shared: Arc<Shared>,
) {
    while let Some(message) = out.recv().await {
        let Some(value) = message else {
            // 优雅关闭：先 flush 已排队的帧，再 drop writer（stdio 子进程收到 EOF）。
            let _ = writer.flush().await;
            return;
        };
        let mut bytes = match serde_json::to_vec(&value) {
            Ok(bytes) => bytes,
            Err(error) => {
                shared.close_with(ClientError::Protocol(format!("serialize failed: {error}")));
                return;
            }
        };
        bytes.push(b'\n');
        if let Err(error) = writer.write_all(&bytes).await {
            shared.close_with(ClientError::Transport(error.to_string()));
            return;
        }
        if let Err(error) = writer.flush().await {
            shared.close_with(ClientError::Transport(error.to_string()));
            return;
        }
    }
}

async fn reader_loop<R: AsyncRead + Unpin>(reader: R, shared: Arc<Shared>) {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) => {
                shared.close_with(ClientError::Closed);
                return;
            }
            Ok(_) => {}
            Err(error) => {
                shared.close_with(ClientError::Transport(error.to_string()));
                return;
            }
        }
        if line.len() > MAX_FRAME_BYTES {
            shared.close_with(ClientError::Protocol(format!(
                "frame exceeds {MAX_FRAME_BYTES} bytes"
            )));
            return;
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(error) => {
                shared.close_with(ClientError::Protocol(format!("invalid frame: {error}")));
                return;
            }
        };
        handle_inbound(&shared, value);
        if shared.is_closed() {
            return;
        }
    }
}

fn handle_inbound(shared: &Arc<Shared>, value: Value) {
    let Some(object) = value.as_object() else {
        shared.close_with(ClientError::Protocol("frame is not an object".into()));
        return;
    };
    let id = object.get("id").and_then(RequestId::from_value);
    let method = object.get("method").and_then(Value::as_str);

    if let Some(id) = &id {
        if object.contains_key("result") {
            if let Some(sender) = shared
                .pending
                .lock()
                .expect("pending map poisoned")
                .remove(id)
            {
                let _ = sender.send(Ok(object.get("result").cloned().unwrap_or(Value::Null)));
            }
            return;
        }
        if let Some(error) = object.get("error") {
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            let data = error.get("data").cloned();
            if let Some(sender) = shared
                .pending
                .lock()
                .expect("pending map poisoned")
                .remove(id)
            {
                let _ = sender.send(Err(ClientError::Server {
                    code,
                    message,
                    data,
                }));
            }
            return;
        }
        if let Some(method) = method {
            respond_to_request(shared, id, method, object.get("params"));
            return;
        }
        shared.close_with(ClientError::Protocol(
            "frame with id has neither result, error nor method".into(),
        ));
        return;
    }

    if method.is_some() {
        let _ = shared.notifications.send(value);
        return;
    }
    shared.close_with(ClientError::Protocol(
        "frame has neither id nor method".into(),
    ));
}

fn respond_to_request(shared: &Arc<Shared>, id: &RequestId, method: &str, params: Option<&Value>) {
    let handler = shared
        .request_handler
        .lock()
        .expect("handler poisoned")
        .clone();
    let reply = handler.and_then(|handler| {
        let request = json!({
            "id": id.to_value(),
            "method": method,
            "params": params.cloned().unwrap_or(Value::Null),
        });
        handler(&request)
    });
    let frame = match reply {
        Some(result) => json!({ "id": id.to_value(), "result": result }),
        None => json!({
            "id": id.to_value(),
            "error": { "code": -32601, "message": "Method not found" }
        }),
    };
    if let Err(error) = shared.send_frame(frame) {
        shared.close_with(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_round_trip_over_duplex() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let client = ProtocolClient::connect(client_read, client_write, ClientOptions::default());

        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).await.unwrap();
            let request: Value = serde_json::from_slice(&line[..line.len() - 1]).unwrap();
            assert_eq!(request["method"], "workspaceHookTrustGrant");
            let reply = json!({ "id": request["id"], "result": { "accepted": true } });
            server_write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        });

        let result = client
            .request(
                "workspaceHookTrustGrant",
                Some(json!({ "bundleDigest": "abc" })),
            )
            .await
            .unwrap();
        assert_eq!(result["accepted"], true);
        server.await.unwrap();
        client.dispose();
    }

    #[tokio::test]
    async fn server_error_maps_to_server_variant() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let client = ProtocolClient::connect(client_read, client_write, ClientOptions::default());
        tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).await.unwrap();
            let request: Value = serde_json::from_slice(&line[..line.len() - 1]).unwrap();
            let reply = json!({
                "id": request["id"],
                "error": { "code": -32000, "message": "boom" }
            });
            server_write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        });

        let error = client.request("x", None).await.unwrap_err();
        assert_eq!(
            error,
            ClientError::Server {
                code: -32000,
                message: "boom".into(),
                data: None
            }
        );
        client.dispose();
    }

    #[tokio::test]
    async fn notifications_and_server_requests_are_multiplexed() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let client = ProtocolClient::connect(client_read, client_write, ClientOptions::default());
        let mut notifications = client.subscribe_notifications();
        client.set_request_handler(Arc::new(|request: &Value| {
            assert_eq!(request["method"], "interaction/ask");
            Some(json!({ "answer": 42 }))
        }));

        tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            let mut line = Vec::new();
            // 先发通知，再发服务端请求，最后回客户端请求的结果。
            server_write
                .write_all(b"{\"method\":\"startup/storageState\",\"params\":{\"state\":\"ready\"}}\n")
                .await
                .unwrap();
            server_write
                .write_all(b"{\"id\":\"srv-1\",\"method\":\"interaction/ask\",\"params\":{}}\n")
                .await
                .unwrap();
            reader.read_until(b'\n', &mut line).await.unwrap();
            let response: Value = serde_json::from_slice(&line[..line.len() - 1]).unwrap();
            assert_eq!(response["result"]["answer"], 42);
            line.clear();
            reader.read_until(b'\n', &mut line).await.unwrap();
            let request: Value = serde_json::from_slice(&line[..line.len() - 1]).unwrap();
            let reply = json!({ "id": request["id"], "result": "ok" });
            server_write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .unwrap();
        });

        let notification = notifications.recv().await.unwrap();
        assert_eq!(notification["method"], "startup/storageState");
        assert_eq!(client.request("v4/anything", None).await.unwrap(), "ok");
        client.dispose();
    }

    #[tokio::test]
    async fn frames_split_across_chunks_and_unicode_line_separators_do_not_split() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let client = ProtocolClient::connect(client_read, client_write, ClientOptions::default());
        let mut notifications = client.subscribe_notifications();

        tokio::spawn(async move {
            let _ = server_read;
            let frame = "{\"method\":\"n\",\"params\":\"a\u{2028}b\u{2029}c\"}\n";
            let bytes = frame.as_bytes();
            for chunk in bytes.chunks(7) {
                server_write.write_all(chunk).await.unwrap();
            }
            // 保持连接存活，等待客户端读取。
            tokio::time::sleep(Duration::from_millis(200)).await;
        });

        let notification = notifications.recv().await.unwrap();
        assert_eq!(notification["params"], "a\u{2028}b\u{2029}c");
        client.dispose();
    }

    #[tokio::test]
    async fn timeout_fails_only_the_timed_out_request() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (_server_read, _server_write) = tokio::io::split(server_io);
        let client = ProtocolClient::connect(client_read, client_write, ClientOptions::default());

        let error = client
            .request_with_timeout("slow", None, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(error, ClientError::Timeout { method: "slow".into() });
        assert_eq!(client.pending_count(), 0);
        client.dispose();
    }

    #[tokio::test]
    async fn eof_fails_pending_requests_with_closed() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (server_read, server_write) = tokio::io::split(server_io);
        let client = ProtocolClient::connect(client_read, client_write, ClientOptions::default());

        // 对端读完请求后关闭连接，不回帧。
        tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            let mut line = Vec::new();
            let _ = reader.read_until(b'\n', &mut line).await;
            drop(server_write);
            drop(reader);
        });

        let error = client
            .request_with_timeout("orphan", None, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(error, ClientError::Closed);
        assert!(client.is_closed());
    }

    #[tokio::test]
    async fn unknown_method_from_server_gets_method_not_found() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let client = ProtocolClient::connect(client_read, client_write, ClientOptions::default());

        let server = tokio::spawn(async move {
            server_write
                .write_all(b"{\"id\":7,\"method\":\"unknown\"}\n")
                .await
                .unwrap();
            let mut reader = BufReader::new(server_read);
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).await.unwrap();
            let response: Value = serde_json::from_slice(&line[..line.len() - 1]).unwrap();
            assert_eq!(response["id"], 7);
            assert_eq!(response["error"]["code"], -32601);
        });
        server.await.unwrap();
        client.dispose();
    }

    #[tokio::test]
    async fn close_write_rejects_new_requests_but_keeps_reading() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_io);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let client = ProtocolClient::connect(client_read, client_write, ClientOptions::default());
        let mut notifications = client.subscribe_notifications();

        client.close_write();
        assert_eq!(
            client.request("after-close", None).await.unwrap_err(),
            ClientError::Closed
        );

        // 写端关闭不影响读端：对端仍可推送通知。
        tokio::spawn(async move {
            let _ = server_read;
            server_write
                .write_all(b"{\"method\":\"late\",\"params\":1}\n")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        let notification = notifications.recv().await.unwrap();
        assert_eq!(notification["method"], "late");
        client.dispose();
    }
}
