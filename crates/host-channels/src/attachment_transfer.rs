//! `prompt-attachment-transfer` 频道适配器（本地宿主形态）。
//!
//! 真源：`packages/services/src/prompt-attachment-transfer/promptAttachmentTransferService.ts`。
//! 本地 workspace 保持 `localPath` **零拷贝**：`stage` 只回报字节数与引用，不做上传；
//! `adopt` / `cancel` / `cleanup` 是 no-op；`onDynamicProgress` 永不 emit
//! （只有远端的 wrapper 才会产生上传进度）。

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel, Subscription};
use rpc_wire::Value;

use crate::params::{first_arg_object, required_string};

pub const STAGE: &str = "stage";
pub const ADOPT: &str = "adopt";
pub const CANCEL: &str = "cancel";
pub const CLEANUP: &str = "cleanup";
pub const PROGRESS_EVENT: &str = "onDynamicProgress";

pub struct AttachmentTransferChannel;

impl ServerChannel for AttachmentTransferChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        Box::pin(async move {
            match command.as_str() {
                STAGE => {
                    let params = first_arg_object(&arg)?;
                    let operation_id = required_string(&params, "operationId")?;
                    let local_path = required_string(&params, "localPath")?;
                    let bytes = params
                        .get("sizeBytes")
                        .and_then(serde_json::Value::as_u64)
                        .filter(|size| *size > 0)
                        .unwrap_or_else(|| {
                            std::fs::metadata(&local_path)
                                .map(|metadata| metadata.len())
                                .unwrap_or(0)
                        });
                    Ok(Value::Json(
                        serde_json::json!({
                            "operationId": operation_id,
                            "ref": local_path,
                            "bytes": bytes,
                            "staged": false,
                        })
                        .to_string(),
                    ))
                }
                // 本地零拷贝没有暂存态，三个生命周期方法都是 no-op。
                ADOPT | CANCEL | CLEANUP => Ok(Value::Undefined),
                other => Err(ChannelError::error(format!(
                    "prompt-attachment-transfer: unknown command {other}"
                ))),
            }
        })
    }

    fn listen(
        &self,
        _ctx: String,
        event: String,
        _arg: Value,
        _emit: rpc_host::EventCallback,
    ) -> Result<Subscription, ChannelError> {
        if event == PROGRESS_EVENT {
            Ok(Box::new(|| {}))
        } else {
            Err(ChannelError::error(format!(
                "prompt-attachment-transfer: unknown event {event}"
            )))
        }
    }
}
