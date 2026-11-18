//! schema 门。
//!
//! 必需表来自 `docs/specs/agent-core-parity.md` §5.5。C2 起迁移 runner
//! （`migrations.rs`）负责 fresh-install 创建与冻结 checksum 比对；本门在其之后
//! 兜底：必需表缺失或账本为空仍 fail-closed（升级路径漏建表、外部手改库等
//! 情况不静默放行）。
//!
//! 这里**不**做进程级写锁：库级写者由 SQLite WAL + `busy_timeout` 串行化，
//! 唯一 owner 由 `AgentRuntimeRegistry` / `agent-process` 调度（见 crate 文档）。

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
