//! schema 门与进程级写锁。
//!
//! C1 只读已迁移库：必需表来自 `docs/specs/agent-core-parity.md` §5.5；账本
//! `schema_migration` 的 DDL 由 Node 迁移 runner 创建（列：`id/checksum/app_version/
//! time_applied`），这里只要求它存在且至少有一行，不校验 checksum（C2 才实现迁移
//! runner 与账本比对）。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::StoreError;
use crate::SessionStore;

/// C1 读写涉及的表（`agent-core-parity.md` §5.5）。基线里的 `dwf_*` / `workflow_*`
/// 等表属于其他功能域，C1 不依赖，也不阻止它们存在。
pub const REQUIRED_TABLES: [&str; 12] = [
    "session",
    "message",
    "part",
    "session_entry",
    "session_input",
    "session_target",
    "todo",
    "local_setting",
    "permission",
    "model_usage",
    "turn_usage",
    "tool_usage",
];

/// Node 迁移 runner 的账本表名。
pub const SCHEMA_MIGRATION_TABLE: &str = "schema_migration";

impl SessionStore {
    /// 校验必需表与迁移账本。缺失/空账本一律 fail-closed，不做任何建表或修复。
    pub fn ensure_supported_schema(&self) -> Result<(), StoreError> {
        let connection = self.connection();
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
        let table_names = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        let missing = REQUIRED_TABLES
            .iter()
            .filter(|required| !table_names.iter().any(|name| name == *required))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(StoreError::StorageSchemaUnsupported {
                reason: format!("缺少必需表：{}", missing.join(", ")),
            });
        }

        if !table_names
            .iter()
            .any(|name| name == SCHEMA_MIGRATION_TABLE)
        {
            return Err(StoreError::StorageSchemaUnsupported {
                reason: format!("缺少 {SCHEMA_MIGRATION_TABLE} 迁移账本"),
            });
        }

        drop(statement);
        let applied: i64 =
            connection.query_row("SELECT COUNT(*) FROM schema_migration", [], |row| {
                row.get(0)
            })?;
        if applied == 0 {
            return Err(StoreError::StorageSchemaUnsupported {
                reason: format!(
                    "{SCHEMA_MIGRATION_TABLE} 账本为空，数据根尚未完成迁移（C1 不执行 fresh-install）"
                ),
            });
        }

        Ok(())
    }
}

/// `<db>.agent-write.lock` 的所有者。
///
/// 用 `create_new` 抢占；内容写入 owner 信息，Drop 时只在内容仍是自己写的值时才删除，
/// 避免手工删除锁文件后误删新 owner 的锁。Node 不识别此锁，见 crate 文档。
pub(crate) struct WriteLock {
    path: PathBuf,
    content: String,
}

impl WriteLock {
    pub(crate) fn acquire(db_path: &Path) -> Result<Self, StoreError> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = lock_path_for(db_path);
        let content = format!(
            "pid={}\nopened_at_ms={}\nsequence={}\n",
            std::process::id(),
            now_millis(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );

        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(StoreError::StorageLocked { path })
            }
            Err(error) => return Err(StoreError::Io(error)),
        };

        let lock = Self {
            path: path.clone(),
            content,
        };
        if let Err(error) = file.write_all(lock.content.as_bytes()) {
            // 内容可能只写了一半；显式删除，不能把半截锁留给下一个进程。
            let _ = fs::remove_file(&lock.path);
            return Err(StoreError::Io(error));
        }
        drop(file);
        Ok(lock)
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        // 只删除自己创建的锁：锁文件被手工删除后，可能有新 owner 占用了同一路径。
        if let Ok(current) = fs::read_to_string(&self.path) {
            if current == self.content {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

fn lock_path_for(db_path: &Path) -> PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push(".agent-write.lock");
    PathBuf::from(name)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
