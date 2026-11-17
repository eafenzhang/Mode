//! fresh-install 迁移（LT-1 C2 T7）：空数据根创建的 schema + 迁移账本与 T0 oracle
//! 观察逐字节一致；第二次启动 no-op；并发首启只有一个创建者；checksum 不一致
//! fail-closed。行为真源是机器录制的
//! `packages/desktop/tests/agent-runtime-oracle/fixtures/c2/fresh-install-ledger.json`
//! 与 `scripts/sqlite-schema-baseline.json`，不在测试里手写期望值。

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};

use common::TempRoot;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value as Json;
use session_store::{MigrationEvent, MigrationKind, SessionStore, StoreError};

fn fixture() -> Json {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packages")
        .join("desktop")
        .join("tests")
        .join("agent-runtime-oracle")
        .join("fixtures")
        .join("c2")
        .join("fresh-install-ledger.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 fresh-install 夹具 {} 失败：{error}", path.display()));
    serde_json::from_str(&text).expect("fresh-install 夹具必须是 JSON")
}

fn baseline() -> Json {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts")
        .join("sqlite-schema-baseline.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 schema 基线 {} 失败：{error}", path.display()));
    serde_json::from_str(&text).expect("schema 基线必须是 JSON")
}

/// CLI 布局的会话库路径（`{root}/.mode/cli/db/db.sqlite`；空根下目录不存在）。
fn cli_layout_db_path(root: &TempRoot) -> std::path::PathBuf {
    root.path()
        .join(".mode")
        .join("cli")
        .join("db")
        .join("db.sqlite")
}

/// 只读连接；探针同款 `foreign_keys=1`（该 pragma 是连接级设置，fixture 的
/// `pragmas.foreign_keys` 是探针连接自己的状态）。
fn read_connection(path: &Path) -> Connection {
    let connection =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("只读连接");
    connection
        .execute_batch("PRAGMA foreign_keys=1;")
        .expect("开启 foreign_keys");
    connection
}

/// `sqlite_master` 快照（探针同口径：排除 `sqlite_%`，`order by type, name`）。
fn read_master(connection: &Connection) -> Vec<(String, String, String, String)> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .expect("查询 sqlite_master");
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .expect("映射 sqlite_master");
    rows.collect::<Result<Vec<_>, _>>()
        .expect("读取 sqlite_master")
}

/// 账本行（rowid 序 = 写入序）。
fn read_ledger(connection: &Connection) -> Vec<(String, String, String, i64)> {
    let mut statement = connection
        .prepare(
            "SELECT id, checksum, app_version, time_applied FROM schema_migration ORDER BY rowid",
        )
        .expect("查询账本");
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .expect("映射账本");
    rows.collect::<Result<Vec<_>, _>>().expect("读取账本")
}

fn read_pragmas(connection: &Connection) -> (i64, String, i64, i64) {
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("user_version");
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("journal_mode");
    let page_size: i64 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .expect("page_size");
    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .expect("foreign_keys");
    (user_version, journal_mode, page_size, foreign_keys)
}

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 断言创建出的库与 fixture 的 69 条 `sqlite_master` 条目逐字节一致。
fn assert_master_matches_fixture(connection: &Connection) {
    let entries = fixture()["expect"]["firstStart"]["sqliteMaster"]["entries"]
        .as_array()
        .expect("夹具 entries")
        .clone();
    let master = read_master(connection);
    assert_eq!(
        master.len(),
        entries.len(),
        "sqlite_master 条目数必须与夹具一致"
    );
    for (index, (actual, expected)) in master.iter().zip(entries.iter()).enumerate() {
        let (kind, name, tbl_name, sql) = (
            expected["type"].as_str().expect("type"),
            expected["name"].as_str().expect("name"),
            expected["tbl_name"].as_str().expect("tbl_name"),
            expected["sql"].as_str().expect("sql"),
        );
        assert_eq!(&actual.0, kind, "条目 {index} type 不一致");
        assert_eq!(&actual.1, name, "条目 {index} name 不一致");
        assert_eq!(&actual.2, tbl_name, "条目 {index} tbl_name 不一致");
        assert_eq!(&actual.3, sql, "条目 {index} sql 与 oracle 存储文本不一致");
    }
}

