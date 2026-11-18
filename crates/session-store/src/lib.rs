//! `session-store`：CLI 会话库 `db.sqlite` 的 schema 门与核心读写（LT-1 C1）。
//!
//! 数据库路径由调用方给出（CLI 布局是 `{dataRoot}/.mode/cli/db/db.sqlite`）。C1 只接
//! **已由 Node 迁移完成**的数据根：
//!
//! - [`SessionStore::open`] 不建库、不建表、不跑迁移；文件不存在、必需表缺失、
//!   `schema_migration` 账本缺失或为空都返回 [`StoreError::StorageSchemaUnsupported`]。
//! - schema 契约是 `scripts/sqlite-schema-baseline.json` 的 `session-store` 段；
//!   fresh-install migration 属于 C2。
//! - `message.data` / `part.data` 保持 `serde_json::Value`，写入时按 `serde_json`
//!   规范文本落库；读取再写入逐字节稳定。
//!
//! # 写者并发（2026-11-17 控制器裁决 (a)）
//!
//! **不再**创建 `<db>.agent-write.lock` 进程独占锁。多个合法 owner 的 workspace
//! 进程（以及 Node 回退路径）可以同时打开同一会话库：
//!
//! - 写操作使用短事务 `BEGIN IMMEDIATE` + `busy_timeout`，由 SQLite/WAL 串行化；
//!   `busy_timeout` 耗尽时报 [`StoreError::StorageLocked`]（不静默丢写）。
//! - 「一个 workspace 一个 Agent」的唯一 owner 保证来自 `AgentRuntimeRegistry` /
//!   `agent-process` 的进程调度，而不是库文件锁。Node 不识别 Rust 的锁文件，独占锁
//!   既挡不住 Node 重叠，又会误伤第二个 workspace（最终评审 C1）。
//! - schema 门保持不变：打开时仍只接受已迁移库，缺表/缺账本 fail-closed。
//!
//! 读操作共用同一个连接并在进程内互斥，避免同一 `SessionStore` 的读看到写事务的
//! 中间状态。

mod error;
mod message;
mod part;
mod schema;
mod session;
mod usage;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

pub use error::StoreError;
pub use message::{MessageRecord, MessageWithParts};
pub use part::PartRecord;
pub use schema::{REQUIRED_TABLES, SCHEMA_MIGRATION_TABLE};
pub use session::{SessionEntryRecord, SessionRecord};
pub use usage::{ModelUsageRecord, ToolUsageRecord, TurnUsageRecord};

/// 与 Host 写同一文件时的短锁等待，口径与 `agent-core::rule_store` 一致。
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 已迁移会话库的读写句柄；多个实例可指向同一数据库文件（SQLite 串行化写）。
pub struct SessionStore {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl fmt::Debug for SessionStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl SessionStore {
    /// 打开一个已迁移的会话库。
    ///
    /// 不创建数据库：路径不存在或不是普通文件直接报
    /// [`StoreError::StorageSchemaUnsupported`]（权限等真实 IO 错误仍报 `Io`）。
    /// 打开后立即执行 [`Self::ensure_supported_schema`]；不创建任何进程锁文件，多个
    /// `SessionStore` 可以同时打开同一文件（写由 SQLite 串行化）。
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => {
                return Err(StoreError::StorageSchemaUnsupported {
                    reason: format!("路径不是普通文件：{}", path.display()),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::StorageSchemaUnsupported {
                    reason: format!("会话库不存在：{}", path.display()),
                })
            }
            Err(error) => return Err(StoreError::Io(error)),
        }

        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;

        let store = Self {
            path: path.to_path_buf(),
            connection: Mutex::new(connection),
        };
        store.ensure_supported_schema()?;
        Ok(store)
    }

    /// 会话库路径（调用方用于日志/诊断，不用于绕过本 store 写库）。
    pub fn db_path(&self) -> &Path {
        &self.path
    }

    /// 在进程内互斥地取连接；读操作也走这里，避免同连接并发。
    pub(crate) fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 在一个 `BEGIN IMMEDIATE` 事务里执行写操作；闭包返回错误时回滚。
    pub(crate) fn with_write_transaction<T>(
        &self,
        operation: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut connection = self.connection();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| self.map_write_error(error))?;
        let value = operation(&transaction)?;
        transaction
            .commit()
            .map_err(|error| self.map_write_error(error))?;
        Ok(value)
    }
}

impl SessionStore {
    /// 写事务拿不到锁（`busy_timeout` 耗尽）→ `storage_locked`；其余错误保持既有映射。
    fn map_write_error(&self, error: rusqlite::Error) -> StoreError {
        match &error {
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                StoreError::StorageLocked {
                    path: self.path.clone(),
                }
            }
            _ => error.into(),
        }
    }
}

/// `LIMIT` 绑定值：`None` → -1（SQLite 表示不限），避免拼 SQL。
pub(crate) fn bound_limit(limit: Option<usize>) -> i64 {
    match limit {
        None => -1,
        Some(value) => i64::try_from(value).unwrap_or(i64::MAX),
    }
}
