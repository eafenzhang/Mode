//! 服务层错误：`message` 即 RPC 错误文本（与 TS `Error.message` 对齐，例如
//! `Command file already exists: foo.md`、`Missing workspace path for project command`）。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandsServiceError {
    pub message: String,
}

impl CommandsServiceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CommandsServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandsServiceError {}

impl From<std::io::Error> for CommandsServiceError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub type CommandsResult<T> = std::result::Result<T, CommandsServiceError>;

pub fn commands_error(message: impl Into<String>) -> CommandsServiceError {
    CommandsServiceError::new(message)
}
