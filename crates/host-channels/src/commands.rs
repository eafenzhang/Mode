//! `commands` 频道适配器：6 个方法。
//!
//! 参数形状是 `ProxyChannel` 的位置参数数组（第一个元素为对象）；服务是同步实现
//! （小文件读写 + 跨进程锁），统一丢进 `spawn_blocking`，避免阻塞 RPC 执行器。

use std::sync::Arc;

use commands_service::{
    CommandCreateParams, CommandDeleteParams, CommandSetEnabledParams, CommandUpdateParams,
    CommandsService, CommandsServiceError, ListParams, PrimaryDirectoryParams,
};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Serialize;

use crate::params::first_arg_object;

pub const LIST: &str = "list";
pub const WRITE_COMMAND_FILE: &str = "writeCommandFile";
pub const UPDATE_COMMAND_FILE: &str = "updateCommandFile";
pub const DELETE_COMMAND_FILE: &str = "deleteCommandFile";
pub const SET_COMMAND_ENABLED: &str = "setCommandEnabled";
pub const GET_PRIMARY_USER_COMMANDS_DIRECTORY: &str = "getPrimaryUserCommandsDirectory";

pub struct CommandsChannel {
    service: Arc<CommandsService>,
}

impl CommandsChannel {
    pub fn new(service: Arc<CommandsService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for CommandsChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            let value = tokio::task::spawn_blocking(move || run(&service, &command, &arg))
                .await
                .map_err(|error| ChannelError::error(format!("commands channel task failed: {error}")))??;
            Ok(value)
        })
    }
}

fn run(service: &CommandsService, command: &str, arg: &Value) -> Result<Value, ChannelError> {
    match command {
        LIST => {
            let params: ListParams = parse(arg)?;
            Ok(json(&service.list(&params).chan()?))
        }
        WRITE_COMMAND_FILE => {
            let params: CommandCreateParams = parse(arg)?;
            Ok(json(&service.write_command_file(&params).chan()?))
        }
        UPDATE_COMMAND_FILE => {
            let params: CommandUpdateParams = parse(arg)?;
            Ok(json(&service.update_command_file(&params).chan()?))
        }
        DELETE_COMMAND_FILE => {
            let params: CommandDeleteParams = parse(arg)?;
            service.delete_command_file(&params).chan()?;
            Ok(Value::Undefined)
        }
        SET_COMMAND_ENABLED => {
            let params: CommandSetEnabledParams = parse(arg)?;
            service.set_command_enabled(&params).chan()?;
            Ok(Value::Undefined)
        }
        GET_PRIMARY_USER_COMMANDS_DIRECTORY => {
            let params: PrimaryDirectoryParams = parse(arg)?;
            Ok(json(&service.get_primary_user_commands_directory(&params).chan()?))
        }
        other => Err(ChannelError::error(format!(
            "commands channel: unknown command {other}"
        ))),
    }
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = first_arg_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

/// `CommandsServiceError` → `ChannelError`（跨 crate 无法实现 `From`，用局部扩展 trait）。
trait IntoChannel<T> {
    fn chan(self) -> Result<T, ChannelError>;
}

impl<T> IntoChannel<T> for commands_service::CommandsResult<T> {
    fn chan(self) -> Result<T, ChannelError> {
        self.map_err(map_error)
    }
}

fn json<T: Serialize>(value: &T) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}

fn map_error(error: CommandsServiceError) -> ChannelError {
    ChannelError::error(error.message)
}
