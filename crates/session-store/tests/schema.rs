//! schema 门：只接受已迁移的 Node 会话库，缺库/缺表/缺账本或空账本都 fail-closed。

mod common;

use common::{create_baseline_database, create_ledger, lock_path, FixtureOptions, TempRoot};
use session_store::{SessionStore, StoreError};

#[test]
fn opens_database_created_from_schema_baseline() {
    let root = TempRoot::new("schema-ok");
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());

    let store = SessionStore::open(&db_path).expect("已迁移库必须能打开");
    store
        .ensure_supported_schema()
        .expect("显式复查 schema 仍应通过");
    assert_eq!(store.db_path(), db_path.as_path());
    assert!(lock_path(&db_path).exists(), "打开期间必须持有写锁文件");

    drop(store);
    assert!(
        !lock_path(&db_path).exists(),
        "关闭后必须删除写锁文件，不能把数据根永久锁死"
    );
}

#[test]
fn missing_database_is_schema_unsupported_and_is_not_created() {
    let root = TempRoot::new("schema-missing-db");
    let db_path = root.db_path();

    let error = SessionStore::open(&db_path).expect_err("缺库必须 fail-closed");
    assert!(
        matches!(error, StoreError::StorageSchemaUnsupported { .. }),
        "缺库应报 StorageSchemaUnsupported，实际：{error:?}"
    );
    assert_eq!(error.code(), "storage_schema_unsupported");
    assert!(!db_path.exists(), "open 不能顺手创建数据库文件");
    assert!(!lock_path(&db_path).exists(), "校验失败后不能残留写锁文件");
}

#[test]
fn missing_required_table_is_schema_unsupported() {
    let root = TempRoot::new("schema-missing-table");
    let db_path = root.db_path();
    create_baseline_database(
        &db_path,
        &FixtureOptions {
            omit_tables: vec!["part".to_owned()],
            ..FixtureOptions::default()
        },
    );

    let error = SessionStore::open(&db_path).expect_err("缺必需表必须 fail-closed");
    assert!(matches!(error, StoreError::StorageSchemaUnsupported { .. }));
    assert_eq!(error.code(), "storage_schema_unsupported");
    assert!(
        error.to_string().contains("part"),
        "错误信息必须点出缺少的表：{error}"
    );
    assert!(!lock_path(&db_path).exists());
}

#[test]
fn missing_migration_ledger_is_schema_unsupported() {
    let root = TempRoot::new("schema-missing-ledger");
    let db_path = root.db_path();
    create_baseline_database(
        &db_path,
        &FixtureOptions {
            ledger: false,
            ..FixtureOptions::default()
        },
    );

    let error = SessionStore::open(&db_path).expect_err("缺账本必须 fail-closed");
    assert!(matches!(error, StoreError::StorageSchemaUnsupported { .. }));
    assert_eq!(error.code(), "storage_schema_unsupported");
    assert!(
        error.to_string().contains("schema_migration"),
        "错误信息必须点出账本：{error}"
    );
    assert!(!lock_path(&db_path).exists());
}

#[test]
fn empty_migration_ledger_is_schema_unsupported() {
    let root = TempRoot::new("schema-empty-ledger");
    let db_path = root.db_path();
    create_baseline_database(
        &db_path,
        &FixtureOptions {
            ledger_rows: 0,
            ..FixtureOptions::default()
        },
    );

    let error = SessionStore::open(&db_path).expect_err("空账本必须 fail-closed");
    assert!(matches!(error, StoreError::StorageSchemaUnsupported { .. }));
    assert!(
        error.to_string().contains("schema_migration"),
        "空账本错误信息必须点出账本：{error}"
    );
}

#[test]
fn failed_schema_check_releases_lock_and_allows_retry() {
    let root = TempRoot::new("schema-lock-release");
    let db_path = root.db_path();
    create_baseline_database(
        &db_path,
        &FixtureOptions {
            ledger: false,
            ..FixtureOptions::default()
        },
    );

    let error = SessionStore::open(&db_path).expect_err("缺账本必须 fail-closed");
    assert!(matches!(error, StoreError::StorageSchemaUnsupported { .. }));
    assert!(
        !lock_path(&db_path).exists(),
        "ensure_supported_schema 失败必须删除已抢占的锁文件"
    );

    // 补上账本后必须能立即打开：失败路径不能留下 stale lock。
    create_ledger(&db_path, 1);
    let store = SessionStore::open(&db_path).expect("补齐账本后应打开");
    drop(store);
    assert!(!lock_path(&db_path).exists());
}
