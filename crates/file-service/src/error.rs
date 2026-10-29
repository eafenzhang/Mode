//! 服务层错误：消息即 RPC 错误文本（与 TS `Error.message` 对齐）。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileServiceError {
    pub message: String,
}

impl FileServiceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for FileServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for FileServiceError {}

pub type FileResult<T> = std::result::Result<T, FileServiceError>;

pub fn file_result(message: impl Into<String>) -> FileServiceError {
    FileServiceError::new(message)
}
