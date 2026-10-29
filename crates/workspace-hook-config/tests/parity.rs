//! 与 TS 真源的目录树对拍。
//!
//! fixture 由 `pnpm workspace-hook-config:fixtures` 生成：每个场景在临时目录里铺一棵树，
//! 输出里的绝对根被替换成 `<root>`；本测试在另一棵临时树下重放同样的树，做同样的归一化后比较。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use workspace_hook_config::{
    create_workspace_hook_source_input, discover_workspace_hook_config_paths,
    path_to_forward_slashes, read_workspace_hook_project_sources, resolve_workspace_hook_runtime_root,
    RuntimeRootInput,
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

impl TempRoot {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("zc-hooks-cfg-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).expect("创建临时目录");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn join(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn materialize(root: &TempRoot, tree: &Value) {
    for entry in tree.as_array().expect("tree 必须是数组") {
        let relative = entry["path"].as_str().expect("tree.path");
        let target = root.join(&relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        match entry.get("content").and_then(Value::as_str) {
            None => std::fs::create_dir_all(&target).expect("创建目录"),
            Some(content) => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).expect("创建父目录");
                }
                std::fs::write(&target, content).expect("写入文件");
            }
        }
    }
}

fn normalize(value: &Value, root_prefix: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(root_prefix, "<root>")),
        Value::Array(items) => Value::Array(items.iter().map(|item| normalize(item, root_prefix)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), normalize(item, root_prefix)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn runtime_root_input(value: &Value) -> Option<RuntimeRootInput> {
    if value.is_null() {
        return None;
    }
    Some(RuntimeRootInput {
        enabled: value.get("enabled").and_then(Value::as_bool),
        timeout_ms: value.get("timeoutMs").and_then(Value::as_f64),
        max_output_bytes: value.get("maxOutputBytes").and_then(Value::as_f64),
    })
}

#[test]
fn matches_ts_discovery_fixture() {
    let base = env!("CARGO_MANIFEST_DIR");
    let raw = std::fs::read_to_string(format!("{base}/fixtures/hooks-config.json")).expect("读取 fixture");
    let fixture: Value = serde_json::from_str(&raw).expect("解析 fixture");

    for scenario in fixture["scenarios"].as_array().expect("scenarios") {
        let name = scenario["name"].as_str().expect("name");
        let root = TempRoot::new();
        materialize(&root, &scenario["tree"]);
        let prefix = path_to_forward_slashes(root.path());

        let cwd = root.join(
            scenario["cwd"]
                .as_str()
                .expect("cwd")
                .replace('/', std::path::MAIN_SEPARATOR_STR)
                .as_str(),
        );
        let explicit = scenario["explicit"].as_str().map(|value| {
            root.join(value.replace('/', std::path::MAIN_SEPARATOR_STR).as_str())
        });

        let refs: Vec<Value> = discover_workspace_hook_config_paths(&cwd, explicit.as_deref())
            .iter()
            .map(|reference| reference.to_json())
            .collect();
        let (sources, errors) = read_workspace_hook_project_sources(&cwd, explicit.as_deref());
        let sources: Vec<Value> = sources.iter().map(|source| source.to_json()).collect();
        let errors: Vec<Value> = errors.iter().map(|error| error.to_json()).collect();

        assert_json(
            &format!("[{name}] refs"),
            &scenario["refs"],
            &normalize(&Value::Array(refs), &prefix),
        );
        assert_json(
            &format!("[{name}] sources"),
            &scenario["sources"],
            &normalize(&Value::Array(sources), &prefix),
        );
        assert_json(
            &format!("[{name}] errors"),
            &scenario["errors"],
            &normalize(&Value::Array(errors), &prefix),
        );
    }

    for (index, case) in fixture["runtimeRoots"].as_array().expect("runtimeRoots").iter().enumerate() {
        let roots: Vec<Option<RuntimeRootInput>> = case["roots"]
            .as_array()
            .expect("roots")
            .iter()
            .map(runtime_root_input)
            .collect();
        let actual = resolve_workspace_hook_runtime_root(&roots);
        let actual = serde_json::json!({
            "enabled": actual.enabled,
            "timeoutMs": actual.timeout_ms,
            "maxOutputBytes": actual.max_output_bytes,
        });
        assert_json(&format!("runtimeRoots#{index}"), &case["expected"], &actual);
    }

    for (index, case) in fixture["sourceInputs"].as_array().expect("sourceInputs").iter().enumerate() {
        let root = TempRoot::new();
        let prefix = path_to_forward_slashes(root.path());
        let input = &case["input"];
        let path = root.join(
            input["path"]
                .as_str()
                .expect("path")
                .trim_start_matches("<root>/")
                .replace('/', std::path::MAIN_SEPARATOR_STR)
                .as_str(),
        );
        let cwd = root.join(
            input["cwd"]
                .as_str()
                .expect("cwd")
                .trim_start_matches("<root>/")
                .replace('/', std::path::MAIN_SEPARATOR_STR)
                .as_str(),
        );
        let hooks = case["expected"]["hooks"].clone();
        let actual = create_workspace_hook_source_input(
            &path,
            &cwd,
            hooks,
            case["expected"]["discoveryOrder"].as_u64().unwrap_or(0) as usize,
            input["explicit"].as_bool().unwrap_or(false),
        )
        .to_json();
        assert_json(
            &format!("sourceInputs#{index}"),
            &case["expected"],
            &normalize(&actual, &prefix),
        );
    }
}
