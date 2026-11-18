//! `session_target` 行（C3 goal 的唯一事实源）。
//!
//! 列形状来自 C3 夹具的 PRAGMA table_info（`agent-c3-contract.md` §1.3）：
//! `session_id` 主键（一个会话至多一行目标），`status` 的 CHECK 约束由迁移
//! `0004_session_target` 冻结。`set` / `replace` 覆盖同一行（新目标 id 覆盖旧 id），
//! `clear` 删除整行——不保留历史目标。
//!
//! `status` 保持 `String`：契约是 `active|paused|budget_limited|complete`，取值
//! 校验归 `agent_core::goal` 与 SQLite CHECK；本 crate 不引入第二套枚举。

use rusqlite::{params, OptionalExtension, Row};

use crate::error::StoreError;
use crate::SessionStore;

const TARGET_COLUMNS: &str = "session_id, target_id, objective, status, token_budget, \
tokens_used, time_used_seconds, time_created, time_updated, summary_title, active_input_id, \
active_run_started_at, active_run_last_seen_at";

/// `session_target` 一行的完整形状（字段与列一一对应，时间均为毫秒）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetRecord {
    pub session_id: String,
    pub target_id: String,
    pub objective: String,
    pub status: String,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub time_created: i64,
    pub time_updated: i64,
    pub summary_title: Option<String>,
    pub active_input_id: Option<String>,
    pub active_run_started_at: Option<i64>,
    pub active_run_last_seen_at: Option<i64>,
}

impl SessionStore {
    /// 读取会话目标；不存在返回 `None`。
    pub fn get_target(&self, session_id: &str) -> Result<Option<TargetRecord>, StoreError> {
        let connection = self.connection();
        let sql = format!("SELECT {TARGET_COLUMNS} FROM session_target WHERE session_id = ?1");
        let record = connection
            .query_row(&sql, [session_id], target_from_row)
            .optional()?;
        Ok(record)
    }

    /// upsert 一条目标（`session_id` 主键）：set / replace / 状态迁移都走这里，
    /// 与观察到的「单行覆盖」一致。
    pub fn upsert_target(&self, record: &TargetRecord) -> Result<(), StoreError> {
        self.with_write_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO session_target (session_id, target_id, objective, status, \
                 token_budget, tokens_used, time_used_seconds, time_created, time_updated, \
                 summary_title, active_input_id, active_run_started_at, active_run_last_seen_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
                 ON CONFLICT(session_id) DO UPDATE SET \
                 target_id = excluded.target_id, \
                 objective = excluded.objective, \
                 status = excluded.status, \
                 token_budget = excluded.token_budget, \
                 tokens_used = excluded.tokens_used, \
                 time_used_seconds = excluded.time_used_seconds, \
                 time_updated = excluded.time_updated, \
                 summary_title = excluded.summary_title, \
                 active_input_id = excluded.active_input_id, \
                 active_run_started_at = excluded.active_run_started_at, \
                 active_run_last_seen_at = excluded.active_run_last_seen_at",
                params![
                    record.session_id,
                    record.target_id,
                    record.objective,
                    record.status,
                    record.token_budget,
                    record.tokens_used,
                    record.time_used_seconds,
                    record.time_created,
                    record.time_updated,
                    record.summary_title,
                    record.active_input_id,
                    record.active_run_started_at,
                    record.active_run_last_seen_at,
                ],
            )?;
            Ok(())
        })
    }

    /// 删除会话目标（`clear`）；返回是否真的删掉了一行。
    pub fn delete_target(&self, session_id: &str) -> Result<bool, StoreError> {
        self.with_write_transaction(|transaction| {
            let deleted = transaction.execute(
                "DELETE FROM session_target WHERE session_id = ?1",
                [session_id],
            )?;
            Ok(deleted > 0)
        })
    }
}

fn target_from_row(row: &Row<'_>) -> rusqlite::Result<TargetRecord> {
    Ok(TargetRecord {
        session_id: row.get(0)?,
        target_id: row.get(1)?,
        objective: row.get(2)?,
        status: row.get(3)?,
        token_budget: row.get(4)?,
        tokens_used: row.get(5)?,
        time_used_seconds: row.get(6)?,
        time_created: row.get(7)?,
        time_updated: row.get(8)?,
        summary_title: row.get(9)?,
        active_input_id: row.get(10)?,
        active_run_started_at: row.get(11)?,
        active_run_last_seen_at: row.get(12)?,
    })
}
