//! `allow_project` 规则的 SQLite 持久化（M2 Task 6 · D1 裁决 (a)：Rust 直连）。
//!
//! 表结构**不是**从迁移实现里读的：由 `probe-rule-store.mts` 跑旧运行时、再用
//! `node:sqlite` 检查它写出的 DB 得到（spec §3.3）：
//!
//! ```sql
//! CREATE TABLE local_setting (
//!   scope text not null, scope_id text not null, namespace text not null, key text not null,
//!   value text not null, schema_version integer not null,
//!   time_created integer not null, time_updated integer not null,
//!   primary key(scope, scope_id, namespace, key)
//! )
//! ```
//!
//! 行：`scope='project'`、`namespace='permission'`、`key='ruleset'`、`schema_version=1`，
//! `value` 是 [`Ruleset`] 的 JSON。`scope_id` 由 cwd 归一化（观察值：小写、非字母数字段折成 `-`）。
//!
//! # 并发边界
//!
//! 每次读写各开一次短连接（不长期持有），尽量缩小与 M3 Node Host 同时打开该文件的窗口；
//! 这是 D1 选 (a) 的已知代价，不是「已经解决」。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_protocol::permission::PermissionUpdate;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};

use crate::approval::Ruleset;

/// 黑盒观察到的 DDL（测试与排障用；生产库由 Host 迁移创建，Rust 侧不执行建表）。
pub const LOCAL_SETTING_DDL: &str = "CREATE TABLE local_setting (
        scope text not null,
        scope_id text not null,
        namespace text not null,
        key text not null,
        value text not null,
        schema_version integer not null,
        time_created integer not null,
        time_updated integer not null,
        primary key(scope, scope_id, namespace, key)
      )";

/// 规则持久化的消费端。会话只依赖这个 trait；SQLite 实现见 [`SqliteRuleStore`]。
pub trait RuleStore {
    fn load(&self, scope_id: &str) -> Option<Ruleset>;
    /// 在**单个写事务**里把 `updates` 追加到当前行。
    ///
    /// 不做「整份 ruleset 覆盖写」：两个会话各自持有旧副本时，覆盖写会丢规则。
    /// 实现必须用 `BEGIN IMMEDIATE` 先拿写锁、再读当前值、追加、写回。
    fn append(&self, scope_id: &str, updates: &[PermissionUpdate]) -> Result<(), String>;
}

/// cwd → `scope_id`（观察：`C:\Users\X\ws` → `proj_c-users-x-ws`）。
pub fn scope_id_for(cwd: &str) -> String {
    let normalized: Vec<String> = cwd
        .split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_lowercase())
        .collect();
    format!("proj_{}", normalized.join("-"))
}

/// 直接读写 `.mode/cli/db/db.sqlite` 的 `local_setting`。
pub struct SqliteRuleStore {
    db_path: PathBuf,
}

impl SqliteRuleStore {
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn open(&self, flags: OpenFlags) -> Result<Connection, String> {
        let connection = Connection::open_with_flags(&self.db_path, flags)
            .map_err(|error| format!("打开规则库 {} 失败：{error}", self.db_path.display()))?;
        // Host 可能正在写同一个库：短锁等待而不是立刻 SQLITE_BUSY。
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| format!("设置 busy_timeout 失败：{error}"))?;
        Ok(connection)
    }
}

impl RuleStore for SqliteRuleStore {
    fn load(&self, scope_id: &str) -> Option<Ruleset> {
        let connection = self.open(OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT value FROM local_setting \
                 WHERE scope = 'project' AND scope_id = ?1 AND namespace = 'permission' AND key = 'ruleset'",
                [scope_id],
                |row| row.get(0),
            )
            .optional()
            .ok()?;
        serde_json::from_str(&value?).ok()
    }

    fn append(&self, scope_id: &str, updates: &[PermissionUpdate]) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let mut connection = self.open(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        // `IMMEDIATE` 先拿写锁，避免「读后升级锁」阶段才发现冲突；配合 busy_timeout
        // 把并发写串行化。读-合并-写都在同一个事务里，两个会话不会互相覆盖。
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("开启规则写事务失败：{error}"))?;
        let current: Option<String> = transaction
            .query_row(
                "SELECT value FROM local_setting \
                 WHERE scope = 'project' AND scope_id = ?1 AND namespace = 'permission' AND key = 'ruleset'",
                [scope_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("读取当前规则失败：{error}"))?;
        let mut ruleset = current
            .and_then(|value| serde_json::from_str::<Ruleset>(&value).ok())
            .unwrap_or_default();
        ruleset.apply_updates(updates);
        let value = serde_json::to_string(&ruleset).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO local_setting \
                 (scope, scope_id, namespace, key, value, schema_version, time_created, time_updated) \
                 VALUES ('project', ?1, 'permission', 'ruleset', ?2, 1, ?3, ?3) \
                 ON CONFLICT(scope, scope_id, namespace, key) \
                 DO UPDATE SET value = excluded.value, time_updated = excluded.time_updated",
                params![scope_id, value, now],
            )
            .map_err(|error| format!("写入规则库失败：{error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("提交规则事务失败：{error}"))?;
        Ok(())
    }
}
