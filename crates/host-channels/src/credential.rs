//! `credential` 频道适配器。
//!
//! 四个方法都是同步文件 IO（还要等跨进程锁），统一丢进 `spawn_blocking`，
//! 避免阻塞 RPC 消费任务。
//!
//! 错误映射与 TS 对齐：解密失败带 `code = MODE_CREDENTIAL_DECRYPT_FAILED`，
//! 消息保留「凭据解密失败：」前缀（渲染层两种识别方式都能命中）。

use std::sync::Arc;

use credential_store::{CredentialError, CredentialStore};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

pub const LOAD: &str = "load";
pub const SAVE: &str = "save";
pub const DELETE: &str = "delete";
pub const LIST: &str = "list";

pub struct CredentialChannel {
    store: Arc<CredentialStore>,
}

impl CredentialChannel {
    pub fn new(store: Arc<CredentialStore>) -> Self {
        Self { store }
    }
}

impl ServerChannel for CredentialChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let store = Arc::clone(&self.store);
        Box::pin(async move {
            match command.as_str() {
                LOAD => {
                    let key = required_arg(&arg, 0, "key")?;
                    let value = run_blocking(move || store.load(&key)).await?;
                    value.map(json_value).map_err(map_error)
                }
                SAVE => {
                    let key = required_arg(&arg, 0, "key")?;
                    let value = required_arg(&arg, 1, "value")?;
                    let result = run_blocking(move || store.save(&key, &value)).await?;
                    result.map(|()| Value::Undefined).map_err(map_error)
                }
                DELETE => {
                    let key = required_arg(&arg, 0, "key")?;
                    let result = run_blocking(move || store.delete(&key)).await?;
                    result.map(|()| Value::Undefined).map_err(map_error)
                }
                LIST => {
                    let prefix = required_arg(&arg, 0, "prefix")?;
                    let keys = run_blocking(move || store.list(&prefix)).await?;
                    keys.map(|keys| Value::Array(keys.into_iter().map(Value::Str).collect()))
                        .map_err(map_error)
                }
                other => Err(ChannelError::error(format!(
                    "credential channel: unknown command {other}"
                ))),
            }
        })
    }
}

async fn run_blocking<T, F>(task: F) -> Result<T, ChannelError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| ChannelError::error(format!("credential task failed: {error}")))
}

/// `load` 要返回 `null`（不是 `undefined`）：`Value::Undefined` 会被编码成 JS
/// `undefined`，而 TS 的 `Promise<string | null>` 是 `null`。
fn json_value<T: serde::Serialize>(value: T) -> Value {
    match serde_json::to_string(&value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}

fn map_error(error: CredentialError) -> ChannelError {
    let mut mapped = ChannelError::error(error.to_string());
    if let Some(code) = error.code() {
        mapped = mapped.with_extra("code", Value::Str(code.to_string()));
    }
    mapped
}

/// 位置参数：`ProxyChannel.toService` 把方法入参原样包成数组。
fn required_arg(arg: &Value, index: usize, name: &str) -> Result<String, ChannelError> {
    let value = match arg {
        Value::Array(items) => items.get(index),
        // 单参数调用可能被压成裸值（与本 crate 其它频道保持一致）。
        other if index == 0 => Some(other),
        _ => None,
    };
    match value {
        Some(Value::Str(text)) => Ok(text.clone()),
        Some(Value::Json(raw)) => serde_json::from_str::<String>(raw)
            .map_err(|_| ChannelError::error(format!("{name} must be a string"))),
        _ => Err(ChannelError::error(format!("{name} is required"))),
    }
}
