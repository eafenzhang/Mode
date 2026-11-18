//! `part` 行与写入。
//!
//! `data` 保持 `serde_json::Value`；这里只负责序列化与落库，不解释 part 类型。

use rusqlite::{params, Row};
use serde_json::Value;

use crate::error::StoreError;
use crate::SessionStore;

const PART_INSERT: &str = "INSERT INTO part (\
id, message_id, session_id, time_created, time_updated, data, sequence) \
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

/// `part` 一行的完整形状（与 schema 基线逐列对应）。
#[derive(Debug, Clone, PartialEq)]
pub struct PartRecord {
    pub id: String,
    pub message_id: String,
    pub session_id: String,
    pub time_created: i64,
    pub time_updated: i64,
    pub data: Value,
    pub sequence: Option<i64>,
}

impl SessionStore {
    /// 追加一条 part。重复 id 由 SQLite 主键报错。
    pub fn append_part(&self, part: PartRecord) -> Result<(), StoreError> {
        self.with_write_transaction(|transaction| insert_part(transaction, &part))
    }
}

pub(crate) fn insert_part(
    transaction: &rusqlite::Transaction<'_>,
    part: &PartRecord,
) -> Result<(), StoreError> {
    let data = serde_json::to_string(&part.data)?;
    transaction.execute(
        PART_INSERT,
        params![
            part.id,
            part.message_id,
            part.session_id,
            part.time_created,
            part.time_updated,
            data,
            part.sequence,
        ],
    )?;
    Ok(())
}

pub(crate) fn part_from_row(row: &Row<'_>) -> rusqlite::Result<PartRecord> {
    Ok(PartRecord {
        id: row.get(0)?,
        message_id: row.get(1)?,
        session_id: row.get(2)?,
        time_created: row.get(3)?,
        time_updated: row.get(4)?,
        data: serde_json::from_str(&row.get::<_, String>(5)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        sequence: row.get(6)?,
    })
}
