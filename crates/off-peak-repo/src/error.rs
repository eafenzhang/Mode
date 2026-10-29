use std::fmt;

/// 闲时任务仓库错误。
#[derive(Debug)]
pub enum OffPeakRepoError {
    /// 连接尚未通过 `ensure_ready` 建立。
    NotInitialized,
    /// 业务校验错误：文案与 TS `throw new Error(...)` 一致。
    Message(String),
    /// 任务库迁移 / schema 错误。
    TasksDb(tasks_db::TasksDbError),
    /// SQLite 读写错误。
    Sqlite(rusqlite::Error),
    /// JSON 序列化错误。
    Json(serde_json::Error),
    /// 文件系统错误。
    Io(std::io::Error),
}

impl fmt::Display for OffPeakRepoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => {
                formatter.write_str("OffPeakTaskRepo 未初始化：请先 await ensureReady()")
            }
            Self::Message(message) => formatter.write_str(message),
            Self::TasksDb(error) => formatter.write_str(&error.to_string()),
            Self::Sqlite(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for OffPeakRepoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TasksDb(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<tasks_db::TasksDbError> for OffPeakRepoError {
    fn from(error: tasks_db::TasksDbError) -> Self {
        Self::TasksDb(error)
    }
}

impl From<rusqlite::Error> for OffPeakRepoError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for OffPeakRepoError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<std::io::Error> for OffPeakRepoError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, OffPeakRepoError>;
