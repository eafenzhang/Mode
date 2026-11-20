//! `message` 行与 message+parts 读取/写入。
//!
//! 顺序契约：会话内按 `COALESCE(sequence, -1), time_created, id` 升序；part 在同一
//! message 内按相同规则升序。`after` 是 message id 游标，找不到时返回
//! [`StoreError::InvalidInput`]，避免调用方把“游标失效”误判为“从头开始”。

use rusqlite::{params, OptionalExtension, Row};
use serde_json::Value;

use crate::error::StoreError;
use crate::part::{insert_part, part_from_row, PartRecord};
use crate::SessionStore;

const MESSAGE_COLUMNS: &str = "id, session_id, time_created, time_updated, data, sequence";
const MESSAGE_ORDER: &str = "COALESCE(sequence, -1), time_created, id";
const PART_ORDER: &str = "COALESCE(sequence, -1), time_created, id";

/// `message` 一行的完整形状（与 schema 基线逐列对应）。
#[derive(Debug, Clone, PartialEq)]
pub struct MessageRecord {
    pub id: String,
    pub session_id: String,
    pub time_created: i64,
    pub time_updated: i64,
    /// 原始 JSON；写入时按 `serde_json` 规范文本保存，不做投影。
    pub data: Value,
    pub sequence: Option<i64>,
}

/// 一条 message 及其全部 part（按顺序）。
#[derive(Debug, Clone, PartialEq)]
pub struct MessageWithParts {
    pub message: MessageRecord,
    pub parts: Vec<PartRecord>,
}

impl SessionStore {
    /// 追加一条 message。
    pub fn append_message(&self, message: MessageRecord) -> Result<(), StoreError> {
        self.with_write_transaction(|transaction| insert_message(transaction, &message))
    }

    /// 在同一事务里写入 message 及其 parts；任一 part 失败整批回滚。
    ///
    /// 先校验每条 part 的 `message_id` / `session_id` 与 message 一致，再开始写事务，
    /// 避免把归属错误的 part 挂到别的会话上。
    pub fn append_message_with_parts(
        &self,
        message: MessageRecord,
        parts: &[PartRecord],
    ) -> Result<(), StoreError> {
        self.with_write_transaction(|transaction| {
            Self::append_message_with_parts_tx(transaction, &message, parts)
        })
    }

    /// [`Self::append_message_with_parts`] 的**调用方事务内**变体：
    /// compact 落库把多条派生写合并进单个事务时使用。
    pub fn append_message_with_parts_tx(
        tx: &rusqlite::Transaction<'_>,
        message: &MessageRecord,
        parts: &[PartRecord],
    ) -> Result<(), StoreError> {
        for part in parts {
            if part.message_id != message.id || part.session_id != message.session_id {
                return Err(StoreError::InvalidInput(format!(
                    "part {} 不属于 message {}（message_id/session_id 不一致）",
                    part.id, message.id
                )));
            }
        }
        insert_message(tx, message)?;
        for part in parts {
            insert_part(tx, part)?;
        }
        Ok(())
    }

    /// 读取会话的 messages（含 parts），按升序返回。
    ///
    /// `after` 为 message id 游标（不含该条）：此时 `limit` 是从游标之后最早的 N 条。
    /// 无 `after` 且给 `limit` 时取**最新** N 条（与 Node store 的 `limit` 语义一致），
    /// 结果仍按 `sequence, time_created, id` 升序返回；`limit=0` 返回空。
    pub fn messages(
        &self,
        session_id: &str,
        after: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<MessageWithParts>, StoreError> {
        let connection = self.connection();
        let limit = crate::bound_limit(limit);

        let messages = match after {
            Some(after) => {
                let anchor = connection
                    .query_row(
                        "SELECT sequence, time_created, id FROM message \
                         WHERE id = ?1 AND session_id = ?2",
                        params![after, session_id],
                        |row| {
                            Ok((
                                row.get::<_, Option<i64>>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()?;
                let Some((sequence, time_created, id)) = anchor else {
                    return Err(StoreError::InvalidInput(format!(
                        "after 游标消息不存在：{after}"
                    )));
                };
                let sql = format!(
                    "SELECT {MESSAGE_COLUMNS} FROM message \
                     WHERE session_id = ?1 \
                       AND (COALESCE(sequence, -1) > COALESCE(?2, -1) \
                            OR (COALESCE(sequence, -1) = COALESCE(?2, -1) \
                                AND (time_created > ?3 \
                                     OR (time_created = ?3 AND id > ?4)))) \
                     ORDER BY {MESSAGE_ORDER} LIMIT ?5"
                );
                let mut statement = connection.prepare(&sql)?;
                let rows = statement.query_map(
                    params![session_id, sequence, time_created, id, limit],
                    message_from_row,
                )?;
                rows.collect::<Result<Vec<_>, _>>()?
            }
            None => {
                // Node store 的 limit 语义是「最新 N 条」：先在派生表里倒序截取，
                // 再由外层按同一排序键翻回升序，不能直接 LIMIT 升序查询。
                let sql = format!(
                    "SELECT {MESSAGE_COLUMNS} FROM ( \
                         SELECT {MESSAGE_COLUMNS} FROM message WHERE session_id = ?1 \
                         ORDER BY COALESCE(sequence, -1) DESC, time_created DESC, id DESC \
                         LIMIT ?2\
                     ) ORDER BY {MESSAGE_ORDER}"
                );
                let mut statement = connection.prepare(&sql)?;
                let rows = statement.query_map(params![session_id, limit], message_from_row)?;
                rows.collect::<Result<Vec<_>, _>>()?
            }
        };

        let mut part_statement = connection.prepare(&format!(
            "SELECT id, message_id, session_id, time_created, time_updated, data, sequence \
             FROM part WHERE message_id = ?1 ORDER BY {PART_ORDER}"
        ))?;
        let mut result = Vec::with_capacity(messages.len());
        for message in messages {
            let parts = part_statement
                .query_map([message.id.as_str()], part_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            result.push(MessageWithParts { message, parts });
        }
        Ok(result)
    }
}

fn insert_message(
    transaction: &rusqlite::Transaction<'_>,
    message: &MessageRecord,
) -> Result<(), StoreError> {
    let data = serde_json::to_string(&message.data)?;
    transaction.execute(
        "INSERT INTO message (id, session_id, time_created, time_updated, data, sequence) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            message.id,
            message.session_id,
            message.time_created,
            message.time_updated,
            data,
            message.sequence,
        ],
    )?;
    Ok(())
}

fn message_from_row(row: &Row<'_>) -> rusqlite::Result<MessageRecord> {
    Ok(MessageRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        time_created: row.get(2)?,
        time_updated: row.get(3)?,
        data: serde_json::from_str(&row.get::<_, String>(4)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        sequence: row.get(5)?,
    })
}
