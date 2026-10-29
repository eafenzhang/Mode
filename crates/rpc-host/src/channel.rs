//! 频道抽象：服务端实现 `ServerChannel`，由 [`crate::ChannelServer`] 分派。
//!
//! 语义对齐 TS `IServerChannel`：`call` 返回结果或错误，`listen` 注册事件并返回一个
//! 释放函数。区别只在 Rust 侧显式建模取消令牌，不再用 JS 的 CancellationTokenSource。

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rpc_wire::Value;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub type EventCallback = Arc<dyn Fn(Value) + Send + Sync>;
pub type Subscription = Box<dyn Fn() + Send + Sync>;

/// 取消令牌：`Promise` 请求被 `PromiseCancel` 取消时置位。
#[derive(Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

/// 频道返回的错误。
///
/// TS 把 `Error` 实例映射成 `PromiseError`（带 message/name/stack 与若干透传字段），
/// 其他值映射成 `PromiseErrorObj`（原样回传）。这里用两个变体表达同一区别。
#[derive(Debug, Clone, PartialEq)]
pub enum ChannelError {
    /// 对应 JS `Error`：会编码成 `{message,name,stack,...extra}`。
    Error {
        message: String,
        name: String,
        stack: Option<Vec<String>>,
        extra: Vec<(String, Value)>,
    },
    /// 对应非 Error 的拒绝值：原样作为回帧 body。
    Value(Value),
}

impl ChannelError {
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
            name: "Error".into(),
            stack: None,
            extra: Vec::new(),
        }
    }

    pub fn with_extra(mut self, key: impl Into<String>, value: Value) -> Self {
        if let Self::Error { extra, .. } = &mut self {
            extra.push((key.into(), value));
        }
        self
    }
}

/// 一个可被 RPC 调用的频道。
pub trait ServerChannel: Send + Sync + 'static {
    fn call(
        &self,
        ctx: String,
        command: String,
        arg: Value,
        cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>>;

    /// 订阅事件；返回释放函数（`EventDispose` 或服务端释放时调用一次）。
    fn listen(
        &self,
        _ctx: String,
        event: String,
        _arg: Value,
        _emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        Err(ChannelError::error(format!("event {event} is not supported")))
    }
}
