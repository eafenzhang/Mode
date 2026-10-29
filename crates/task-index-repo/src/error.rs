use std::fmt;

/// 任务索引仓库错误。
///
/// 业务错误文案与 TS 保持一致（例如 `task index 中不存在 task: {taskId}`），
/// 便于上层直接透传诊断信息。
#[derive(Debug)]
pub enum TaskIndexRepoError {
    /// 连接尚未通过 `ensure_ready` 建立。
    NotInitialized,
    /// 目标 task 不存在（或已删除），对应 TS 抛出的 Error。
    TaskNotFound(String),
    /// 任务库迁移 / schema 错误。
    TasksDb(tasks_db::TasksDbError),
    /// SQLite 读写错误。
    Sqlite(rusqlite::Error),
    /// JS 兼容序列化错误（正常路径不会发生）。
    Json(serde_json::Error),
    /// 文件系统错误。
    Io(std::io::Error),
}

impl fmt::Display for TaskIndexRepoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized => formatter.write_str("task index sqlite 尚未初始化"),
            Self::TaskNotFound(task_id) => {
                write!(formatter, "task index 中不存在 task: {task_id}")
            }
            Self::TasksDb(error) => formatter.write_str(&error.to_string()),
            Self::Sqlite(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for TaskIndexRepoError {
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

impl From<tasks_db::TasksDbError> for TaskIndexRepoError {
    fn from(error: tasks_db::TasksDbError) -> Self {
        Self::TasksDb(error)
    }
}

impl From<rusqlite::Error> for TaskIndexRepoError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for TaskIndexRepoError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<std::io::Error> for TaskIndexRepoError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, TaskIndexRepoError>;
