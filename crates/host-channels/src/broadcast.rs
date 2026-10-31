//! `broadcast` 频道适配器：5 个方法 + `onMessage` 事件。
//!
//! Tauri 线的 Host 以 `parentPort === null` 构造广播服务（没有跨 Host 中转），
//! 这里按同一 local-only 语义实现：`send` 只分发给本窗口订阅者。

use std::sync::Arc;

use broadcast_service::BroadcastService;
use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
use rpc_wire::Value;
use serde::Deserialize;

use crate::params::first_arg_object;

pub const SEND: &str = "send";
pub const ACQUIRE_CLAIM: &str = "acquireClaim";
pub const COMMIT_CLAIM: &str = "commitClaim";
pub const RELEASE_CLAIM: &str = "releaseClaim";
pub const TRY_CLAIM: &str = "tryClaim";
pub const ON_MESSAGE: &str = "onMessage";

#[derive(Deserialize)]
struct LeaseArgs {
    key: String,
    token: String,
}

pub struct BroadcastChannel {
    service: Arc<BroadcastService>,
}

impl BroadcastChannel {
    pub fn new(service: Arc<BroadcastService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for BroadcastChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            match command.as_str() {
                SEND => {
                    let message = first_arg_value(&arg)
                        .ok_or_else(|| ChannelError::error("missing arguments"))?;
                    service.send(&message).map_err(ChannelError::error)?;
                    Ok(Value::Undefined)
                }
                ACQUIRE_CLAIM => {
                    let key = first_arg_string(&arg)
                        .ok_or_else(|| ChannelError::error("acquireClaim requires a key"))?;
                    let result = service.acquire_claim(&key);
                    Ok(json(
                        &serde_json::to_value(&result)
                            .map_err(|error| ChannelError::error(error.to_string()))?,
                    ))
                }
                COMMIT_CLAIM => {
                    let lease: LeaseArgs = parse(&arg)?;
                    service.commit_claim(&broadcast_service::ClaimLease {
                        key: lease.key,
                        token: lease.token,
                    });
                    Ok(Value::Undefined)
                }
                RELEASE_CLAIM => {
                    let lease: LeaseArgs = parse(&arg)?;
                    service.release_claim(&broadcast_service::ClaimLease {
                        key: lease.key,
                        token: lease.token,
                    });
                    Ok(Value::Undefined)
                }
                TRY_CLAIM => {
                    let key = first_arg_string(&arg)
                        .ok_or_else(|| ChannelError::error("tryClaim requires a key"))?;
                    Ok(json(&serde_json::Value::Bool(service.try_claim(&key))))
                }
                other => Err(ChannelError::error(format!(
                    "broadcast channel: unknown command {other}"
                ))),
            }
        })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        _arg: Value,
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        if event != ON_MESSAGE {
            return Err(ChannelError::error(format!(
                "broadcast channel: unknown event {event}"
            )));
        }
        let subscription = self.service.subscribe(Arc::new(move |message| {
            if let Ok(text) = serde_json::to_string(message) {
                emit(Value::Json(text));
            }
        }));
        // `Subscription` 是 `Fn`：用 `Mutex<Option<_>>` 让回调可以幂等地取走并释放。
        let subscription = std::sync::Mutex::new(Some(subscription));
        Ok(Box::new(move || {
            if let Some(subscription) = subscription.lock().unwrap().take() {
                drop(subscription);
            }
        }))
    }
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = first_arg_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

fn first_arg_value(arg: &Value) -> Option<serde_json::Value> {
    let value = match arg {
        Value::Array(items) => items.first().cloned(),
        Value::Undefined => None,
        other => Some(other.clone()),
    }?;
    Some(match value {
        Value::Str(text) => serde_json::Value::String(text),
        other => other.to_json(),
    })
}

fn first_arg_string(arg: &Value) -> Option<String> {
    let value = match arg {
        Value::Array(items) => items.first().cloned(),
        Value::Undefined => None,
        other => Some(other.clone()),
    }?;
    match value {
        Value::Str(text) => Some(text),
        Value::Json(raw) => serde_json::from_str::<String>(&raw).ok(),
        _ => None,
    }
}

fn json(value: &serde_json::Value) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}
