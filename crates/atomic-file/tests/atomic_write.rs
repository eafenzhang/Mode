//! 原子写与跨进程锁的行为用例（含并发写、废弃锁回收、超时）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use atomic_file::{
    acquire_file_lock, write_json_atomic, write_text_atomic, AtomicError, WriteOptions,
};

fn unique_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "mode-atomic-file-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn lock_dir(path: &Path) -> PathBuf {
    let mut raw = path.as_os_str().to_os_string();
    raw.push(".lock");
    PathBuf::from(raw)
}

#[test]
fn writes_atomically_and_creates_parent_directories() {
    let dir = unique_dir("basic");
    let target = dir.join("nested").join("config.json");
    write_text_atomic(&target, "{\"a\":1}", &WriteOptions::default()).expect("写入");
    assert_eq!(fs::read_to_string(&target).expect("读回"), "{\"a\":1}");
    assert!(!lock_dir(&target).exists(), "成功后必须释放锁目录");
    assert!(
        fs::read_dir(dir.join("nested"))
            .expect("列目录")
            .all(|entry| !entry.unwrap().file_name().to_string_lossy().ends_with(".tmp")),
        "不应留下临时文件"
    );
}

#[test]
fn concurrent_writers_never_leave_partial_content() {
    let dir = unique_dir("concurrent");
    let target = dir.join("config.json");
    let payloads: Vec<String> = (0..6).map(|index| "x".repeat(1000 + index)).collect();

    std::thread::scope(|scope| {
        for payload in &payloads {
            let target = target.clone();
            scope.spawn(move || {
                for _ in 0..8 {
                    write_text_atomic(&target, payload, &WriteOptions::default()).expect("并发写");
                }
            });
        }
    });

    let content = fs::read_to_string(&target).expect("读回");
    assert!(
        payloads.iter().any(|payload| payload == &content),
        "最终内容必须是某个完整写入，不能是半截：len={}",
        content.len()
    );
}

#[test]
fn removes_stale_temp_files_but_keeps_fresh_ones() {
    let dir = unique_dir("stale-temp");
    let target = dir.join("config.json");
    let stale = dir.join("config.json.1.1.dead.tmp");
    fs::write(&stale, "old").expect("造过期临时文件");

    let options = WriteOptions {
        temp_file_stale_ms: 0,
        ..WriteOptions::default()
    };
    write_text_atomic(&target, "new", &options).expect("写入");
    assert!(!stale.exists(), "超过阈值的同名临时文件应被清理");
}

#[test]
fn times_out_when_lock_is_held_by_a_live_process() {
    let dir = unique_dir("lock-timeout");
    let target = dir.join("config.json");
    let lock = lock_dir(&target);
    fs::create_dir_all(&lock).expect("造锁目录");
    fs::write(
        lock.join(format!("owner-{}.json", std::process::id())),
        format!(
            "{{\"pid\":{},\"createdAt\":{},\"token\":\"t\"}}",
            std::process::id(),
            now_ms()
        ),
    )
    .expect("写 owner");
    // 目录 mtime 也要够新，避免被「ownerless grace」误回收。
    let options = WriteOptions {
        lock_max_wait_ms: 150,
        lock_retry_delays_ms: vec![10],
        lock_ownerless_grace_ms: 0,
        ..WriteOptions::default()
    };
    let started = Instant::now();
    let error = write_text_atomic(&target, "x", &options).expect_err("应超时");
    assert!(matches!(error, AtomicError::LockTimeout { .. }), "{error:?}");
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(!target.exists(), "超时不得写入目标文件");
    assert!(lock.exists(), "活进程持有的锁不能被回收");
}

#[test]
fn reclaims_lock_owned_by_a_dead_process() {
    let dir = unique_dir("dead-owner");
    let target = dir.join("config.json");
    let lock = lock_dir(&target);
    fs::create_dir_all(&lock).expect("造锁目录");
    fs::write(
        lock.join("owner-999999.json"),
        "{\"pid\":999999,\"createdAt\":1,\"token\":\"dead\"}",
    )
    .expect("写 owner");

    write_text_atomic(&target, "ok", &WriteOptions::default()).expect("应回收废弃锁并写入");
    assert_eq!(fs::read_to_string(&target).expect("读回"), "ok");
}

#[test]
fn reclaims_ownerless_lock_directory_after_grace() {
    let dir = unique_dir("ownerless");
    let target = dir.join("config.json");
    fs::create_dir_all(lock_dir(&target)).expect("造空锁目录");
    let options = WriteOptions {
        lock_ownerless_grace_ms: 0,
        ..WriteOptions::default()
    };
    write_text_atomic(&target, "ok", &options).expect("空锁目录应被回收");
    assert_eq!(fs::read_to_string(&target).expect("读回"), "ok");
}

#[test]
fn json_writes_are_pretty_printed_without_trailing_newline() {
    let dir = unique_dir("json");
    let target = dir.join("settings.json");
    let value = serde_json::json!({ "b": 2, "a": 1 });
    write_json_atomic(&target, &value, &WriteOptions::default()).expect("写入");
    let text = fs::read_to_string(&target).expect("读回");
    assert!(!text.ends_with('\n'), "与 TS 的 JSON.stringify 一致：无尾随换行");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("可解析");
    assert_eq!(parsed, value);
}

#[test]
fn lock_can_be_acquired_and_released_explicitly() {
    let dir = unique_dir("explicit-lock");
    let target = dir.join("config.json");
    let options = WriteOptions::default();
    {
        let lock = acquire_file_lock(&target, &options).expect("取锁");
        assert!(lock_dir(&target).exists());
        drop(lock);
    }
    assert!(!lock_dir(&target).exists(), "drop 必须释放锁目录");
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
