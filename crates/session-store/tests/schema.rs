//! schema 门（LT-1 C2 T7 后的口径）：
//!
//! - fresh-install（文件缺失 / 空 schema）由迁移 runner 创建（`tests/fresh_install.rs`
//!   覆盖与 oracle 的逐字节对齐）；
//! - 非 fresh 形态保持 C1 的 fail-closed：schema 存在但缺账本/账本为空、冻结迁移
//!   checksum 不一致，一律 [`session_store::StoreError::StorageSchemaUnsupported`]。

mod common;

use common::{create_baseline_database, create_ledger, FixtureOptions, TempRoot};
use session_store::{SessionStore, StoreError};

#[test]
fn opens_database_created_from_schema_baseline() {
    let root = TempRoot::new("schema-ok");
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());

    // 夹具账本只有占位行：首次打开按 upgrade 补齐冻结迁移（幂等 DDL），随后可打开。
    let store = SessionStore::open(&db_path).expect("基线库必须能打开");
    store
        .ensure_supported_schema()
        .expect("显式复查 schema 仍应通过");
    assert_eq!(store.db_path(), db_path.as_path());
    drop(store);
}

#[test]
fn checksum_mismatch_is_schema_unsupported() {
    let root = TempRoot::new("schema-checksum");
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());
    let connection = rusqlite::Connection::open(&db_path).expect("打开库改账本");
    connection
        .execute(
            "INSERT INTO schema_migration (id, checksum, app_version, time_applied) \
             VALUES ('0002_local_setting', 'checksum-from-another-impl', '0.2.0', 1)",
            [],
        )
        .expect("写入冲突账本行");
    drop(connection);

    let error = SessionStore::open(&db_path).expect_err("checksum 不一致必须 fail-closed");
    assert!(
        matches!(error, StoreError::StorageSchemaUnsupported { .. }),
        "应报 StorageSchemaUnsupported，实际：{error:?}"
    );
    assert_eq!(error.code(), "storage_schema_unsupported");
    assert!(
        error.to_string().contains("0002_local_setting"),
        "错误信息必须点出冲突的迁移 id：{error}"
    );
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
fn failed_schema_check_does_not_leave_stale_state_and_allows_retry() {
    let root = TempRoot::new("schema-retry");
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

    // 补上账本后必须能立即打开：失败路径不能留下 stale 进程锁（C1 已无锁文件）。
    create_ledger(&db_path, 1);
    let store = SessionStore::open(&db_path).expect("补齐账本后应打开");
    drop(store);
}
