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
//! # 单写者
//!
//! 打开时在数据库旁创建 `<db>.agent-write.lock`（`create_new`，内容含 owner PID 与
//! 打开时间；锁路径按数据库 realpath 派生，路径别名不能绕过），Drop 时删除；第二个
//! Rust `SessionStore` 打开同一文件会得到 [`StoreError::StorageLocked`]。这是 Rust 侧的
//! backstop：
//!
//! - Node 运行时不识别这个锁文件，**不能**用它阻止 Node 与 Rust 同时写同一数据根；
//!   真正的唯一所有者保证来自 `AgentRuntimeRegistry` / `agent-process` 的进程调度。
//! - 进程崩溃会留下锁文件；确认无进程持有后需人工删除，下一次打开才能成功。
//!
//! SQLite 连接使用短写事务：每次写操作用 `BEGIN IMMEDIATE` + `busy_timeout` 串行化，
//! 不长期占用 SQLite 写锁。读操作共用同一个连接并在进程内互斥，避免同一
//! `SessionStore` 的读看到写事务的中间状态。

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

use crate::schema::WriteLock;

/// 与 Host 写同一文件时的短锁等待，口径与 `agent-core::rule_store` 一致。
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 已迁移会话库的唯一读写句柄。一个数据库文件同时只允许一个 `SessionStore`。
pub struct SessionStore {
    path: PathBuf,
    // 字段顺序即 Drop 顺序：先关 SQLite 连接，再删除锁文件。
    connection: Mutex<Connection>,
    // 只用于持有锁并在 Drop 时释放；不对外暴露。
    _write_lock: WriteLock,
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
    /// 打开后立即执行 [`Self::ensure_supported_schema`]，失败时释放写锁文件。
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

        let write_lock = WriteLock::acquire(path)?;
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;

        let store = Self {
            path: path.to_path_buf(),
            connection: Mutex::new(connection),
            _write_lock: write_lock,
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
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = operation(&transaction)?;
        transaction.commit()?;
        Ok(value)
    }
}

/// `LIMIT` 绑定值：`None` → -1（SQLite 表示不限），避免拼 SQL。
pub(crate) fn bound_limit(limit: Option<usize>) -> i64 {
    match limit {
        None => -1,
        Some(value) => i64::try_from(value).unwrap_or(i64::MAX),
    }
}
