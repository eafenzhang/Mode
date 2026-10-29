//! 壳内频道适配器。
//!
//! 只做「RPC 方法名 + 位置参数」到服务调用的翻译与错误映射；服务本身在各自的 crate 里。
//! 第一个适配的是 `memory`（两个只读方法，见 `docs/specs/project-memory-service.md`）。
//!
//! 参数形状与 `ProxyChannel` 一致：`arg` 是**位置参数数组**（`channel.call(ctx, method,
//! ...args)` 在协议层被包成数组）。对象参数以 `Value::Json` 承载。

use std::sync::Arc;

use project_memory::{MemoryError, ProjectMemoryService};
use rpc_host::{BoxFuture, ChannelError, ServerChannel};
use rpc_wire::Value;

pub const MEMORY_CHANNEL: &str = "memory";
pub const LIST_MEMORIES: &str = "listProjectMemories";
pub const READ_MEMORY_FILE: &str = "readProjectMemoryFile";

pub struct MemoryChannel {
    service: Arc<ProjectMemoryService>,
}

impl MemoryChannel {
    pub fn new(service: Arc<ProjectMemoryService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for MemoryChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: rpc_host::CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            match command.as_str() {
                LIST_MEMORIES => {
                    let result = run_blocking(move || service.list_project_memories()).await?;
                    result.map(json_value).map_err(map_error)
                }
                READ_MEMORY_FILE => {
                    let params = first_arg_object(&arg)?;
                    let workspace_id = required_string(&params, "workspaceId")?;
                    let file_name = required_string(&params, "fileName")?;
                    let result = run_blocking(move || {
                        service.read_project_memory_file(&workspace_id, &file_name)
                    })
                    .await?;
                    result.map(json_value).map_err(map_error)
                }
                other => Err(ChannelError::error(format!(
                    "memory channel: unknown command {other}"
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
        .map_err(|error| ChannelError::error(format!("memory task failed: {error}")))
}

fn json_value<T: serde::Serialize>(value: T) -> Value {
    match serde_json::to_string(&value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}

fn map_error(error: MemoryError) -> ChannelError {
    let mut mapped = ChannelError::error(error.to_string());
    if let Some(code) = error.code() {
        mapped = mapped.with_extra("code", Value::Str(code.to_string()));
    }
    mapped
}

/// 位置参数数组 → 第一个参数必须是对象。
fn first_arg_object(arg: &Value) -> Result<serde_json::Map<String, serde_json::Value>, ChannelError> {
    let first = match arg {
        Value::Array(items) => items.first().cloned(),
        Value::Undefined => None,
        other => Some(other.clone()),
    };
    let Some(first) = first else {
        return Err(ChannelError::error("memory channel: missing arguments"));
    };
    match first.to_json() {
        serde_json::Value::Object(object) => Ok(object),
        _ => Err(ChannelError::error(
            "memory channel: arguments must be an object",
        )),
    }
}

fn required_string(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, ChannelError> {
    params
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .ok_or_else(|| ChannelError::error(format!("memory channel: {key} is required")))
}