/// 断言列/索引事实与 `scripts/sqlite-schema-baseline.json` 一致
/// （列按名排序比较；索引 SQL 归一化空白后比较）。
fn assert_schema_matches_baseline(connection: &Connection) {
    let baseline = baseline();
    let tables = baseline["databases"]["session-store"]["tables"]
        .as_object()
        .expect("基线 tables");

    let mut actual_tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .expect("查询表名")
        .query_map([], |row| row.get(0))
        .expect("映射表名")
        .collect::<Result<Vec<_>, _>>()
        .expect("读取表名");
    actual_tables.retain(|name| name != "schema_migration");
    let mut expected_tables: Vec<&String> = tables.keys().collect();
    expected_tables.sort();
    let expected_tables: Vec<String> = expected_tables.into_iter().cloned().collect();
    assert_eq!(actual_tables, expected_tables, "表清单与基线不一致");

    for (name, table) in tables {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info(\"{name}\")"))
            .unwrap_or_else(|error| panic!("table_info({name}) 失败：{error}"));
        let mut columns: Vec<(String, String, bool, Option<String>, i64)> = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .expect("映射列")
            .collect::<Result<Vec<_>, _>>()
            .expect("读取列");
        columns.sort_by(|left, right| left.0.cmp(&right.0));

        let expected_columns = table["columns"].as_array().expect("columns");
        let mut expected: Vec<(String, String, bool, Option<String>, i64)> = expected_columns
            .iter()
            .map(|column| {
                (
                    column["name"].as_str().expect("列名").to_owned(),
                    column["type"].as_str().expect("列类型").to_owned(),
                    column["notNull"].as_bool().unwrap_or(false),
                    column["defaultValue"]
                        .as_str()
                        .map(|value| value.to_owned()),
                    column["primaryKey"].as_i64().unwrap_or(0),
                )
            })
            .collect();
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(columns, expected, "表 {name} 的列与基线不一致");

        let mut statement = connection
            .prepare(&format!(
                "SELECT name, sql FROM sqlite_master WHERE type='index' AND tbl_name='{name}' \
                 AND sql IS NOT NULL ORDER BY name"
            ))
            .expect("查询索引");
        let indexes: Vec<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("映射索引")
            .collect::<Result<Vec<_>, _>>()
            .expect("读取索引");
        let expected_indexes = table["indexes"].as_array().expect("indexes");
        assert_eq!(indexes.len(), expected_indexes.len(), "表 {name} 索引数");
        for ((actual_name, actual_sql), expected_index) in indexes.iter().zip(expected_indexes) {
            assert_eq!(
                actual_name,
                expected_index["name"].as_str().expect("索引名"),
                "表 {name} 的索引名"
            );
            assert_eq!(
                normalize_sql(actual_sql),
                expected_index["sql"].as_str().expect("索引 SQL"),
                "表 {name} 的索引 SQL 与基线不一致"
            );
        }
    }
}

/// 断言账本 22 行与 fixture 逐字节一致（`time_applied` 只断言非递减）。
fn assert_ledger_matches_fixture(connection: &Connection) {
    let rows = fixture()["expect"]["firstStart"]["ledger"]["rows"]
        .as_array()
        .expect("夹具 ledger rows")
        .clone();
    let actual = read_ledger(connection);
    assert_eq!(actual.len(), rows.len(), "账本行数必须与夹具一致");
    let mut previous_time: Option<i64> = None;
    for ((actual_id, actual_checksum, actual_version, actual_time), expected) in
        actual.iter().zip(rows.iter())
    {
        assert_eq!(actual_id, expected["id"].as_str().expect("id"));
        assert_eq!(
            actual_checksum,
            expected["checksum"].as_str().expect("checksum")
        );
        assert_eq!(
            actual_version,
            expected["app_version"].as_str().expect("app_version")
        );
        if let Some(previous) = previous_time {
            assert!(
                *actual_time >= previous,
                "time_applied 必须非递减：{actual_time} < {previous}"
            );
        }
        previous_time = Some(*actual_time);
    }
}

fn count_migrating(events: &[MigrationEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, MigrationEvent::Migrating { .. }))
        .count()
}

fn migration_id_of(event: &MigrationEvent) -> Option<&'static str> {
    match event {
        MigrationEvent::Migrating { migration_id, .. } => Some(migration_id),
        _ => None,
    }
}

