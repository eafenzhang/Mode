//! `git-checkpoint` 频道适配器：4 个方法。
//!
//! 参数形状是 `ProxyChannel` 的位置参数数组（第一个元素为对象）；checkpoint 涉及真实
//! Git 命令与文件 IO，统一丢进 `spawn_blocking`。
//!
//! 行为契约见 `docs/specs/git-checkpoint-service.md`。

use std::path::PathBuf;
use std::sync::Arc;

use git_checkpoint_service::GitCheckpointService;
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Deserialize;

use crate::params::first_arg_object;

pub const CREATE_CHECKPOINT: &str = "createCheckpoint";
pub const DIFF_CHECKPOINTS: &str = "diffCheckpoints";
pub const RESTORE_BETWEEN_CHECKPOINTS: &str = "restoreBetweenCheckpoints";
pub const DELETE_CHECKPOINT: &str = "deleteCheckpoint";

pub struct GitCheckpointChannel {
    service: Arc<GitCheckpointService>,
}

impl GitCheckpointChannel {
    pub fn new(service: Arc<GitCheckpointService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for GitCheckpointChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run(&service, &command, &arg))
                .await
                .map_err(|error| {
                    ChannelError::error(format!("git-checkpoint channel task failed: {error}"))
                })?
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceArgs {
    workspace_path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffQueryArgs {
    workspace_path: PathBuf,
    from_checkpoint_id: String,
    to_checkpoint_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreQueryArgs {
    workspace_path: PathBuf,
    from_checkpoint_id: String,
    to_checkpoint_id: String,
    #[serde(default)]
    force: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteArgs {
    workspace_path: PathBuf,
    checkpoint_id: String,
}

fn run(service: &GitCheckpointService, command: &str, arg: &Value) -> Result<Value, ChannelError> {
    match command {
        CREATE_CHECKPOINT => {
            let args: WorkspaceArgs = parse(arg)?;
            let meta = service
                .create_checkpoint(&args.workspace_path)
                .map_err(ChannelError::error)?;
            Ok(to_value(&meta))
        }
        DIFF_CHECKPOINTS => {
            let args: DiffQueryArgs = parse(arg)?;
            let diff = service
                .diff_checkpoints(
                    &args.workspace_path,
                    &args.from_checkpoint_id,
                    &args.to_checkpoint_id,
                )
                .map_err(ChannelError::error)?;
            Ok(to_value(&diff))
        }
        RESTORE_BETWEEN_CHECKPOINTS => {
            let args: RestoreQueryArgs = parse(arg)?;
            let result = service
                .restore_between_checkpoints(
                    &args.workspace_path,
                    &args.from_checkpoint_id,
                    &args.to_checkpoint_id,
                    args.force.unwrap_or(false),
                )
                .map_err(ChannelError::error)?;
            Ok(to_value(&result))
        }
        DELETE_CHECKPOINT => {
            let args: DeleteArgs = parse(arg)?;
            service
                .delete_checkpoint(&args.workspace_path, &args.checkpoint_id)
                .map_err(ChannelError::error)?;
            Ok(Value::Undefined)
        }
        other => Err(ChannelError::error(format!(
            "git-checkpoint channel: unknown command {other}"
        ))),
    }
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = first_arg_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}

/// 供壳装配：默认环境探测 + 指定数据根。
pub fn default_service(data_root: PathBuf) -> Arc<GitCheckpointService> {
    git_checkpoint_service::default_service(data_root)
}
