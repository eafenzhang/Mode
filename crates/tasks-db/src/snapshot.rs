//! 任务库 schema 快照：形状与归一化对齐
//! `scripts/sqlite-schema-snapshot.ts` + `scripts/lib/sqlite-schema.ts`。
//!
//! 只提供函数；现有门禁、基线与 CI 保持 TS 生产者不变。

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::TasksDbError;
use crate::js_compat::normalize_schema_sql;
use crate::migrations::{run_migrations, RunOptions};

const EXCLUDED_TABLES: [&str; 2] = ["tasks_schema_migration", "schema_migration"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnSnapshot {
    pub name: String,
    #[serde(rename = "type")]
    pub column_type: String,
    pub not_null: bool,
    pub default_value: Option<String>,
    pub primary_key: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexSnapshot {
    pub name: String,
    pub sql: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSnapshot {
    pub columns: Vec<ColumnSnapshot>,
    pub indexes: Vec<IndexSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasksSchemaSnapshot {
    pub tables: BTreeMap<String, TableSnapshot>,
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn read_columns(conn: &Connection, table: &str) -> Result<Vec<ColumnSnapshot>, TasksDbError> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))?;
    let mut rows = statement.query([])?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next()? {
        columns.push(ColumnSnapshot {
            name: row.get(1)?,
            column_type: row.get(2)?,
            not_null: row.get::<_, i64>(3)? == 1,
            default_value: row.get(4)?,
            primary_key: row.get(5)?,
        });
    }
    columns.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(columns)
}

fn read_indexes(conn: &Connection, table: &str) -> Result<Vec<IndexSnapshot>, TasksDbError> {
    let mut statement = conn.prepare(
        "SELECT name, sql FROM sqlite_master WHERE type='index' AND tbl_name=? AND sql IS NOT NULL ORDER BY name",
    )?;
    let mut rows = statement.query([table])?;
    let mut indexes = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let sql: String = row.get(1)?;
        indexes.push(IndexSnapshot {
            name,
            sql: normalize_schema_sql(&sql),
        });
    }
    Ok(indexes)
}

/// 内存库跑完整迁移后按 TS 快照形状读取 schema。
pub fn build_tasks_schema_snapshot() -> Result<TasksSchemaSnapshot, TasksDbError> {
    let conn = Connection::open_in_memory()?;
    run_migrations(&conn, RunOptions::default())?;
    let names = {
        let mut statement = conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        names
    };
    let mut tables = BTreeMap::new();
    for name in names {
        if EXCLUDED_TABLES.contains(&name.as_str()) {
            continue;
        }
        let columns = read_columns(&conn, &name)?;
        let indexes = read_indexes(&conn, &name)?;
        tables.insert(name, TableSnapshot { columns, indexes });
    }
    Ok(TasksSchemaSnapshot { tables })
}
