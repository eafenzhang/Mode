//! `system` 频道适配器。

use std::sync::Arc;

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use system_service::{IntranetProbeRequest, SystemService};

use crate::params::first_arg_object;

pub const INFO: &str = "info";
pub const LIST_INTEGRATED_TERMINAL_SHELLS: &str = "listIntegratedTerminalShells";
pub const PROBE_INTRANET: &str = "probeIntranet";

pub struct SystemChannel {
    service: Arc<SystemService>,
}

impl SystemChannel {
    pub fn new(service: Arc<SystemService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for SystemChannel {
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
                INFO => Ok(json_value(&service.info())),
                LIST_INTEGRATED_TERMINAL_SHELLS => {
                    Ok(json_value(&service.list_integrated_terminal_shells().await))
                }
                PROBE_INTRANET => {
                    let params = first_arg_object(&arg)?;
                    let request: IntranetProbeRequest =
                        serde_json::from_value(serde_json::Value::Object(params)).map_err(
                            |error| {
                                ChannelError::error(format!(
                                    "invalid probeIntranet request: {error}"
                                ))
                            },
                        )?;
                    Ok(json_value(&service.probe_intranet(&request).await))
                }
                other => Err(ChannelError::error(format!(
                    "system channel: unknown command {other}"
                ))),
            }
        })
    }
}

fn json_value<T: serde::Serialize>(value: &T) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}
