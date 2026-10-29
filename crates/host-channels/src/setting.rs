//! `setting` 频道适配器。

use std::path::PathBuf;
use std::sync::Arc;

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use settings_store::SettingsService;

use crate::params::first_arg_object;

pub const GET: &str = "get";
pub const UPDATE: &str = "update";
pub const UPDATE_DATA_BASE_DIR: &str = "updateDataBaseDir";
pub const ENSURE_DEFAULT_PROJECT: &str = "ensureDefaultProject";

pub struct SettingChannel {
    service: Arc<SettingsService>,
    home: PathBuf,
}

impl SettingChannel {
    pub fn new(service: Arc<SettingsService>, home: PathBuf) -> Self {
        Self { service, home }
    }
}

impl ServerChannel for SettingChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let service = Arc::clone(&self.service);
        let home = self.home.clone();
        Box::pin(async move {
            match command.as_str() {
                GET => {
                    let value = tokio::task::spawn_blocking(move || service.get())
                        .await
                        .map_err(|error| ChannelError::error(format!("settings task failed: {error}")))?;
                    Ok(Value::Json(value.to_string()))
                }
                UPDATE => {
                    let patch = first_arg_object(&arg)?;
                    let patch = serde_json::Value::Object(patch);
                    let result = tokio::task::spawn_blocking(move || service.update(&patch))
                        .await
                        .map_err(|error| ChannelError::error(format!("settings task failed: {error}")))?;
                    result.map(|_| Value::Undefined).map_err(|error| ChannelError::error(error.to_string()))
                }
                UPDATE_DATA_BASE_DIR => {
                    let new_dir = positional_string(&arg);
                    let result = tokio::task::spawn_blocking(move || {
                        service.update_data_base_dir(new_dir.as_deref())
                    })
                    .await
                    .map_err(|error| ChannelError::error(format!("settings task failed: {error}")))?;
                    result
                        .map(|_| Value::Undefined)
                        .map_err(|error| ChannelError::error(error.to_string()))
                }
                ENSURE_DEFAULT_PROJECT => {
                    let target = positional_string(&arg).map(PathBuf::from).unwrap_or(home);
                    let result = tokio::task::spawn_blocking(move || {
                        settings_store::ensure_default_project(&target)
                    })
                    .await
                    .map_err(|error| ChannelError::error(format!("settings task failed: {error}")))?;
                    let project = result.map_err(|error| ChannelError::error(error.to_string()))?;
                    Ok(Value::Json(
                        serde_json::json!({
                            "path": project.path.to_string_lossy(),
                            "created": project.created,
                        })
                        .to_string(),
                    ))
                }
                other => Err(ChannelError::error(format!("setting: unknown command {other}"))),
            }
        })
    }
}

/// 位置参数里的第一个字符串；`undefined`/`null` 视为「未提供」。
fn positional_string(arg: &Value) -> Option<String> {
    match arg {
        Value::Array(items) => match items.first() {
            Some(Value::Str(text)) => Some(text.clone()),
            Some(Value::Json(raw)) => serde_json::from_str::<String>(raw).ok(),
            _ => None,
        },
        Value::Str(text) => Some(text.clone()),
        _ => None,
    }
}
