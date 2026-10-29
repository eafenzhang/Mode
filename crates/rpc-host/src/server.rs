//! 壳内 RPC 服务端：把请求帧分派给注册的频道，并按 TS `ChannelServer` 的语义回帧。
//!
//! 对齐的行为（逐条对应 `packages/rpc/src/channelServer.ts`）：
//!
//! 1. 构造时发 `Initialize`（`deferInit` 时可延后到 [`ChannelServer::ready`]）；
//! 2. `Promise` → `PromiseSuccess`；JS `Error` 形态的错误 → `PromiseError`（message/name/
//!    stack + 透传字段）；非 Error 的拒绝值 → `PromiseErrorObj`（原样）；
//! 3. `EventListen` → 每次事件发 `EventFire`；`EventDispose` / `PromiseCancel` 释放对应 id；
//! 4. 频道尚未注册时请求进 pending；频道注册后立即补发；等到超时仍没有频道，`Promise`
//!    收到一条「Unknown channel」错误（`EventListen` 只留诊断，不回帧）；
//! 5. `dispose` 释放全部订阅与取消令牌。
//!
//! 与 TS 的差异（有意为之，均不影响客户端）：错误回帧的 JSON 键序按字典序（TS 按写入序），
//! 客户端只做 JSON 解析，与键序无关。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rpc_wire::{
    decode_rpc_frame, encode_rpc_response, RequestHeader, ResponseHeader, RpcFrame, Value,
};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::channel::{CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription};

enum Active {
    Cancel(CancellationToken),
    Subscription(Subscription),
}

enum PendingKind {
    Promise,
    EventListen,
}

struct PendingRequest {
    id: i32,
    kind: PendingKind,
    channel: String,
    name: String,
    arg: Value,
    timer: Option<JoinHandle<()>>,
}

pub struct ChannelServer {
    out: UnboundedSender<Vec<u8>>,
    ctx: String,
    timeout: Duration,
    channels: HashMap<String, Arc<dyn ServerChannel>>,
    active: Arc<Mutex<HashMap<i32, Active>>>,
    pending: HashMap<String, Vec<PendingRequest>>,
}

impl ChannelServer {
    /// `defer_init` 对应 TS 的同名参数：`true` 时不立刻发 `Initialize`。
    pub fn new(
        out: UnboundedSender<Vec<u8>>,
        ctx: impl Into<String>,
        timeout: Duration,
        defer_init: bool,
    ) -> Self {
        let server = Self {
            out,
            ctx: ctx.into(),
            timeout,
            channels: HashMap::new(),
            active: Arc::new(Mutex::new(HashMap::new())),
            pending: HashMap::new(),
        };
        if !defer_init {
            server.send_response(ResponseHeader::Initialize, Value::Undefined);
        }
        server
    }

    pub fn ready(&self) {
        self.send_response(ResponseHeader::Initialize, Value::Undefined);
    }

    /// 注册频道，并把等待该频道的 pending 请求补发。
    pub fn register_channel(&mut self, name: impl Into<String>, channel: Arc<dyn ServerChannel>) {
        let name = name.into();
        self.channels.insert(name.clone(), channel);
        let Some(requests) = self.pending.remove(&name) else {
            return;
        };
        for mut pending in requests {
            if let Some(timer) = pending.timer.take() {
                timer.abort();
            }
            match pending.kind {
                PendingKind::Promise => self.dispatch_promise(pending),
                PendingKind::EventListen => self.dispatch_listen(pending),
            }
        }
    }

    /// 处理一条 RPC payload（不含 stdio 长度前缀）。
    pub fn handle_frame(&mut self, frame: &[u8]) {
        let Ok(RpcFrame::Request(message)) = decode_rpc_frame(frame) else {
            // 回帧与无法解析的帧都不属于服务端入口；TS 侧同样忽略。
            return;
        };
        match message.header {
            RequestHeader::Promise {
                id,
                channel,
                method,
            } => self.on_promise(id, channel, method, message.body),
            RequestHeader::EventListen {
                id,
                channel,
                event,
            } => self.on_listen(id, channel, event, message.body),
            RequestHeader::PromiseCancel { id } | RequestHeader::EventDispose { id } => {
                self.dispose_active(id);
            }
        }
    }

    pub fn dispose(&mut self) {
        for (_, requests) in self.pending.drain() {
            for mut pending in requests {
                if let Some(timer) = pending.timer.take() {
                    timer.abort();
                }
            }
        }
        let mut active = self.active.lock().expect("active map poisoned");
        for (_, entry) in active.drain() {
            match entry {
                Active::Cancel(token) => token.cancel(),
                Active::Subscription(dispose) => dispose(),
            }
        }
    }

    fn on_promise(&mut self, id: i32, channel: String, name: String, arg: Value) {
        let request = PendingRequest {
            id,
            kind: PendingKind::Promise,
            channel,
            name,
            arg,
            timer: None,
        };
        if !self.channels.contains_key(&request.channel) {
            self.collect_pending(request);
            return;
        }
        self.dispatch_promise(request);
    }

    fn on_listen(&mut self, id: i32, channel: String, name: String, arg: Value) {
        let request = PendingRequest {
            id,
            kind: PendingKind::EventListen,
            channel,
            name,
            arg,
            timer: None,
        };
        if !self.channels.contains_key(&request.channel) {
            self.collect_pending(request);
            return;
        }
        self.dispatch_listen(request);
    }

