//! 保存路径与 TS 真源的逐步对拍。
//!
//! fixture 由 `pnpm hooks-save:fixtures` 生成：每个场景在临时目录里铺文件，输出中的绝对根
//! 替换成 `<root>`；本测试在自己的临时根下重放，并做同样的归一化后比较。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use workspace_hooks_service::{
    resolve_next_root_enabled, save_hooks, to_mode_hooks_events,
    write_workspace_hook_configured_toggle,
};

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

struct TempRoot(PathBuf);

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
            "zc-hooks-save-{}-{nanos}-{counter}",
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

/// 把 fixture 里的 `<root>` 占位符还原成本次临时根（统一用 `/`）。
fn restore(value: &Value, root: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace("<root>", root)),
        Value::Array(items) => Value::Array(items.iter().map(|item| restore(item, root)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), restore(item, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn write_file(root: &Path, relative: &str, content: &str) {
    let target = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).expect("创建父目录");
    }
    std::fs::write(target, content).expect("写入文件");
}

fn read_json(path: &Path) -> Value {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    }
}

fn load_fixture() -> Value {
    let base = env!("CARGO_MANIFEST_DIR");
    let raw = std::fs::read_to_string(format!("{base}/fixtures/hooks-save.json")).expect("读取 fixture");
    serde_json::from_str(&raw).expect("解析 fixture")
}

#[test]
fn pure_projections_match_ts() {
    let fixture = load_fixture();
    for (index, case) in fixture["pure"]["events"].as_array().unwrap().iter().enumerate() {
        let hooks = case["hooks"].as_array().unwrap();
        let actual = to_mode_hooks_events(hooks);
        assert_json(&format!("pure.events#{index}"), &case["expected"], &actual);
    }
    for (index, case) in fixture["pure"]["rootEnabled"].as_array().unwrap().iter().enumerate() {
        let existing = case["existing"].as_bool();
        let hooks = case["hooks"].as_array().unwrap();
        let expected = case["expected"].as_bool();
        assert_eq!(
            resolve_next_root_enabled(existing, hooks),
            expected,
            "pure.rootEnabled#{index}"
        );
    }
}

#[test]
fn save_hooks_matches_ts() {
    let fixture = load_fixture();
    for case in fixture["save"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let root = TempRoot::new();
        let root_text = root.path().to_string_lossy().replace('\\', "/");
        for file in case["files"].as_array().unwrap() {
            write_file(
                root.path(),
                file["path"].as_str().unwrap(),
                file["content"].as_str().unwrap(),
            );
        }
        let hooks = restore(&case["hooks"], &root_text);
        let hooks = hooks.as_array().unwrap();
        save_hooks(
            &root.path().join("ws"),
            hooks,
            &root.path().join("home"),
        )
        .expect("保存 hooks");
        let actual = case["expect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|relative| {
                let relative = relative.as_str().unwrap();
                (
                    relative.to_string(),
                    read_json(
                        &root
                            .path()
                            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR)),
                    ),
                )
            })
            .collect::<serde_json::Map<String, Value>>();
        assert_json(&format!("save[{name}]"), &case["result"], &Value::Object(actual));
    }
}

#[test]
fn configured_toggle_matches_ts() {
    let fixture = load_fixture();
    for case in fixture["toggle"]["scenarios"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let root = TempRoot::new();
        let root_text = root.path().to_string_lossy().replace('\\', "/");
        let config_path = root
            .path()
            .join(case["configPath"].as_str().unwrap().replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).expect("创建配置目录");
        }
        std::fs::write(&config_path, case["configBefore"].as_str().unwrap()).expect("写入配置");
        let snapshot = restore(&case["snapshot"], &root_text);
        let result = write_workspace_hook_configured_toggle(
            &config_path,
            &snapshot,
            case["reviewItemId"].as_str().unwrap(),
            case["enabled"].as_bool().unwrap(),
        );
        match (&case["error"], result) {
            (Value::Null, Ok(())) => {
                assert_json(&format!("toggle[{name}]"), &case["result"], &read_json(&config_path))
            }
            (error, Err(actual)) => {
                assert_eq!(
                    error["code"].as_str().unwrap(),
                    actual.code,
                    "toggle[{name}] 错误码"
                );
                assert_eq!(
                    error["message"].as_str().unwrap(),
                    actual.message,
                    "toggle[{name}] 错误文案"
                );
            }
            (expected, actual) => {
                panic!("toggle[{name}] 结果类别不一致：期望 {expected:?}，实得 {actual:?}")
            }
        }
    }
}
