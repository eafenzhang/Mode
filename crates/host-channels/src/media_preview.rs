//! `media-preview` 频道适配器（本地宿主形态）。
//!
//! 真源：`packages/services/src/media-preview/mediaPreview.ts`。
//! 当前宿主**没有注入** `authorizeLocalMediaPreviewPath`（那是远端/Electron 的 local-url
//! 分支），所以本地实际走 **inline**：校验扩展名与 kind → stat → 超过 8 MiB 报错 →
//! 读全文并 base64。`refreshPlaybackUrl` / `release` 在 TS 里是可选方法（本地服务没有），
//! 这里同样按未知方法报错。
//!
//! 扩展名表来自 `packages/shared/src/media-preview.ts`，两边必须同步。

use std::fs;

use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

use crate::base64::encode_base64;
use crate::params::{first_arg_object, required_string};

pub const PREPARE: &str = "prepare";
pub const INLINE_MAX_BYTES: u64 = 8 * 1024 * 1024;

struct Format {
    extension: &'static str,
    kind: &'static str,
    media_type: &'static str,
}

const FORMATS: &[Format] = &[
    Format { extension: ".mp4", kind: "video", media_type: "video/mp4" },
    Format { extension: ".mov", kind: "video", media_type: "video/quicktime" },
    Format { extension: ".webm", kind: "video", media_type: "video/webm" },
    Format { extension: ".m4v", kind: "video", media_type: "video/x-m4v" },
    Format { extension: ".mp3", kind: "audio", media_type: "audio/mpeg" },
    Format { extension: ".wav", kind: "audio", media_type: "audio/wav" },
    Format { extension: ".m4a", kind: "audio", media_type: "audio/mp4" },
    Format { extension: ".ogg", kind: "audio", media_type: "audio/ogg" },
    Format { extension: ".opus", kind: "audio", media_type: "audio/opus" },
    Format { extension: ".flac", kind: "audio", media_type: "audio/flac" },
    Format { extension: ".weba", kind: "audio", media_type: "audio/webm" },
];

/// 与 TS 一致：反斜杠归一成 `/`，整体转小写，再按后缀匹配。
fn get_format(path: &str) -> Option<&'static Format> {
    let normalized = path.replace('\\', "/").to_lowercase();
    FORMATS
        .iter()
        .find(|format| normalized.ends_with(format.extension))
}

pub struct MediaPreviewChannel;

impl ServerChannel for MediaPreviewChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        Box::pin(async move {
            match command.as_str() {
                PREPARE => {
                    let params = first_arg_object(&arg)?;
                    let path = required_string(&params, "path")?;
                    let expected_kind = required_string(&params, "expectedKind")?;
                    let Some(format) = get_format(&path) else {
                        return Err(ChannelError::error(format!(
                            "Unsupported media preview format: {path}"
                        )));
                    };
                    if format.kind != expected_kind {
                        return Err(ChannelError::error(format!(
                            "Unsupported media preview format: {path}"
                        )));
                    }
                    let metadata = fs::metadata(&path).map_err(|error| {
                        ChannelError::error(format!("Cannot stat media file {path}: {error}"))
                    })?;
                    if !metadata.is_file() {
                        return Err(ChannelError::error(format!(
                            "Path is not a media file: {path}"
                        )));
                    }
                    let size = metadata.len();
                    if size > INLINE_MAX_BYTES {
                        return Err(ChannelError::error(format!(
                            "Media file is too large for inline preview: {path}"
                        )));
                    }
                    let bytes = fs::read(&path).map_err(|error| {
                        ChannelError::error(format!("Cannot read media file {path}: {error}"))
                    })?;
                    Ok(Value::Json(
                        serde_json::json!({
                            "kind": "inline",
                            "dataBase64": encode_base64(&bytes),
                            "mediaType": format.media_type,
                            "path": path,
                            "size": size,
                        })
                        .to_string(),
                    ))
                }
                other => Err(ChannelError::error(format!(
                    "media-preview: unknown command {other}"
                ))),
            }
        })
    }
}
