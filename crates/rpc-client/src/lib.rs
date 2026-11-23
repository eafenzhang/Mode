//! 壳内 RPC 客户端：把请求帧发给 `rpc_host::ChannelServer` 并按 id 分发回帧。
//!
//! 真源：`packages/rpc/src/channelClient.ts`。规范见
//! `docs/specs/rpc-client-and-lan-transport.md`。
//!
//! 为什么需要它：Rust 侧此前只有 [`rpc_host::ChannelServer`]（服务端），没有任何客户端，
//! 所以 LAN / SSH / WSL / Docker / v3 五种远程 transport 全部接不上——它们的共同缺口
//! 就是这一层。客户端补齐后，每种 transport 只需负责「把字节搬过去」。
//!
//! 三条容易写错、这里逐条钉住的语义：
//!
//! 1. **Initialize 门**：服务端在建连瞬间发一帧 `Initialize`，且可能与握手回包同批到达。
//!    在它之前发起的请求必须**排队**而不是丢弃，收到后按序补发。
//! 2. **重连重放**：第二次 `Initialize` 意味着换了服务端实例，旧订阅它不认识，
//!    必须按**原 id** 重放 `EventListen`。
//! 3. **dispose fail-closed**：所有在途请求（含还排在 Initialize 之前的）都要 reject，
//!    否则上层「in-flight 去重」会永久占住 workspace key。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use rpc_host::{CancellationToken, ChannelError, EventCallback};
use rpc_wire::{
    decode_rpc_response, encode_rpc_message, RequestHeader, ResponseHeader, RpcMessage, Value,
};

/// 请求帧类型（TS `RequestType`）。
pub mod request_type {
    pub const PROMISE: i32 = 100;
    pub const PROMISE_CANCEL: i32 = 101;
    pub const EVENT_LISTEN: i32 = 102;
    pub const EVENT_DISPOSE: i32 = 103;
}

/// 客户端 → 服务端的帧写出通道（transport 提供）。
pub type FrameSink = Arc<dyn Fn(Vec<u8>) + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    /// 客户端已释放（对应 TS `ChannelClient is disposed`）。
    Disposed,
    /// 请求被取消。
    Cancelled,
    /// 服务端返回 `PromiseError`：`{message,name,stack?,...extra}`。
    Remote {
        message: String,
        name: String,
        stack: Option<Vec<String>>,
        extra: Vec<(String, Value)>,
    },
    /// 服务端返回 `PromiseErrorObj`：非 Error 的拒绝值。
    RemoteValue(Value),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disposed => write!(formatter, "ChannelClient is disposed"),
            Self::Cancelled => write!(formatter, "Cancelled"),
            Self::Remote { message, .. } => write!(formatter, "{message}"),
            Self::RemoteValue(_) => write!(formatter, "远程调用失败（非 Error 拒绝值）"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<ClientError> for ChannelError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Disposed => ChannelError::error("ChannelClient is disposed"),
            ClientError::Cancelled => ChannelError::error("Cancelled"),
            ClientError::Remote {
                message,
                name,
                stack,
                extra,
            } => ChannelError::Error {
                message,
                name,
                stack,
                extra,
            },
            ClientError::RemoteValue(value) => ChannelError::Value(value),
        }
    }
}

/// 一个待决请求的完成回调（transport 无关，便于单测注入）。
pub type Responder = Arc<dyn Fn(Result<Value, ClientError>) + Send + Sync>;

struct PendingRequest {
    id: i32,
    channel: String,
    method: String,
    arg: Value,
    responder: Responder,
}

struct EventSubscription {
    id: i32,
    channel: String,
    event: String,
    emit: EventCallback,
    /// 是否已发出过 `EventListen`（重连时要重放）。
    sent: bool,
}

#[derive(Default)]
struct Inner {
    /// 已收到 `Initialize`（TS `state === Idle`）。
    initialized: bool,
    /// 曾经初始化过（用于区分「首次」与「重连」）。
    ever_initialized: bool,
    disposed: bool,
    /// Initialize 之前发起的请求，按发起顺序排队。
    queued: Vec<PendingRequest>,
    /// 已发出、等待回帧的请求。
    in_flight: BTreeMap<i32, Responder>,
    /// 事件订阅（含尚未发出的）。
    events: BTreeMap<i32, EventSubscription>,
}

/// RPC 客户端。`out` 是 transport 注入的帧写出函数。
///
/// 刻意不做成 async：客户端只做「id 分配 + 排队 + 分发」，
/// 真正的等待由调用方持有的 channel（oneshot 之类）承担。这样
/// 「迟到回帧」「dispose 时 fail-closed」这些不变式可以逐条断言。
pub struct ChannelClient {
    inner: Arc<Mutex<Inner>>,
    out: FrameSink,
    disposed_flag: Arc<AtomicBool>,
    next_id: AtomicI32,
}

