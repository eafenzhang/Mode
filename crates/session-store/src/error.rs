//! `session-store` 的错误面。
//!
//! `code()` 是给 `startup/storageState` 与 `agent-server` 用的稳定错误码；spec
//! `agent-core-parity.md` §6 要求写事务在 `busy_timeout` 耗尽时报 `storage_locked`，
//! 迁移账本缺失/不匹配时报 `storage_schema_unsupported`。

use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum StoreError {
    /// 数据根未迁移、缺表、缺账本或账本为空。C1 不建表，直接 fail-closed。
    StorageSchemaUnsupported {
        reason: String,
    },
    /// SQLite 写事务在 `busy_timeout` 内拿不到写锁（另一个连接持有写事务）。
    StorageLocked {
        path: PathBuf,
    },
    /// 调用方参数与库内事实不一致（例如 `after` 游标消息不存在）。
    InvalidInput(String),
    Io(io::Error),
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
}

impl StoreError {
    /// 稳定错误码；未登记的错误码不会发给协议层。
    pub fn code(&self) -> &'static str {
        match self {
            Self::StorageSchemaUnsupported { .. } => "storage_schema_unsupported",
            Self::StorageLocked { .. } => "storage_locked",
            Self::InvalidInput(_) => "invalid_input",
            Self::Io(_) | Self::Sqlite(_) => "storage_io",
            Self::Json(_) => "storage_json_invalid",
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StorageSchemaUnsupported { reason } => {
                write!(formatter, "会话库 schema 不支持：{reason}")
            }
            Self::StorageLocked { path } => write!(
                formatter,
                "会话库写事务在 busy_timeout 内未拿到写锁：{}",
                path.display()
            ),
            Self::InvalidInput(message) => write!(formatter, "无效的会话库请求：{message}"),
            Self::Io(error) => write!(formatter, "会话库 IO 失败：{error}"),
            Self::Sqlite(error) => write!(formatter, "会话库 SQLite 失败：{error}"),
            Self::Json(error) => write!(formatter, "会话库 JSON 失败：{error}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StorageSchemaUnsupported { .. }
            | Self::StorageLocked { .. }
            | Self::InvalidInput(_) => None,
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
