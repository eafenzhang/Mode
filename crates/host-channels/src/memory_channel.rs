//! `memory` 频道适配器。

use std::sync::Arc;

use project_memory::{MemoryError, ProjectMemoryService};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

use crate::params::{first_arg_object, required_string};

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
        _cancel: CancellationToken,
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