    fn dispatch_promise(&mut self, request: PendingRequest) {
        let Some(channel) = self.channels.get(&request.channel).cloned() else {
            self.collect_pending(request);
            return;
        };
        let cancel = CancellationToken::default();
        self.active
            .lock()
            .expect("active map poisoned")
            .insert(request.id, Active::Cancel(cancel.clone()));

        let out = self.out.clone();
        let active = Arc::clone(&self.active);
        let id = request.id;
        let ctx = self.ctx.clone();
        tokio::spawn(async move {
            let result = channel.call(ctx, request.name, request.arg, cancel).await;
            let frame = match result {
                Ok(value) => encode_response(ResponseHeader::PromiseSuccess { id }, value),
                Err(ChannelError::Error {
                    message,
                    name,
                    stack,
                    extra,
                }) => encode_response(
                    ResponseHeader::PromiseError { id },
                    Value::Json(error_payload(&message, &name, stack.as_deref(), &extra)),
                ),
                Err(ChannelError::Value(value)) => {
                    encode_response(ResponseHeader::PromiseErrorObj { id }, value)
                }
            };
            let _ = out.send(frame);
            active.lock().expect("active map poisoned").remove(&id);
        });
    }

    fn dispatch_listen(&mut self, request: PendingRequest) {
        let Some(channel) = self.channels.get(&request.channel).cloned() else {
            self.collect_pending(request);
            return;
        };
        let out = self.out.clone();
        let id = request.id;
        let emit: EventCallback = Arc::new(move |data: Value| {
            let _ = out.send(encode_response(ResponseHeader::EventFire { id }, data));
        });
        // TS 在这里没有 try/catch（异常会冒泡）。Rust 侧显式忽略：没有订阅就没有事件，
        // 也不伪造回帧。
        if let Ok(dispose) = channel.listen(
            self.ctx.clone(),
            request.name,
            request.arg,
            emit,
        ) {
            self.active
                .lock()
                .expect("active map poisoned")
                .insert(id, Active::Subscription(dispose));
        }
    }

    fn collect_pending(&mut self, request: PendingRequest) {
        let out = self.out.clone();
        let id = request.id;
        let channel_name = request.channel.clone();
        let is_promise = matches!(&request.kind, PendingKind::Promise);
        let timeout = self.timeout;
        let timer = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            // 与 TS 的 `console.error` 同位置：超时仍是 pending 就先留痕，再决定是否回错。
            eprintln!("Unknown channel: {channel_name}");
            if !is_promise {
                return;
            }
            let payload = error_payload(
                &format!(
                    "Channel name '{}' timed out after {}ms",
                    channel_name,
                    timeout.as_millis()
                ),
                "Unknown channel",
                None,
                &[],
            );
            let _ = out.send(encode_response(
                ResponseHeader::PromiseError { id },
                Value::Json(payload),
            ));
        });
        let entry = PendingRequest {
            timer: Some(timer),
            ..request
        };
        self.pending.entry(entry.channel.clone()).or_default().push(entry);
    }

    fn dispose_active(&mut self, id: i32) {
        let entry = self.active.lock().expect("active map poisoned").remove(&id);
        match entry {
            Some(Active::Cancel(token)) => token.cancel(),
            Some(Active::Subscription(dispose)) => dispose(),
            None => {}
        }
    }

    fn send_response(&self, header: ResponseHeader, body: Value) {
        let _ = self.out.send(encode_response(header, body));
    }
}

fn encode_response(header: ResponseHeader, body: Value) -> Vec<u8> {
    encode_rpc_response(&rpc_wire::RpcResponse { header, body })
}

/// 构造 TS `PromiseError` 的 body：`{message,name,stack?,...extra}`。
///
/// `stack` 为 `None` 时**不写该键**（TS 里 `stack: undefined` 会被 JSON.stringify 丢弃）。
fn error_payload(
    message: &str,
    name: &str,
    stack: Option<&[String]>,
    extra: &[(String, Value)],
) -> String {
    let mut map = serde_json::Map::new();
    map.insert("message".into(), serde_json::Value::String(message.into()));
    map.insert("name".into(), serde_json::Value::String(name.into()));
    if let Some(stack) = stack {
        map.insert(
            "stack".into(),
            serde_json::Value::Array(
                stack
                    .iter()
                    .map(|line| serde_json::Value::String(line.clone()))
                    .collect(),
            ),
        );
    }
    for (key, value) in extra {
        map.insert(key.clone(), to_json(value));
    }
    serde_json::Value::Object(map).to_string()
}

fn to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Undefined => serde_json::Value::Null,
        Value::Str(text) => serde_json::Value::String(text.clone()),
        Value::Int(number) => serde_json::Value::from(*number),
        Value::Array(items) => serde_json::Value::Array(items.iter().map(to_json).collect()),
        Value::Json(text) => serde_json::from_str(text).unwrap_or(serde_json::Value::Null),
        Value::Bytes(bytes) | Value::Buffer(bytes) => serde_json::json!({
            "__mode_rpc_nested_uint8array_v1": true,
            "base64": base64_encode(bytes),
        }),
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}
