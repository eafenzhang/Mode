//! `conversation-mirror` 频道适配器。

use std::sync::Arc;

use conversation_mirror::ConversationMirrorService;
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

use crate::params::{first_arg_object, required_string};

pub const GET_IDENTITY: &str = "getMirrorIdentity";
pub const PUBLISH: &str = "publishSnapshot";
pub const WRITE_LOCAL: &str = "writeLocalSnapshot";
pub const READ_WORKSPACE: &str = "readWorkspaceSnapshots";

pub struct ConversationMirrorChannel {
    service: Arc<ConversationMirrorService>,
}

impl ConversationMirrorChannel {
    pub fn new(service: Arc<ConversationMirrorService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for ConversationMirrorChannel {
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
                GET_IDENTITY => {
                    let identity = run_blocking(move || service.get_mirror_identity()).await?;
                    identity
                        .map(|identity| to_json(&identity))
                        .map_err(ChannelError::error)
                }
                PUBLISH | WRITE_LOCAL => {
                    let params = first_arg_object(&arg)?;
                    let Some(snapshot) = params.get("snapshot") else {
                        return Err(ChannelError::error("snapshot is required"));
                    };
                    let snapshot = snapshot.clone();
                    let expect_local = command == WRITE_LOCAL;
                    let result = run_blocking(move || {
                        if expect_local {
                            service.write_local_snapshot(&snapshot)
                        } else {
                            service.publish_snapshot(&snapshot)
                        }
                    })
                    .await?;
                    Ok(to_json(&result))
                }
                READ_WORKSPACE => {
                    let params = first_arg_object(&arg)?;
                    let workspace_path = required_string(&params, "workspacePath")?;
                    let snapshots = run_blocking(move || {
                        service.read_workspace_snapshots(&workspace_path)
                    })
                    .await?;
                    let payload = serde_json::json!({ "snapshots": snapshots });
                    Ok(Value::Json(payload.to_string()))
                }
                other => Err(ChannelError::error(format!(
                    "conversation-mirror: unknown command {other}"
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
        .map_err(|error| ChannelError::error(format!("conversation-mirror task failed: {error}")))
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    Value::Json(serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()))
}
