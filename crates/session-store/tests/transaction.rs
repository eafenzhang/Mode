//! 写事务与单写者：message+parts 原子提交、失败回滚、第二个 writer fail-closed。

mod common;

use common::{
    count_rows, create_baseline_database, lock_path, read_raw_text, sample_message, sample_part,
    sample_session, FixtureOptions, TempRoot,
};
use serde_json::json;
use session_store::{SessionStore, StoreError};

fn open_fixture(label: &str) -> (TempRoot, SessionStore) {
    let root = TempRoot::new(label);
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());
    let store = SessionStore::open(&db_path).expect("打开测试库");
    (root, store)
}

#[test]
fn message_and_parts_are_atomic_and_roll_back_on_part_failure() {
    let (root, store) = open_fixture("tx-atomic");
    let db_path = root.db_path();
    store
        .create_session(sample_session("s1", r"C:\ws\a", 100, false))
        .expect("写入 session");

    let message = sample_message("m1", "s1", 0);
    let part = sample_part("p1", "m1", "s1", 0);

    // 同一批 parts 里重复主键会让第二条插入失败；整批必须回滚，不能留下半截消息。
    let error = store
        .append_message_with_parts(message.clone(), &[part.clone(), part.clone()])
        .expect_err("重复 part 必须失败");
    assert!(matches!(error, StoreError::Sqlite(_)));
    assert_eq!(count_rows(&db_path, "message"), 0, "message 不能留下");
    assert_eq!(count_rows(&db_path, "part"), 0, "part 不能留下");

    store
        .append_message_with_parts(message, &[part])
        .expect("合法批次必须成功");
    assert_eq!(count_rows(&db_path, "message"), 1);
    assert_eq!(count_rows(&db_path, "part"), 1);
}

#[test]
fn mismatched_part_is_rejected_before_any_write() {
    let (root, store) = open_fixture("tx-mismatch");
    let db_path = root.db_path();
    store
        .create_session(sample_session("s1", r"C:\ws\a", 100, false))
        .expect("写入 session");

    let message = sample_message("m1", "s1", 0);
    let mismatched = sample_part("p1", "other-message", "s1", 0);
    let error = store
        .append_message_with_parts(message, &[mismatched])
        .expect_err("part 归属不一致必须失败");
    assert!(matches!(error, StoreError::InvalidInput(_)));
    assert_eq!(count_rows(&db_path, "message"), 0);
    assert_eq!(count_rows(&db_path, "part"), 0);

    let other_session = sample_part("p2", "m1", "other-session", 0);
    let error = store
        .append_message_with_parts(sample_message("m1", "s1", 0), &[other_session])
        .expect_err("part 的 session_id 不一致必须失败");
    assert!(matches!(error, StoreError::InvalidInput(_)));
    assert_eq!(count_rows(&db_path, "message"), 0);
    assert_eq!(count_rows(&db_path, "part"), 0);
}

#[test]
fn duplicate_message_insert_keeps_the_original_row() {
    let (root, store) = open_fixture("tx-duplicate-message");
    let db_path = root.db_path();
    store
        .create_session(sample_session("s1", r"C:\ws\a", 100, false))
        .expect("写入 session");

    let original = sample_message("m1", "s1", 0);
    store.append_message(original.clone()).expect("首次写入");
    let original_raw = read_raw_text(&db_path, "message", "data", "m1");

    let mut duplicate = original;
    duplicate.data = json!({ "role": "user", "messageId": "m1", "changed": true });
    let error = store
        .append_message(duplicate)
        .expect_err("重复 message id 必须失败");
    assert!(matches!(error, StoreError::Sqlite(_)));
    assert_eq!(count_rows(&db_path, "message"), 1);
    assert_eq!(
        read_raw_text(&db_path, "message", "data", "m1"),
        original_raw,
        "失败写入不能覆盖已有行"
    );
}

#[test]
fn duplicate_part_insert_keeps_the_existing_row() {
    let (root, store) = open_fixture("tx-duplicate-part");
    let db_path = root.db_path();
    store
        .create_session(sample_session("s1", r"C:\ws\a", 100, false))
        .expect("写入 session");
    store
        .append_message(sample_message("m1", "s1", 0))
        .expect("写入 message");

    let part = sample_part("p1", "m1", "s1", 0);
    store.append_part(part.clone()).expect("首次写入 part");
    let error = store.append_part(part).expect_err("重复 part id 必须失败");
    assert!(matches!(error, StoreError::Sqlite(_)));
    assert_eq!(count_rows(&db_path, "part"), 1);
    assert_eq!(count_rows(&db_path, "message"), 1);
}

#[test]
fn second_writer_is_locked_out_and_lock_is_released_on_drop() {
    let root = TempRoot::new("tx-single-writer");
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());
    let lock = lock_path(&db_path);

    let first = SessionStore::open(&db_path).expect("第一个 writer");
    assert!(lock.exists(), "第一个 writer 必须创建锁文件");

    let error = SessionStore::open(&db_path).expect_err("第二个 writer 必须 fail-closed");
    assert!(
        matches!(error, StoreError::StorageLocked { .. }),
        "第二个 writer 应报 StorageLocked，实际：{error:?}"
    );
    assert_eq!(error.code(), "storage_locked");
    assert!(
        error.to_string().contains("agent-write.lock"),
        "锁错误必须能定位锁文件：{error}"
    );

    drop(first);
    assert!(!lock.exists(), "Drop 必须释放锁文件");

    let third = SessionStore::open(&db_path).expect("锁释放后可以重新打开");
    drop(third);
    assert!(!lock.exists());
}

#[test]
fn lock_file_records_owner_pid_and_open_time() {
    let root = TempRoot::new("tx-lock-owner");
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());

    let store = SessionStore::open(&db_path).expect("打开完整库");
    let content = std::fs::read_to_string(lock_path(&db_path)).expect("读取锁文件");
    assert!(
        content.contains(&format!("pid={}", std::process::id())),
        "锁文件必须记录 owner pid：{content}"
    );
    assert!(
        content.contains("opened_at_ms="),
        "锁文件必须记录打开时间：{content}"
    );
    drop(store);
    assert!(!lock_path(&db_path).exists());
}
