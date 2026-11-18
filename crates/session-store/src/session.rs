//! `session` / `session_entry` 行与读写。
//!
//! 列顺序与 `scripts/sqlite-schema-baseline.json` 的 `session-store.tables.session` 一致；
//! JSON 列（`permission` / `revert` / `summary_diffs`）按原始文本保存，本 crate 不解释
//! 其语义，避免发明 Node 侧形状。

use rusqlite::{params, OptionalExtension, Row};
use serde_json::Value;

use crate::error::StoreError;
use crate::SessionStore;

const SESSION_COLUMNS: &str = "id, project_id, workspace_id, parent_id, slug, directory, path, \
title, version, share_url, summary_additions, summary_deletions, summary_files, summary_diffs, \
revert, permission, time_created, time_updated, time_compacting, time_archived, task_type, \
title_source, title_message_id, time_title_updated, trace_id";

const SESSION_INSERT: &str = "INSERT INTO session (\
id, project_id, workspace_id, parent_id, slug, directory, path, title, version, share_url, \
summary_additions, summary_deletions, summary_files, summary_diffs, revert, permission, \
time_created, time_updated, time_compacting, time_archived, task_type, title_source, \
title_message_id, time_title_updated, trace_id) VALUES (\
?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, \
?21, ?22, ?23, ?24, ?25)";

/// `session` 一行的完整形状（与 schema 基线逐列对应）。
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRecord {
    pub id: String,
    pub project_id: String,
    pub workspace_id: Option<String>,
    pub parent_id: Option<String>,
    pub slug: String,
    pub directory: String,
    pub path: Option<String>,
    pub title: String,
    pub version: String,
    pub share_url: Option<String>,
    pub summary_additions: Option<i64>,
    pub summary_deletions: Option<i64>,
    pub summary_files: Option<i64>,
    /// 原始 JSON 文本列；C1 不解释形状。
    pub summary_diffs: Option<String>,
    /// 原始 JSON 文本列；C1 不解释形状。
    pub revert: Option<String>,
    /// 原始 JSON 文本列，观察值形如 `{"mode":"build"}`。
    pub permission: Option<String>,
    pub time_created: i64,
    pub time_updated: i64,
    pub time_compacting: Option<i64>,
    pub time_archived: Option<i64>,
    pub task_type: String,
    pub title_source: String,
    pub title_message_id: Option<String>,
    pub time_title_updated: Option<i64>,
    pub trace_id: Option<String>,
}

/// `session_entry` 一行。DB 列名是 `type`；Rust 字段用 `entry_type`，读写时映射。
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEntryRecord {
    pub id: String,
    pub session_id: String,
    pub entry_type: String,
    pub time_created: i64,
    pub time_updated: i64,
    pub data: Value,
}

impl SessionStore {
    /// 写入一条 session。重复 id 由 SQLite 主键报错，不会覆盖已有行。
    pub fn create_session(&self, record: SessionRecord) -> Result<(), StoreError> {
        self.with_write_transaction(|transaction| {
            transaction.execute(
                SESSION_INSERT,
                params![
                    record.id,
                    record.project_id,
                    record.workspace_id,
                    record.parent_id,
                    record.slug,
                    record.directory,
                    record.path,
                    record.title,
                    record.version,
                    record.share_url,
                    record.summary_additions,
                    record.summary_deletions,
                    record.summary_files,
                    record.summary_diffs,
                    record.revert,
                    record.permission,
                    record.time_created,
                    record.time_updated,
                    record.time_compacting,
                    record.time_archived,
                    record.task_type,
                    record.title_source,
                    record.title_message_id,
                    record.time_title_updated,
                    record.trace_id,
                ],
            )?;
            Ok(())
        })
    }

    /// 按 id 读取；不存在返回 `None`。
    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>, StoreError> {
        let connection = self.connection();
        let sql = format!("SELECT {SESSION_COLUMNS} FROM session WHERE id = ?1");
        let record = connection
            .query_row(&sql, [session_id], session_from_row)
            .optional()?;
        Ok(record)
    }

    /// 刷新 `session.time_updated`（LT-1 C1 Task 6：turn 终态后由运行时调用，让
    /// `session/list` 的 `time_updated DESC` 排序看到最新活动）。
    ///
    /// 只更新时间列，不改其他列；会话行不存在时报 [`StoreError::InvalidInput`]
    /// （fail-closed：静默 Ok 会把「runtime 指向了不存在的会话」藏起来）。
    pub fn touch_session(&self, session_id: &str, time_updated: i64) -> Result<(), StoreError> {
        self.with_write_transaction(|transaction| {
            let updated = transaction.execute(
                "UPDATE session SET time_updated = ?2 WHERE id = ?1",
                params![session_id, time_updated],
            )?;
            if updated == 0 {
                return Err(StoreError::InvalidInput(format!(
                    "touch_session：会话不存在：{session_id}"
                )));
            }
            Ok(())
        })
    }

    /// 更新 `session.permission`（JSON 文本）与 `time_updated`。
    ///
    /// `session/setMode` 用：热快照的 mode 来自 runtime 配置，而 `session/list` /
    /// 冷读从这一列投影，两处必须看到同一事实（最终评审 I4）。行不存在时报
    /// [`StoreError::InvalidInput`]，不静默成功。
    pub fn set_session_permission(
        &self,
        session_id: &str,
        permission: &str,
        time_updated: i64,
    ) -> Result<(), StoreError> {
        self.with_write_transaction(|transaction| {
            let updated = transaction.execute(
                "UPDATE session SET permission = ?2, time_updated = ?3 WHERE id = ?1",
                params![session_id, permission, time_updated],
            )?;
            if updated == 0 {
                return Err(StoreError::InvalidInput(format!(
                    "set_session_permission：会话不存在：{session_id}"
                )));
            }
            Ok(())
        })
    }

