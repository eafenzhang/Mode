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
fn two_stores_share_the_database_and_sqlite_serializes_writes() {
    let root = TempRoot::new("tx-shared-writers");
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());

    // 控制器裁决 (a)：不再有 `<db>.agent-write.lock`；两个 store 都必须能打开。
    let first = SessionStore::open(&db_path).expect("第一个 workspace writer");
    let second = SessionStore::open(&db_path).expect("第二个 workspace writer");
    assert!(
        !lock_path(&db_path).exists(),
        "SessionStore 不应再创建进程独占锁文件"
    );

    first
        .create_session(sample_session("s1", "C:/ws/a", 100, false))
        .expect("第一个 writer 写入");
    second
        .create_session(sample_session("s2", "C:/ws/b", 100, false))
        .expect("第二个 writer 写入");
    assert_eq!(count_rows(&db_path, "session"), 2);
    assert!(second.get_session("s1").expect("读").is_some());
    assert!(first.get_session("s2").expect("读").is_some());

    // 并发写：两个连接各自 BEGIN IMMEDIATE，busy_timeout 内必须全部成功。
    std::thread::scope(|scope| {
        let first_ref = &first;
        let second_ref = &second;
        let left = scope.spawn(move || {
            for index in 0..10 {
                first_ref
                    .create_session(sample_session(&format!("a{index}"), "C:/ws/a", 200, false))
                    .expect("并发写 a");
            }
        });
        let right = scope.spawn(move || {
            for index in 0..10 {
                second_ref
                    .create_session(sample_session(&format!("b{index}"), "C:/ws/b", 200, false))
                    .expect("并发写 b");
            }
        });
        left.join().expect("左线程");
        right.join().expect("右线程");
    });
    assert_eq!(count_rows(&db_path, "session"), 22);
}

/// Node 路径与 Rust store 重叠：Node 不识别任何 Rust 锁文件，二者共享同一 SQLite；
/// 真正的双写防护是「一个 workspace 一个 Agent」的进程调度，不是库文件锁。
#[test]
fn node_connection_and_rust_store_overlap_on_the_same_database() {
    let root = TempRoot::new("tx-node-overlap");
    let db_path = root.db_path();
    create_baseline_database(&db_path, &FixtureOptions::default());

    // 模拟 Node 侧长期打开的一个连接（读已迁移库 + 写一行）。
    let node = rusqlite::Connection::open(&db_path).expect("Node 侧连接");
    node.busy_timeout(std::time::Duration::from_secs(5))
        .expect("busy_timeout");
    node.execute(
        "INSERT INTO session (id, project_id, slug, directory, title, version, \
         time_created, time_updated) \
         VALUES ('node-row', 'proj_node', 'node-row', 'C:/ws/node', 'node', '0.0.0', 1, 1)",
        [],
    )
    .expect("Node 写入");

    // Rust store 在 Node 连接仍打开时通过 schema 门并写入。
    let store = SessionStore::open(&db_path).expect("Rust store 打开");
    store
        .create_session(sample_session("rust-row", "C:/ws/rust", 100, false))
        .expect("Rust 写入");
    assert_eq!(count_rows(&db_path, "session"), 2);

    // 双方都能看到对方的行；没有遗留锁文件。
    let node_count: i64 = node
        .query_row("SELECT COUNT(*) FROM session", [], |row| row.get(0))
        .expect("Node 读");
    assert_eq!(node_count, 2);
    let rust_row = store
        .get_session("node-row")
        .expect("Rust 读")
        .expect("行存在");
    assert_eq!(rust_row.directory, "C:/ws/node");
    assert!(!lock_path(&db_path).exists());
}