impl ChannelClient {
    pub fn new(out: FrameSink) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            out,
            disposed_flag: Arc::new(AtomicBool::new(false)),
            next_id: AtomicI32::new(0),
        }
    }

    /// 当前是否已初始化（收到过 `Initialize`）。
    pub fn is_initialized(&self) -> bool {
        self.inner.lock().expect("client poisoned").initialized
    }

    pub fn is_disposed(&self) -> bool {
        self.disposed_flag.load(Ordering::SeqCst)
    }

    fn alloc_id(&self) -> i32 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// 发起一次 `Promise` 调用。
    ///
    /// 未初始化时**排队**（不是丢弃也不是报错）——这是 TS 那条注释的全部要点：
    /// 服务端的 `Initialize` 可能与握手同批到达，早发的请求必须等它。
    pub fn call(
        &self,
        channel: &str,
        method: &str,
        arg: Value,
        responder: Responder,
    ) -> (i32, CancellationToken) {
        let cancel = CancellationToken::default();
        if self.is_disposed() {
            responder(Err(ClientError::Disposed));
            return (self.alloc_id(), cancel);
        }
        let id = self.alloc_id();
        let request = PendingRequest {
            id,
            channel: channel.to_string(),
            method: method.to_string(),
            arg,
            responder,
        };

        let mut inner = self.inner.lock().expect("client poisoned");
        if inner.initialized {
            let frame = encode_request(request_type::PROMISE, id, channel, method, &request.arg);
            inner.in_flight.insert(id, Arc::clone(&request.responder));
            drop(inner);
            (self.out)(frame);
        } else {
            inner.queued.push(request);
        }
        (id, cancel)
    }

    /// 取消一个在途请求（TS `PromiseCancel`）。
    pub fn cancel(&self, id: i32) {
        let mut inner = self.inner.lock().expect("client poisoned");
        if let Some(responder) = inner.in_flight.remove(&id) {
            drop(inner);
            (self.out)(encode_cancel(request_type::PROMISE_CANCEL, id));
            responder(Err(ClientError::Cancelled));
            return;
        }
        // 还在队列里：直接摘掉并拒绝，不必发帧（服务端没见过它）。
        if let Some(index) = inner.queued.iter().position(|request| request.id == id) {
            let request = inner.queued.remove(index);
            drop(inner);
            (request.responder)(Err(ClientError::Cancelled));
        }
    }

    /// 注册事件订阅并立即发出 `EventListen`（已初始化时）。
    ///
    /// 返回释放函数：调用时发 `EventDispose`，否则服务端订阅永不释放。
    pub fn listen(
        &self,
        channel: &str,
        event: &str,
        emit: EventCallback,
    ) -> Box<dyn Fn() + Send + Sync> {
        let id = self.alloc_id();
        if self.is_disposed() {
            return Box::new(|| {});
        }
        let mut inner = self.inner.lock().expect("client poisoned");
        let initialized = inner.initialized;
        inner.events.insert(
            id,
            EventSubscription {
                id,
                channel: channel.to_string(),
                event: event.to_string(),
                emit,
                sent: false,
            },
        );
        drop(inner);

        if initialized {
            self.send_event_listen(id);
        }

        let out = Arc::clone(&self.out);
        let disposed = Arc::clone(&self.disposed_flag);
        let inner = Arc::clone(&self.inner);
        Box::new(move || {
            if disposed.load(Ordering::SeqCst) {
                return;
            }
            let mut guard = inner.lock().expect("client poisoned");
            let existed = guard.events.remove(&id).is_some();
            drop(guard);
            if existed {
                (out)(encode_cancel(request_type::EVENT_DISPOSE, id));
            }
        })
    }

    /// 处理一帧入站 payload（已剥掉传输层分帧）。
    pub fn handle_frame(&self, frame: &[u8]) {
        let Ok(response) = decode_rpc_response(frame) else {
            // 请求帧与本端回帧都不属于客户端入口（TS 同样忽略）。
            return;
        };
        match response.header {
            ResponseHeader::Initialize => self.on_initialize(),
            ResponseHeader::PromiseSuccess { id } => {
                self.settle(id, Ok(response.body));
            }
            ResponseHeader::PromiseError { id } => {
                self.settle(id, Err(decode_remote_error(&response.body)));
            }
            ResponseHeader::PromiseErrorObj { id } => {
                self.settle(id, Err(ClientError::RemoteValue(response.body)));
            }
            ResponseHeader::EventFire { id } => {
                let emit = {
                    let inner = self.inner.lock().expect("client poisoned");
                    inner.events.get(&id).map(|entry| Arc::clone(&entry.emit))
                };
                if let Some(emit) = emit {
                    emit(response.body);
                }
            }
        }
    }

    /// 释放客户端：**所有**在途请求 reject（含排队中的）。
    pub fn dispose(&self) {
        if self.disposed_flag.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut inner = self.inner.lock().expect("client poisoned");
        inner.disposed = true;
        let queued = std::mem::take(&mut inner.queued);
        let in_flight = std::mem::take(&mut inner.in_flight);
        inner.events.clear();
        drop(inner);
        for request in queued {
            (request.responder)(Err(ClientError::Disposed));
        }
        for (_, responder) in in_flight {
            responder(Err(ClientError::Disposed));
        }
    }

    fn on_initialize(&self) {
        let mut inner = self.inner.lock().expect("client poisoned");
        let reconnected = inner.ever_initialized;
        inner.initialized = true;
        inner.ever_initialized = true;
        let queued = std::mem::take(&mut inner.queued);
        let events_to_replay = if reconnected {
            inner
                .events
                .values_mut()
                .map(|entry| {
                    entry.sent = false;
                    entry.id
                })
                .collect::<Vec<i32>>()
        } else {
            Vec::new()
        };
        drop(inner);

        for request in queued {
            let frame = encode_request(
                request_type::PROMISE,
                request.id,
                &request.channel,
                &request.method,
                &request.arg,
            );
            {
                let mut inner = self.inner.lock().expect("client poisoned");
                inner
                    .in_flight
                    .insert(request.id, Arc::clone(&request.responder));
            }
            (self.out)(frame);
        }
        for id in events_to_replay {
            self.send_event_listen(id);
        }
    }

    fn send_event_listen(&self, id: i32) {
        let (channel, event, arg) = {
            let mut inner = self.inner.lock().expect("client poisoned");
            let Some(entry) = inner.events.get_mut(&id) else {
                return;
            };
            entry.sent = true;
            (entry.channel.clone(), entry.event.clone(), Value::Undefined)
        };
        (self.out)(encode_request(
            request_type::EVENT_LISTEN,
            id,
            &channel,
            &event,
            &arg,
        ));
    }

    fn settle(&self, id: i32, result: Result<Value, ClientError>) {
        let responder = {
            let mut inner = self.inner.lock().expect("client poisoned");
            inner.in_flight.remove(&id)
        };
        if let Some(responder) = responder {
            responder(result);
        }
    }
}

