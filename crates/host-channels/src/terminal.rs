//! `terminal` 频道适配器：profile/create/write/resize/dispose/disposeByKey +
//! `onDynamicData`/`onDynamicExit` 事件。
//!
//! 行为契约见 `docs/specs/terminal-service.md`。PTY 由 `terminal-service` 内部的
//! `terminal-session` 管理器唯一持有；本适配器只做参数解析、阻塞调用调度与事件桥接。

use std::sync::{Arc, Mutex};

use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
use rpc_wire::Value;
use settings_store::SettingsService;
use terminal_service::{TerminalEnvironment, TerminalService};

use crate::params::first_arg_object;

pub const PROFILE: &str = "profile";
pub const CREATE: &str = "create";
pub const WRITE: &str = "write";
pub const RESIZE: &str = "resize";
pub const DISPOSE: &str = "dispose";
pub const DISPOSE_BY_KEY: &str = "disposeByKey";
pub const ON_DYNAMIC_DATA: &str = "onDynamicData";
pub const ON_DYNAMIC_EXIT: &str = "onDynamicExit";

pub struct TerminalChannel {
    service: Arc<TerminalService>,
}

impl TerminalChannel {
    pub fn new(service: Arc<TerminalService>) -> Self {
        Self { service }
    }
}

/// 生产构造：profile 的字体/主题继承读取同一个 `setting.json` 服务。
pub fn default_service(settings: Arc<SettingsService>) -> Arc<TerminalService> {
    Arc::new(TerminalService::new(TerminalEnvironment::from_process(
        Arc::new(move || settings.get()),
    )))
}

impl ServerChannel for TerminalChannel {
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
                    ChannelError::error(format!("terminal channel task failed: {error}"))
                })?
        })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        arg: Value,
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        let id = string_arg(&arg)
            .ok_or_else(|| ChannelError::error(format!("{event} requires a terminal id")))?;
        match event.as_str() {
            ON_DYNAMIC_DATA => {
                let subscription = Mutex::new(Some(self.service.subscribe_data(
                    &id,
                    Arc::new(move |data: &str| emit(Value::Str(data.to_string()))),
                )));
                Ok(Box::new(move || {
                    if let Ok(mut guard) = subscription.lock() {
                        drop(guard.take());
                    }
                }))
            }
            ON_DYNAMIC_EXIT => {
                let subscription = Mutex::new(Some(self.service.subscribe_exit(
                    &id,
                    Arc::new(move |code: i64| emit(Value::Json(code.to_string()))),
                )));
                Ok(Box::new(move || {
                    if let Ok(mut guard) = subscription.lock() {
                        drop(guard.take());
                    }
                }))
            }
            other => Err(ChannelError::error(format!(
                "terminal channel: unknown event {other}"
            ))),
        }
    }
}

fn run(service: &TerminalService, command: &str, arg: &Value) -> Result<Value, ChannelError> {
    match command {
        PROFILE => {
            let params = first_arg_object(arg)?;
            let cwd = optional_string(&params, "cwd")?;
            let value = service
                .profile(cwd.as_deref())
                .map_err(ChannelError::error)?;
            Ok(json(&value))
        }
        CREATE => {
            let params = first_arg_object(arg)?;
            let cols = required_u16(&params, "cols")?;
            let rows = required_u16(&params, "rows")?;
            let cwd = optional_string(&params, "cwd")?;
            let session_key = optional_string(&params, "sessionKey")?;
            let value = service
                .create(cols, rows, cwd.as_deref(), session_key.as_deref())
                .map_err(ChannelError::error)?;
            Ok(json(&value))
        }
        WRITE => {
            let params = first_arg_object(arg)?;
            let id = required_string(&params, "id")?;
            let data = required_string(&params, "data")?;
            service.write(&id, &data).map_err(ChannelError::error)?;
            Ok(Value::Undefined)
        }
        RESIZE => {
            let params = first_arg_object(arg)?;
            let id = required_string(&params, "id")?;
            let cols = required_u16(&params, "cols")?;
            let rows = required_u16(&params, "rows")?;
            service
                .resize(&id, cols, rows)
                .map_err(ChannelError::error)?;
            Ok(Value::Undefined)
        }
        DISPOSE => {
            let params = first_arg_object(arg)?;
            let id = required_string(&params, "id")?;
            service.dispose(&id).map_err(ChannelError::error)?;
            Ok(Value::Undefined)
        }
        DISPOSE_BY_KEY => {
            let params = first_arg_object(arg)?;
            let key = required_string(&params, "sessionKey")?;
            service.dispose_by_key(&key).map_err(ChannelError::error)?;
            Ok(Value::Undefined)
        }
        other => Err(ChannelError::error(format!(
            "terminal channel: unknown command {other}"
        ))),
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
        .ok_or_else(|| ChannelError::error(format!("{key} is required")))
}

fn optional_string(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<String>, ChannelError> {
    match params.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(ChannelError::error(format!("{key} must be a string"))),
    }
}

fn required_u16(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<u16, ChannelError> {
    params
        .get(key)
        .and_then(|value| value.as_u64())
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| ChannelError::error(format!("{key} must be a u16")))
}

/// 动态事件的 arg 是终端 id（裸值或位置数组的首项）。
fn string_arg(arg: &Value) -> Option<String> {
    let value = match arg {
        Value::Array(items) => items.first(),
        other => Some(other),
    };
    match value {
        Some(Value::Str(text)) => Some(text.clone()),
        Some(Value::Json(raw)) => serde_json::from_str::<String>(raw).ok(),
        _ => None,
    }
}

fn json(value: &serde_json::Value) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}
