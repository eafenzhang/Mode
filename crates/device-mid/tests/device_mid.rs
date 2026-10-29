//! deviceMid 的行为用例：生成、复用、保留未知字段、坏文件重建、并发一致。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use device_mid::DeviceMidStore;

fn unique_root(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "mode-device-mid-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("v2")).expect("建 v2 目录");
    root
}

fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && value.chars().enumerate().all(|(index, ch)| {
            matches!(index, 8 | 13 | 18 | 23) || ch.is_ascii_hexdigit()
        })
        && value.starts_with(&value[0..8])
        && value.as_bytes()[14] == b'4'
        && matches!(value.as_bytes()[19], b'8' | b'9' | b'a' | b'b')
}

#[test]
fn creates_a_uuid_and_reuses_it() {
    let root = unique_root("create");
    let store = DeviceMidStore::new(&root);
    let first = store.ensure().expect("生成 deviceMid");
    assert!(is_uuid_v4(&first), "不是合法 UUID v4：{first}");
    assert_eq!(store.ensure().expect("再次读取"), first);
    assert_eq!(store.ensure().expect("第三次读取"), first);
}

#[test]
fn preserves_other_state_fields() {
    let root = unique_root("preserve");
    let file = root.join("v2/telemetry-state.json");
    fs::write(&file, "{\"telemetryOptOut\":true}").expect("写已有状态");

    let store = DeviceMidStore::new(&root);
    let id = store.ensure().expect("生成");
    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).expect("读回")).expect("合法 JSON");
    assert_eq!(state["deviceMid"], id);
    assert_eq!(state["telemetryOptOut"], true, "未知字段必须保留");
}

#[test]
fn rewrites_a_corrupt_state_file() {
    let root = unique_root("corrupt");
    let file = root.join("v2/telemetry-state.json");
    fs::write(&file, ":wq").expect("写坏文件");

    let store = DeviceMidStore::new(&root);
    let id = store.ensure().expect("坏文件应重建");
    assert!(is_uuid_v4(&id));
    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).expect("读回")).expect("重建后必须合法");
    assert_eq!(state["deviceMid"], id);
}

#[test]
fn concurrent_callers_agree_on_one_id() {
    let root = unique_root("concurrent");
    let mut handles = Vec::new();
    for _ in 0..4 {
        let root = root.clone();
        handles.push(std::thread::spawn(move || {
            DeviceMidStore::new(&root).ensure().expect("生成")
        }));
    }
    let ids: Vec<String> = handles
        .into_iter()
        .map(|handle| handle.join().expect("线程未 panic"))
        .collect();
    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "并发调用必须得到同一个 id：{ids:?}"
    );
}
