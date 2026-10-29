use std::fmt;

/// TS 侧通过 `Object.assign(new Error(...), { kind })` 附加的错误分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksDbErrorKind {
    ChecksumMismatch,
    LockTimeout,
}

#[derive(Debug)]
pub struct TasksDbError {
    pub kind: Option<TasksDbErrorKind>,
    pub message: String,
    /// TS 失败路径附加的当前迁移 id（`databaseStartupErrorDetails` 允许透传）。
    pub migration_id: Option<String>,
    /// `prepare_tasks_storage` 失败时附带的当时迁移 facts。
    pub startup_migration: Option<Box<crate::MigrationFacts>>,
    /// SQLite 扩展错误码（Node `error.errcode` 口径）。
    pub sqlite_code: Option<i32>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl TasksDbError {
    pub(crate) fn checksum_mismatch(migration_id: &str) -> Self {
        Self {
            kind: Some(TasksDbErrorKind::ChecksumMismatch),
            message: format!("Task database migration checksum mismatch: {migration_id}"),
            migration_id: Some(migration_id.to_owned()),
            startup_migration: None,
            sqlite_code: None,
            source: None,
        }
    }

    pub(crate) fn lock_timeout(cause: TasksDbError) -> Self {
        Self {
            kind: Some(TasksDbErrorKind::LockTimeout),
            message: "Task storage lock wait expired".to_owned(),
            migration_id: cause.migration_id.clone(),
            startup_migration: None,
            sqlite_code: cause.sqlite_code,
            source: Some(Box::new(cause)),
        }
    }

    pub(crate) fn with_migration_id(mut self, migration_id: &'static str) -> Self {
        if self.migration_id.is_none() {
            self.migration_id = Some(migration_id.to_owned());
        }
        self
    }

    /// 仅 `SQLITE_BUSY`（errcode & 0xff == 5）按 TS 的重试口径处理。
    pub(crate) fn is_busy(&self) -> bool {
        self.sqlite_code
            .is_some_and(|code| (code & 0xff) == 5)
    }
}

impl fmt::Display for TasksDbError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TasksDbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|error| error as &(dyn std::error::Error + 'static))
    }
}

impl From<rusqlite::Error> for TasksDbError {
    fn from(error: rusqlite::Error) -> Self {
        let sqlite_code = match &error {
            rusqlite::Error::SqliteFailure(failure, _) => Some(failure.extended_code),
            _ => None,
        };
        Self {
            kind: None,
            message: error.to_string(),
            migration_id: None,
            startup_migration: None,
            sqlite_code,
            source: Some(Box::new(error)),
        }
    }
}

impl From<std::io::Error> for TasksDbError {
    fn from(error: std::io::Error) -> Self {
        Self {
            kind: None,
            message: error.to_string(),
            migration_id: None,
            startup_migration: None,
            sqlite_code: None,
            source: Some(Box::new(error)),
        }
    }
}
