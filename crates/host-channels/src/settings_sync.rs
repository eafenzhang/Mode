//! `settings-sync` 频道适配器：6 个方法。
//!
//! 参数形状是 `ProxyChannel` 的位置参数数组（第一个元素为对象）；服务是同步文件实现，
//! 统一丢进 `spawn_blocking`，避免阻塞 RPC 执行器。
//!
//! 行为契约见 `docs/specs/settings-sync-service.md`。

use std::sync::Arc;

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Deserialize;
use settings_sync_service::{DetectParams, ImportParams, SettingsSyncService};

use crate::params::first_arg_object;

pub const GET_CLAUDE_AGENTS_FILE_MIGRATION_STATUS: &str = "getClaudeAgentsFileMigrationStatus";
pub const COPY_CLAUDE_AGENTS_FILE_TO_MODE_AGENTS_FILE: &str =
    "copyClaudeAgentsFileToModeAgentsFile";
pub const DETECT: &str = "detect";
pub const IMPORT_SELECTED: &str = "importSelected";
pub const GET_FIRST_RUN_PROMPT_STATE: &str = "getFirstRunPromptState";
pub const MARK_FIRST_RUN_PROMPT_HANDLED: &str = "markFirstRunPromptHandled";

pub struct SettingsSyncChannel {
    service: Arc<SettingsSyncService>,
}

impl SettingsSyncChannel {
    pub fn new(service: Arc<SettingsSyncService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for SettingsSyncChannel {
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
                    ChannelError::error(format!("settings-sync channel task failed: {error}"))
                })?
        })
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct CopyAgentsFileArgs {
    overwrite: Option<bool>,
}

fn run(service: &SettingsSyncService, command: &str, arg: &Value) -> Result<Value, ChannelError> {
    match command {
        GET_CLAUDE_AGENTS_FILE_MIGRATION_STATUS => Ok(json_value(
            &serde_json::to_value(service.get_claude_agents_file_migration_status())
                .map_err(|error| ChannelError::error(error.to_string()))?,
        )),
        COPY_CLAUDE_AGENTS_FILE_TO_MODE_AGENTS_FILE => {
            let args: CopyAgentsFileArgs = parse(arg)?;
            Ok(json_value(
                &serde_json::to_value(
                    service.copy_claude_agents_file_to_mode_agents_file(
                        args.overwrite.unwrap_or(false),
                    ),
                )
                .map_err(|error| ChannelError::error(error.to_string()))?,
            ))
        }
        DETECT => {
            let params: DetectParams = parse(arg)?;
            Ok(json_value(
                &serde_json::to_value(service.detect(&params))
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            ))
        }
        IMPORT_SELECTED => {
            let params: ImportParams = parse(arg)?;
            Ok(json_value(
                &serde_json::to_value(service.import_selected(&params))
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            ))
        }
        GET_FIRST_RUN_PROMPT_STATE => {
            let state = service
                .get_first_run_prompt_state()
                .map_err(ChannelError::error)?;
            Ok(json_value(
                &serde_json::to_value(&state)
                    .map_err(|error| ChannelError::error(error.to_string()))?,
            ))
        }
        MARK_FIRST_RUN_PROMPT_HANDLED => {
            service
                .mark_first_run_prompt_handled()
                .map_err(ChannelError::error)?;
            Ok(Value::Undefined)
        }
        other => Err(ChannelError::error(format!(
            "settings-sync channel: unknown command {other}"
        ))),
    }
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = first_arg_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

fn json_value(value: &serde_json::Value) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}