#[test]
fn fresh_root_composes_oracle_schema_ledger_and_pragmas() {
    let root = TempRoot::new("fresh-first");
    let db_path = cli_layout_db_path(&root);

    let mut events = Vec::new();
    let store = SessionStore::open_with_progress(&db_path, &mut |event| events.push(event))
        .expect("空根必须能创建并打开");
    assert_eq!(store.db_path(), db_path.as_path());
    assert!(db_path.is_file(), "数据库文件必须被创建");
    drop(store);

    // 启动阶段事实源：1 个 Inspected + 22 个 Migrating + 1 个 Committing，
    // migrationId 顺序 = 冻结清单顺序。
    assert_eq!(count_migrating(&events), 22, "fresh 必须执行 22 步");
    let mut executed: u64 = 0;
    for (index, event) in events.iter().enumerate() {
        match event {
            MigrationEvent::Inspected { facts } => {
                assert_eq!(index, 0, "Inspected 必须最先发出");
                assert_eq!(facts.kind, MigrationKind::Initialize);
                assert_eq!(facts.executed_count, 0);
            }
            MigrationEvent::Migrating { facts, .. } => {
                assert_eq!(facts.kind, MigrationKind::Initialize);
                assert_eq!(facts.executed_count, executed, "执行前发出");
                assert_eq!(facts.committed_count, 0);
                executed += 1;
            }
            MigrationEvent::Committing { facts } => {
                assert_eq!(index, 23, "Committing 最后发出");
                assert_eq!(facts.executed_count, 22);
                assert_eq!(facts.committed_count, 0, "提交前 committed 仍为 0");
                assert_eq!(facts.last_applied_migration_id, None, "空账本基线为 null");
            }
        }
    }
    let ids: Vec<&str> = events.iter().filter_map(migration_id_of).collect();
    let expected_ids: Vec<&str> = session_store::migration_definitions()
        .iter()
        .map(|definition| definition.id)
        .collect();
    assert_eq!(ids, expected_ids, "migrating 帧的 migrationId 顺序");

    let connection = read_connection(&db_path);
    assert_master_matches_fixture(&connection);
    assert_schema_matches_baseline(&connection);
    assert_ledger_matches_fixture(&connection);

    let (user_version, journal_mode, page_size, foreign_keys) = read_pragmas(&connection);
    let expected = &fixture()["expect"]["firstStart"]["sqliteMaster"]["pragmas"];
    assert_eq!(user_version, expected["user_version"].as_i64().unwrap());
    assert_eq!(journal_mode, expected["journal_mode"].as_str().unwrap());
    assert_eq!(page_size, expected["page_size"].as_i64().unwrap());
    assert_eq!(foreign_keys, expected["foreign_keys"].as_i64().unwrap());
}

#[test]
fn second_open_is_noop_and_keeps_ledger_stable() {
    let root = TempRoot::new("fresh-second");
    let db_path = cli_layout_db_path(&root);
    SessionStore::open(&db_path).expect("首次启动创建");

    let first = {
        let connection = read_connection(&db_path);
        (read_master(&connection), read_ledger(&connection))
    };

    let mut events = Vec::new();
    SessionStore::open_with_progress(&db_path, &mut |event| events.push(event))
        .expect("第二次启动必须直接打开");
    assert_eq!(
        count_migrating(&events),
        0,
        "第二次启动不得重跑迁移（fixture §5.1 无 migrating 帧）"
    );
    assert_eq!(events.len(), 2, "只有 Inspected + Committing");
    match &events[0] {
        MigrationEvent::Inspected { facts } => {
            assert_eq!(facts.kind, MigrationKind::None);
        }
        other => panic!("首事件必须是 Inspected，实际 {other:?}"),
    }
    match &events[1] {
        MigrationEvent::Committing { facts } => {
            assert_eq!(facts.kind, MigrationKind::None);
            assert_eq!(facts.executed_count, 0);
            assert_eq!(
                facts.last_applied_migration_id.as_deref(),
                Some("0022_backfilled_session_reasoning"),
                "锁内基线是最后一条账本 id"
            );
        }
        other => panic!("次事件必须是 Committing，实际 {other:?}"),
    }

    let second = {
        let connection = read_connection(&db_path);
        (read_master(&connection), read_ledger(&connection))
    };
    assert_eq!(first.0, second.0, "sqlite_master 必须逐条不变");
    assert_eq!(first.1, second.1, "账本不得重写（fixture ledgerStable）");
}

#[test]
fn concurrent_first_start_has_exactly_one_creator() {
    let root = TempRoot::new("fresh-concurrent");
    let db_path = Arc::new(cli_layout_db_path(&root));

    let collector: Arc<Mutex<Vec<Vec<MigrationEvent>>>> = Arc::new(Mutex::new(Vec::new()));
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let db_path = Arc::clone(&db_path);
        let collector = Arc::clone(&collector);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            let mut events = Vec::new();
            let result =
                SessionStore::open_with_progress(&db_path, &mut |event| events.push(event));
            collector.lock().expect("collector").push(events);
            result.expect("并发首启的两个打开都必须成功")
        }));
    }
    for handle in handles {
        handle.join().expect("线程不得 panic");
    }

    // 两个打开都成功；迁移步总量恒为 22（后到者在锁内重读后全部跳过）。
    let events = collector.lock().expect("collector").clone();
    let total_migrating: usize = events.iter().map(|events| count_migrating(events)).sum();
    assert_eq!(total_migrating, 22, "22 步只执行一次：{total_migrating}");
    for events in &events {
        for event in events {
            if let MigrationEvent::Migrating { facts, .. } = event {
                assert_eq!(facts.kind, MigrationKind::Initialize);
            }
        }
    }

    let connection = read_connection(&db_path);
    assert_master_matches_fixture(&connection);
    assert_ledger_matches_fixture(&connection);
    assert_eq!(read_ledger(&connection).len(), 22, "无重复账本行");
}

