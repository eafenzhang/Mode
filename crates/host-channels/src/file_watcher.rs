//! `file-watcher` 频道适配器：三个方法 + `onDynamicChange` 事件。

use std::sync::Arc;

use file_watcher::{FileWatchEvent, FileWatcherError, FileWatcherService};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription};
use rpc_wire::Value;
use serde::Deserialize;

use crate::params::first_arg_object;

pub const WATCH: &str = "watch";
pub const UNWATCH: &str = "unwatch";
pub const DISPOSE_ALL: &str = "disposeAll";
pub const ON_DYNAMIC_CHANGE: &str = "onDynamicChange";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WatchParams {
    path: String,
    recursive: Option<bool>,
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

pub struct FileWatcherChannel {
    service: Arc<FileWatcherService>,
}

impl FileWatcherChannel {
    pub fn new(service: Arc<FileWatcherService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for FileWatcherChannel {
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
                WATCH => {
                    let params: WatchParams = parse(&arg)?;
                    let id = service
                        .watch(&params.path, params.recursive.unwrap_or(false))
                        .map_err(map_error)?;
                    Ok(json(&serde_json::json!({ "id": id })))
                }
                UNWATCH => {
                    let params: IdParams = parse(&arg)?;
                    service.unwatch(&params.id);
                    Ok(Value::Undefined)
                }
                DISPOSE_ALL => {
                    service.dispose_all();
                    Ok(Value::Undefined)
                }
                other => Err(ChannelError::error(format!(
                    "file-watcher channel: unknown command {other}"
                ))),
            }
        })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        arg: Value,
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        if event != ON_DYNAMIC_CHANGE {
            return Err(ChannelError::error(format!(
                "file-watcher channel: unknown event {event}"
            )));
        }
        let id = string_arg(&arg)
            .ok_or_else(|| ChannelError::error("onDynamicChange requires a watcher id"))?;
        let listener = Arc::new(move |change: &FileWatchEvent| {
            if let Ok(text) = serde_json::to_string(change) {
                emit(Value::Json(text));
            }
        });
        // 失效 id 与 TS 一致：不抛到订阅链路，返回一个永不触发的事件流。
        Ok(self
            .service
            .subscribe(&id, listener)
            .unwrap_or_else(|| Box::new(|| {})))
    }
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = first_arg_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

/// 动态事件的 arg 是 watcher id（裸值或位置数组的首项）。
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

fn json<T: serde::Serialize>(value: &T) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}

fn map_error(error: FileWatcherError) -> ChannelError {
    ChannelError::error(error.message)
}