fn encode_request(kind: i32, id: i32, channel: &str, name: &str, arg: &Value) -> Vec<u8> {
    let header = match kind {
        request_type::PROMISE => RequestHeader::Promise {
            id,
            channel: channel.to_string(),
            method: name.to_string(),
        },
        _ => RequestHeader::EventListen {
            id,
            channel: channel.to_string(),
            event: name.to_string(),
        },
    };
    encode_rpc_message(&RpcMessage {
        header,
        body: arg.clone(),
    })
}

fn encode_cancel(kind: i32, id: i32) -> Vec<u8> {
    let header = match kind {
        request_type::PROMISE_CANCEL => RequestHeader::PromiseCancel { id },
        _ => RequestHeader::EventDispose { id },
    };
    encode_rpc_message(&RpcMessage {
        header,
        body: Value::Undefined,
    })
}

/// `PromiseError` body：`{message,name,stack?,...extra}`。
fn decode_remote_error(body: &Value) -> ClientError {
    let json = body.to_json();
    let object = json.as_object();
    let message = object
        .and_then(|value| value.get("message"))
        .and_then(|value| value.as_str())
        .unwrap_or("远程调用失败")
        .to_string();
    let name = object
        .and_then(|value| value.get("name"))
        .and_then(|value| value.as_str())
        .unwrap_or("Error")
        .to_string();
    let stack = object
        .and_then(|value| value.get("stack"))
        .and_then(|value| value.as_array())
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| line.as_str().map(str::to_string))
                .collect()
        });
    let mut extra = Vec::new();
    if let Some(object) = object {
        for (key, value) in object {
            if matches!(key.as_str(), "message" | "name" | "stack") {
                continue;
            }
            extra.push((key.clone(), json_to_wire(value)));
        }
    }
    ClientError::Remote {
        message,
        name,
        stack,
        extra,
    }
}

fn json_to_wire(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Undefined,
        serde_json::Value::Bool(flag) => Value::Int(i32::from(*flag)),
        serde_json::Value::Number(number) => number
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Value::Int)
            .unwrap_or(Value::Undefined),
        serde_json::Value::String(text) => Value::Str(text.clone()),
        other => Value::Json(other.to_string()),
    }
}
