//! RPC 位置参数解析（`ProxyChannel` 把 `channel.call(ctx, method, ...args)` 包成数组）。

use rpc_host::ChannelError;
use rpc_wire::Value;

/// 取第一个位置参数并要求它是对象。
pub(crate) fn first_arg_object(
    arg: &Value,
) -> Result<serde_json::Map<String, serde_json::Value>, ChannelError> {
    let first = match arg {
        Value::Array(items) => items.first().cloned(),
        Value::Undefined => None,
        other => Some(other.clone()),
    };
    let Some(first) = first else {
        return Err(ChannelError::error("missing arguments"));
    };
    match first.to_json() {
        serde_json::Value::Object(object) => Ok(object),
        _ => Err(ChannelError::error("arguments must be an object")),
    }
}

pub(crate) fn required_string(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, ChannelError> {
    params
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| ChannelError::error(format!("{key} is required")))
}