#[test]
fn checksum_mismatch_on_existing_ledger_stays_fail_closed() {
    let root = TempRoot::new("fresh-checksum");
    let db_path = cli_layout_db_path(&root);
    SessionStore::open(&db_path).expect("先创建库");

    let connection = Connection::open(&db_path).expect("打开库改账本");
    connection
        .execute(
            "UPDATE schema_migration SET checksum = 'mutated-checksum' WHERE id = '0010_usage_observability'",
            [],
        )
        .expect("篡改一行 checksum");
    drop(connection);

    let error = SessionStore::open(&db_path).expect_err("checksum 不一致必须 fail-closed");
    assert!(
        matches!(error, StoreError::StorageSchemaUnsupported { .. }),
        "应报 StorageSchemaUnsupported，实际：{error:?}"
    );
    assert_eq!(error.code(), "storage_schema_unsupported");
    assert!(
        error.to_string().contains("0010_usage_observability"),
        "错误信息必须点出冲突迁移：{error}"
    );
}

#[test]
fn c1_era_database_with_frozen_ledger_reopens_without_migrations() {
    // C1 时代的真实形态：Node 迁移完成的全量账本 + 已迁移 schema → 打开是 no-op。
    let root = TempRoot::new("fresh-c1-era");
    let db_path = root.db_path();
    common::create_baseline_database(&db_path, &common::FixtureOptions::default());
    let rows = fixture()["expect"]["firstStart"]["ledger"]["rows"]
        .as_array()
        .expect("夹具 ledger rows")
        .clone();
    let connection = Connection::open(&db_path).expect("打开夹具库");
    connection
        .execute_batch("DELETE FROM schema_migration")
        .expect("清空占位行");
    for row in &rows {
        connection
            .execute(
                "INSERT INTO schema_migration (id, checksum, app_version, time_applied) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    row["id"].as_str().unwrap(),
                    row["checksum"].as_str().unwrap(),
                    row["app_version"].as_str().unwrap(),
                    1_000 + row["id"].as_str().unwrap().len() as i64,
                ],
            )
            .expect("写入冻结账本行");
    }
    drop(connection);
    let ledger_before = {
        let connection = read_connection(&db_path);
        read_ledger(&connection)
    };

    let mut events = Vec::new();
    SessionStore::open_with_progress(&db_path, &mut |event| events.push(event))
        .expect("全量冻结账本的库必须直接打开");
    assert_eq!(count_migrating(&events), 0, "全量账本不重跑迁移");
    let connection = read_connection(&db_path);
    assert_eq!(read_ledger(&connection), ledger_before, "账本逐字不变");
}

#[test]
fn path_directory_stays_fail_closed() {
    let root = TempRoot::new("fresh-dir-path");
    std::fs::create_dir_all(root.db_path()).expect("建同名目录");
    let error = SessionStore::open(&root.db_path()).expect_err("目录路径必须 fail-closed");
    assert!(
        matches!(error, StoreError::StorageSchemaUnsupported { .. }),
        "应报 StorageSchemaUnsupported，实际：{error:?}"
    );
}

#[test]
fn empty_ledger_with_only_ledger_table_is_initialize() {
    // 账本表存在但零行且无业务表：等价空数据根，按 initialize 创建。
    let root = TempRoot::new("fresh-empty-ledger");
    let db_path = root.db_path();
    let connection = Connection::open(&db_path).expect("建空库");
    connection
        .execute_batch(
            "CREATE TABLE schema_migration (
      id text primary key, checksum text not null, app_version text, time_applied integer not null
    )",
        )
        .expect("建空账本");
    drop(connection);

    SessionStore::open(&db_path).expect("空账本 + 空 schema 必须按 fresh 创建");
    let connection = read_connection(&db_path);
    assert_ledger_matches_fixture(&connection);
    assert_master_matches_fixture(&connection);
}

/// 防止 `Value` 导入未使用告警的占位断言（fixture 帮助函数共同签名）。
#[test]
fn fixture_shape_is_machine_recorded() {
    let fixture = fixture();
    assert_eq!(
        fixture["expect"]["firstStart"]["startup"]["count"].as_i64(),
        Some(27),
        "门帧契约：fresh 27 帧"
    );
    assert_eq!(
        fixture["expect"]["secondStart"]["startup"]["count"].as_i64(),
        Some(5),
        "门帧契约：第二次启动 5 帧"
    );
}