    /// 列出某 workspace 的会话，按 `time_updated DESC, id DESC`。
    ///
    /// `workspace_key` 只匹配 `session.directory`（实际 workspace 目录）。`project_id`
    /// 是 `proj_` + 归一化 cwd 的派生 scope，不是 `workspaceIdentity?.trim() ||
    /// workspacePath` 口径的 workspace key；用 OR 同时匹配会把其他 workspace 的会话串进
    /// 列表（见 `agent-server-c1-contract.md` §3.2）。
    /// `include_archived=false` 时排除 `time_archived` 非空的行。
    pub fn list_sessions(
        &self,
        workspace_key: &str,
        include_archived: bool,
        limit: Option<usize>,
    ) -> Result<Vec<SessionRecord>, StoreError> {
        let connection = self.connection();
        let sql = format!(
            "SELECT {SESSION_COLUMNS} FROM session \
             WHERE directory = ?1 \
               AND (?2 = 1 OR time_archived IS NULL) \
             ORDER BY time_updated DESC, id DESC LIMIT ?3"
        );
        let mut statement = connection.prepare(&sql)?;
        let records = statement
            .query_map(
                params![workspace_key, include_archived, crate::bound_limit(limit)],
                session_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// 列出**全部**会话（`session/list` 不带 workspace 时的线上语义，Task 0 观察：
    /// oracle 的 `session/list` params 可以只有 `{includeArchived}`）。
    /// 排序与归档过滤同 [`Self::list_sessions`]。
    pub fn list_all_sessions(
        &self,
        include_archived: bool,
        limit: Option<usize>,
    ) -> Result<Vec<SessionRecord>, StoreError> {
        let connection = self.connection();
        let sql = format!(
            "SELECT {SESSION_COLUMNS} FROM session \
             WHERE (?1 = 1 OR time_archived IS NULL) \
             ORDER BY time_updated DESC, id DESC LIMIT ?2"
        );
        let mut statement = connection.prepare(&sql)?;
        let records = statement
            .query_map(
                params![include_archived, crate::bound_limit(limit)],
                session_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// 读取一个会话的全部 `session_entry`（`rowid` 序；C1 只用小体积的运行时条目）。
    pub fn entries(&self, session_id: &str) -> Result<Vec<SessionEntryRecord>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT id, session_id, type, time_created, time_updated, data \
             FROM session_entry WHERE session_id = ?1 ORDER BY rowid",
        )?;
        let records = statement
            .query_map([session_id], entry_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// upsert 一条 `session_entry`（按主键 `id` 整行替换）。
    ///
    /// C1 的运行时条目（`runtime/model_selection` / `runtime-execution-state`）在
    /// setter 时会更新同一 id 的行；`append_entry` 的纯 INSERT 会在第二次写时撞主键。
    pub fn put_entry(&self, entry: SessionEntryRecord) -> Result<(), StoreError> {
        let data = serde_json::to_string(&entry.data)?;
        self.with_write_transaction(|transaction| {
            transaction.execute(
                "INSERT OR REPLACE INTO session_entry (id, session_id, type, time_created, time_updated, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    entry.id,
                    entry.session_id,
                    entry.entry_type,
                    entry.time_created,
                    entry.time_updated,
                    data,
                ],
            )?;
            Ok(())
        })
    }

    /// 追加一条 `session_entry`。
    pub fn append_entry(&self, entry: SessionEntryRecord) -> Result<(), StoreError> {
        let data = serde_json::to_string(&entry.data)?;
        self.with_write_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO session_entry (id, session_id, type, time_created, time_updated, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    entry.id,
                    entry.session_id,
                    entry.entry_type,
                    entry.time_created,
                    entry.time_updated,
                    data,
                ],
            )?;
            Ok(())
        })
    }
}

fn session_from_row(row: &Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get(0)?,
        project_id: row.get(1)?,
        workspace_id: row.get(2)?,
        parent_id: row.get(3)?,
        slug: row.get(4)?,
        directory: row.get(5)?,
        path: row.get(6)?,
        title: row.get(7)?,
        version: row.get(8)?,
        share_url: row.get(9)?,
        summary_additions: row.get(10)?,
        summary_deletions: row.get(11)?,
        summary_files: row.get(12)?,
        summary_diffs: row.get(13)?,
        revert: row.get(14)?,
        permission: row.get(15)?,
        time_created: row.get(16)?,
        time_updated: row.get(17)?,
        time_compacting: row.get(18)?,
        time_archived: row.get(19)?,
        task_type: row.get(20)?,
        title_source: row.get(21)?,
        title_message_id: row.get(22)?,
        time_title_updated: row.get(23)?,
        trace_id: row.get(24)?,
    })
}

fn entry_from_row(row: &Row<'_>) -> rusqlite::Result<SessionEntryRecord> {
    Ok(SessionEntryRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        entry_type: row.get(2)?,
        time_created: row.get(3)?,
        time_updated: row.get(4)?,
        data: serde_json::from_str(&row.get::<_, String>(5)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
    })
}
