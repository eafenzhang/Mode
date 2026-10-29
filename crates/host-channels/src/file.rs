//! `file` 频道适配器：17 个方法。
//!
//! 参数形状是 `ProxyChannel` 的位置参数数组（第一个元素为对象）；`readFileRange` 必须返回
//! **顶层二进制**（`Value::Bytes`），否则大文件分段加载会退化成 JSON+base64。

use std::sync::Arc;

use file_service::{
    CheckFilesExistParams, FileRangeParams, FileService, FileServiceError, IgnoreTransformParams,
    PathParams, PreviewParams, ReaddirParams, ReadTextFileParams, RootParams, RootRangeParams,
    ScratchWorkspaceParams, WorkspaceFileSearchParams, WriteIgnoreParams,
};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Serialize;

use crate::params::first_arg_object;

pub const SEARCH_WORKSPACE_FILES: &str = "searchWorkspaceFiles";
pub const READDIR: &str = "readdir";
pub const STAT: &str = "stat";
pub const CHECK_FILES_EXIST: &str = "checkFilesExist";
pub const RESOLVE_PATH: &str = "resolvePath";
pub const ENSURE_CONVERSATION_WORKSPACE: &str = "ensureConversationWorkspace";
pub const CREATE_DEFAULT_WORKSPACE: &str = "createDefaultWorkspace";
pub const CREATE_SCRATCH_WORKSPACE: &str = "createScratchWorkspace";
pub const READ_TEXT_FILE: &str = "readTextFile";
pub const READ_MEDIA_PREVIEW: &str = "readMediaPreview";
pub const READ_FILE_RANGE: &str = "readFileRange";
pub const READ_BINARY_PREVIEW: &str = "readBinaryPreview";
pub const LIST_WORKSPACE_FILES_LENGTH: &str = "listWorkspaceFilesLength";
pub const LIST_WORKSPACE_FILES_RANGE: &str = "listWorkspaceFilesRange";
pub const READ_WORKSPACE_FILE_SEARCH_IGNORE: &str = "readWorkspaceFileSearchIgnore";
pub const APPLY_WORKSPACE_FILE_SEARCH_IGNORE_TRANSFORM: &str =
    "applyWorkspaceFileSearchIgnoreTransform";
pub const WRITE_WORKSPACE_FILE_SEARCH_IGNORE: &str = "writeWorkspaceFileSearchIgnore";

pub struct FileChannel {
    service: Arc<FileService>,
}

impl FileChannel {
    pub fn new(service: Arc<FileService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for FileChannel {
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
                SEARCH_WORKSPACE_FILES => {
                    let params: WorkspaceFileSearchParams = parse(&arg)?;
                    let entries = service.search_workspace_files(&params).await.chan()?;
                    Ok(json(&entries))
                }
                READDIR => {
                    let params: ReaddirParams = parse(&arg)?;
                    Ok(json(&service.readdir(&params).await.chan()?))
                }
                STAT => {
                    let params: PathParams = parse(&arg)?;
                    Ok(json(&service.stat(&params).await.chan()?))
                }
                CHECK_FILES_EXIST => {
                    let params: CheckFilesExistParams = parse(&arg)?;
                    Ok(json(&service.check_files_exist(&params).await.chan()?))
                }
                RESOLVE_PATH => {
                    let params: PathParams = parse(&arg)?;
                    Ok(Value::Str(service.resolve_path(&params).await.chan()?))
                }
                ENSURE_CONVERSATION_WORKSPACE => {
                    Ok(json(&service.ensure_conversation_workspace().await.chan()?))
                }
                CREATE_DEFAULT_WORKSPACE => Ok(json(&service.create_default_workspace().await.chan()?)),
                CREATE_SCRATCH_WORKSPACE => {
                    let params: ScratchWorkspaceParams = parse(&arg)?;
                    Ok(json(&service.create_scratch_workspace(&params).await.chan()?))
                }
                READ_TEXT_FILE => {
                    let params: ReadTextFileParams = parse(&arg)?;
                    Ok(json(&service.read_text_file(&params).await.chan()?))
                }
                READ_MEDIA_PREVIEW => {
                    let params: PreviewParams = parse(&arg)?;
                    Ok(json(&service.read_media_preview(&params).await.chan()?))
                }
                READ_FILE_RANGE => {
                    let params: FileRangeParams = parse(&arg)?;
                    service
                        .read_file_range(&params)
                        .await
                        .map(Value::Bytes)
                        .map_err(map_error)
                }
                READ_BINARY_PREVIEW => {
                    let params: PreviewParams = parse(&arg)?;
                    Ok(json(&service.read_binary_preview(&params).await.chan()?))
                }
                LIST_WORKSPACE_FILES_LENGTH => {
                    let params: RootParams = parse(&arg)?;
                    let length = service.list_workspace_files_length(&params).await.chan()?;
                    Ok(int_or_json(length))
                }
                LIST_WORKSPACE_FILES_RANGE => {
                    let params: RootRangeParams = parse(&arg)?;
                    service
                        .list_workspace_files_range(&params)
                        .await
                        .map(Value::Str)
                        .map_err(map_error)
                }
                READ_WORKSPACE_FILE_SEARCH_IGNORE => {
                    let params: RootParams = parse(&arg)?;
                    Ok(json(&service.read_workspace_file_search_ignore(&params).await.chan()?))
                }
                APPLY_WORKSPACE_FILE_SEARCH_IGNORE_TRANSFORM => {
                    let params: IgnoreTransformParams = parse(&arg)?;
                    Ok(json(
                        &service
                            .apply_workspace_file_search_ignore_transform(&params)
                            .await.chan()?,
                    ))
                }
                WRITE_WORKSPACE_FILE_SEARCH_IGNORE => {
                    let params: WriteIgnoreParams = parse(&arg)?;
                    service
                        .write_workspace_file_search_ignore(&params)
                        .await.chan()?;
                    Ok(Value::Undefined)
                }
                other => Err(ChannelError::error(format!(
                    "file channel: unknown command {other}"
                ))),
            }
        })
    }
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = first_arg_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

/// `FileServiceError` → `ChannelError`（跨 crate 无法实现 `From`，用局部扩展 trait）。
trait IntoChannel<T> {
    fn chan(self) -> Result<T, ChannelError>;
}

impl<T> IntoChannel<T> for file_service::FileResult<T> {
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

/// RPC 的 Int 是 i32；超长 packed 用 JSON 数字（渲染层按 number 处理）。
fn int_or_json(value: usize) -> Value {
    match i32::try_from(value) {
        Ok(number) => Value::Int(number),
        Err(_) => Value::Json(value.to_string()),
    }
}

fn map_error(error: FileServiceError) -> ChannelError {
    ChannelError::error(error.message)
}
