//! 读侧 `load_hooks` 与 TS 真源的对拍。

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use workspace_hooks_service::load_hooks;

fn json_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => a == b,
            _ => a == b,
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| json_equal(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a
                    .iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| json_equal(value, other)))
        }
        _ => left == right,
    }
}

fn assert_json(label: &str, expected: &Value, actual: &Value) {
    assert!(
        json_equal(expected, actual),
        "{label} 不一致\n  期望: {expected}\n  实得: {actual}"
    );
}

struct TempRoot(std::path::PathBuf);

/// 并行用例可能在同一时钟刻度取到相同纳秒值，仅靠时间戳会让两个临时根互相清理。
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl TempRoot {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "zc-hooks-load-{}-{nanos}-{counter}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("创建临时目录");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn normalize(value: &Value, root: &str) -> Value {
    match value {
        Value::String(text) => {
            let native = root.replace('/', std::path::MAIN_SEPARATOR_STR);
            Value::String(
                text.replace(root, "<root>")
                    .replace(&native, "<root>")
                    .replace('\\', "/"),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| normalize(item, root)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), normalize(item, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[test]
fn load_hooks_matches_ts() {
    let base = env!("CARGO_MANIFEST_DIR");
    let raw = std::fs::read_to_string(format!("{base}/fixtures/hooks-load.json")).expect("读取 fixture");
    let fixture: Value = serde_json::from_str(&raw).expect("解析 fixture");

    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let root = TempRoot::new();
        let root_text = root.path().to_string_lossy().replace('\\', "/");
        for file in case["files"].as_array().unwrap() {
            let relative = file["path"].as_str().unwrap();
            let content = file["content"]
                .as_str()
                .unwrap()
                .replace("<root>", &root_text);
            let target = root
                .path()
                .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("创建父目录");
            }
            std::fs::write(target, content).expect("写入文件");
        }

        let result = load_hooks(
            &root.path().join("ws"),
            &root.path().join("home"),
            case["discoveredAt"].as_str().unwrap(),
        );
        let actual = serde_json::json!({
            "hooks": result.hooks,
            "hooksEnabled": result.hooks_enabled,
            "snapshot": result.snapshot.unwrap_or(Value::Null),
            "trustStoreCorrupt": result.trust_store_corrupt,
        });
        let mut actual = normalize(&actual, &root_text);
        // 快照时间戳在 TS 侧取当前时钟，与 fixture 生成时刻无关；摘要不参与该字段。
        if let Some(snapshot) = actual.get_mut("snapshot") {
            if let Some(object) = snapshot.as_object_mut() {
                object.insert("discoveredAt".into(), Value::String("<discoveredAt>".into()));
            }
        }
        assert_json(&format!("load[{name}]"), &case["expected"], &actual);
    }
}
