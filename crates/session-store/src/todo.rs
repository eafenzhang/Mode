//! `todo` 表读写（TodoWrite / TodoRead）。
//!
//! 表结构见迁移 `todo(session_id, content, status, priority, position,
//! time_created, time_updated)`；TodoWrite 是整表替换（观察：send the full list
//! each call; it replaces the previous one），因此在同一 `BEGIN IMMEDIATE`
//! 事务里先删后插，避免读到半张表。

use rusqlite::params;

use crate::error::StoreError;
use crate::SessionStore;

/// `todo` 的一行（按 `position` 升序即模型给出的列表顺序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoRecord {
    pub content: String,
    pub status: String,
    pub priority: String,
    pub position: i64,
    pub time_created: i64,
    pub time_updated: i64,
}

impl SessionStore {
    /// 当前会话的 todo 列表（空会话返回空数组）。
    pub fn todos(&self, session_id: &str) -> Result<Vec<TodoRecord>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT content, status, priority, position, time_created, time_updated \
             FROM todo WHERE session_id = ?1 ORDER BY position",
        )?;
        let rows = statement.query_map([session_id], |row| {
            Ok(TodoRecord {
                content: row.get(0)?,
                status: row.get(1)?,
                priority: row.get(2)?,
                position: row.get(3)?,
                time_created: row.get(4)?,
                time_updated: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 整表替换（TodoWrite 语义）。`position` 由调用方按列表顺序给出。
    pub fn replace_todos(&self, session_id: &str, todos: &[TodoRecord]) -> Result<(), StoreError> {
        self.with_write_transaction(|transaction| {
            transaction.execute("DELETE FROM todo WHERE session_id = ?1", [session_id])?;
            for todo in todos {
                transaction.execute(
                    "INSERT INTO todo (session_id, content, status, priority, position, \
                     time_created, time_updated) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        session_id,
                        todo.content,
                        todo.status,
                        todo.priority,
                        todo.position,
                        todo.time_created,
                        todo.time_updated,
                    ],
                )?;
            }
            Ok(())
        })
    }
}
