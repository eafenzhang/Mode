//! `session-store` 集成测试共用夹具。
//!
//! 测试库的 DDL 只从 `scripts/sqlite-schema-baseline.json` 的 `session-store` 数据生成，
//! 不在测试里手写第二份 schema。迁移账本 `schema_migration` 的 DDL 与真实迁移 runner
//! 一致（`scripts/sqlite-schema-snapshot.ts` 的建表语句），但账本行是测试夹具自己插入的。

#![allow(dead_code)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};
use session_store::{MessageRecord, PartRecord, SessionEntryRecord, SessionRecord};

const LEDGER_DDL: &str = "CREATE TABLE schema_migration (
    id text primary key,
    checksum text not null,
    app_version text,
    time_applied integer not null
)";

/// 临时目录；Drop 时递归删除。测试里的 store/连接必须先于它释放。
pub struct TempRoot(PathBuf);

impl TempRoot {
    pub fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-session-store-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn db_path(&self) -> PathBuf {
        self.0.join("db.sqlite")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 与 `SessionStore::open` 相同的锁文件命名：`<db>.agent-write.lock`。
pub fn lock_path(db_path: &Path) -> PathBuf {
    let mut name = OsString::from(db_path.as_os_str());
    name.push(".agent-write.lock");
    PathBuf::from(name)
}

#[derive(Debug, Clone)]
pub struct FixtureOptions {
    pub ledger: bool,
    pub ledger_rows: usize,
    pub omit_tables: Vec<String>,
}

impl Default for FixtureOptions {
    fn default() -> Self {
        Self {
            ledger: true,
            ledger_rows: 1,
            omit_tables: Vec::new(),
        }
    }
}

fn baseline() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts")
        .join("sqlite-schema-baseline.json");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 schema 基线 {} 失败：{error}", path.display()));
    serde_json::from_str(&text).expect("schema 基线必须是 JSON")
}

fn session_store_tables() -> Map<String, Value> {
    let baseline = baseline();
    baseline["databases"]["session-store"]["tables"]
        .as_object()
        .expect("基线缺少 databases[session-store].tables")
        .clone()
}

/// 按基线生成完整测试库（全部表 + 索引），再按选项补账本。
pub fn create_baseline_database(path: &Path, options: &FixtureOptions) {
    let tables = session_store_tables();
    let connection = Connection::open(path).expect("创建测试库");
    for (name, table) in &tables {
        if options.omit_tables.iter().any(|omitted| omitted == name) {
            continue;
        }
        connection
            .execute_batch(&table_ddl(name, table))
            .unwrap_or_else(|error| panic!("创建表 {name} 失败：{error}"));
    }
    for (name, table) in &tables {
        if options.omit_tables.iter().any(|omitted| omitted == name) {
            continue;
        }
        for index in table["indexes"].as_array().expect("indexes 数组") {
            let sql = index["sql"].as_str().expect("索引 SQL");
            connection
                .execute_batch(sql)
                .unwrap_or_else(|error| panic!("创建索引失败：{error}"));
        }
    }
    if options.ledger {
        connection.execute_batch(LEDGER_DDL).expect("创建账本");
        for index in 0..options.ledger_rows {
            connection
                .execute(
                    "INSERT INTO schema_migration (id, checksum, app_version, time_applied) \
                     VALUES (?1, 'fixture-checksum', '0.0.0', 1)",
                    [format!("000{index}_fixture")],
                )
                .expect("写入账本行");
        }
    }
}

fn table_ddl(name: &str, table: &Value) -> String {
    let columns = table["columns"].as_array().expect("columns 数组");
    let mut definitions = Vec::with_capacity(columns.len() + 1);
    let mut primary_key: Vec<(i64, String)> = Vec::new();
    for column in columns {
        let column_name = column["name"].as_str().expect("列名");
        let column_type = column["type"].as_str().expect("列类型");
        let mut definition = format!("{} {}", quote_identifier(column_name), column_type);
        if column["notNull"].as_bool().unwrap_or(false) {
            definition.push_str(" NOT NULL");
        }
        if let Some(default) = column["defaultValue"].as_str() {
            definition.push_str(" DEFAULT ");
            definition.push_str(default);
        }
        definitions.push(definition);
        let position = column["primaryKey"].as_i64().unwrap_or(0);
        if position > 0 {
            primary_key.push((position, column_name.to_owned()));
        }
    }
    if !primary_key.is_empty() {
        primary_key.sort_by_key(|(position, _)| *position);
        let names = primary_key
            .iter()
            .map(|(_, column)| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        definitions.push(format!("PRIMARY KEY ({names})"));
    }
    format!(
        "CREATE TABLE {} (\n  {}\n)",
        quote_identifier(name),
        definitions.join(",\n  ")
    )
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

pub fn raw_connection(path: &Path) -> Connection {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("只读连接");
    connection
        .busy_timeout(Duration::from_secs(5))
        .expect("busy_timeout");
    connection
}

pub fn count_rows(path: &Path, table: &str) -> i64 {
    raw_connection(path)
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap_or_else(|error| panic!("统计 {table} 行数失败：{error}"))
}

pub fn read_raw_text(path: &Path, table: &str, column: &str, id: &str) -> String {
    raw_connection(path)
        .query_row(
            &format!("SELECT {column} FROM {table} WHERE id = ?1"),
            [id],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| panic!("读取 {table}.{column} 失败：{error}"))
}

pub fn sample_session(
    id: &str,
    workspace: &str,
    time_updated: i64,
    archived: bool,
) -> SessionRecord {
    SessionRecord {
        id: id.to_owned(),
        project_id: format!("proj_{id}"),
        workspace_id: None,
        parent_id: None,
        slug: id.to_owned(),
        directory: workspace.to_owned(),
        path: Some(workspace.to_owned()),
        title: format!("title-{id}"),
        version: "0.0.0".to_owned(),
        share_url: None,
        summary_additions: None,
        summary_deletions: None,
        summary_files: None,
        summary_diffs: None,
        revert: None,
        permission: Some("{\"mode\":\"build\"}".to_owned()),
        time_created: time_updated.saturating_sub(10),
        time_updated,
        time_compacting: None,
        time_archived: archived.then_some(time_updated + 10),
        task_type: "interactive".to_owned(),
        title_source: "first_input".to_owned(),
        title_message_id: None,
        time_title_updated: None,
        trace_id: Some(format!("trace-{id}")),
    }
}

pub fn sample_message(id: &str, session_id: &str, sequence: i64) -> MessageRecord {
    MessageRecord {
        id: id.to_owned(),
        session_id: session_id.to_owned(),
        time_created: 1_000 + sequence,
        time_updated: 1_010 + sequence,
        data: json!({
            "role": "user",
            "messageId": id,
            "sequence": sequence,
        }),
        sequence: Some(sequence),
    }
}

pub fn sample_part(id: &str, message_id: &str, session_id: &str, sequence: i64) -> PartRecord {
    PartRecord {
        id: id.to_owned(),
        message_id: message_id.to_owned(),
        session_id: session_id.to_owned(),
        time_created: 1_000 + sequence,
        time_updated: 1_010 + sequence,
        data: json!({
            "type": "text",
            "text": format!("part {id}"),
        }),
        sequence: Some(sequence),
    }
}

pub fn sample_entry(id: &str, session_id: &str) -> SessionEntryRecord {
    SessionEntryRecord {
        id: id.to_owned(),
        session_id: session_id.to_owned(),
        entry_type: "runtime/model_selection".to_owned(),
        time_created: 2_000,
        time_updated: 2_000,
        data: json!({
            "modelSelection": {
                "providerId": "fixture-provider",
                "modelId": "fixture-model",
            },
        }),
    }
}
