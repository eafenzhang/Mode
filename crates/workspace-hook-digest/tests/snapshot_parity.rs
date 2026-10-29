//! 条目解析与包级快照的 TS 对拍。
//!
//! fixture 由 `pnpm hook-snapshot:fixtures` 生成；输入全是纯数据（固定 `D:/ws` 路径与
//! 固定 `discoveredAt`），因此两端可以在不同机器上得到逐字节相同的摘要。

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;
use workspace_hook_config::{RuntimeRoot, SourceInput};
use workspace_hook_digest::{build_bundle_snapshot, resolve_entries};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    workspace_path: String,
    workspace_identity: String,
    discovered_at: String,
    sources: Vec<SourceJson>,
    runtime_root: RuntimeRootJson,
    entries: Value,
    snapshot: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceJson {
    canonical_path: String,
    base_dir: String,
    discovery_order: usize,
    config_file_kind: String,
    explicit_project_config: bool,
    editable: bool,
    hooks: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeRootJson {
    enabled: bool,
    timeout_ms: u64,
    max_output_bytes: u64,
}

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

fn config_kind(value: &str) -> &'static str {
    match value {
        "mode.json" => "mode.json",
        ".mode/config.json" => ".mode/config.json",
        "explicit" => "explicit",
        other => panic!("未知 configFileKind: {other}"),
    }
}

#[test]
fn matches_ts_snapshot_fixture() {
    let base = env!("CARGO_MANIFEST_DIR");
    let raw = std::fs::read_to_string(format!("{base}/fixtures/hook-snapshot.json")).expect("读取 fixture");
    let fixture: Fixture = serde_json::from_str(&raw).expect("解析 fixture");

    for case in &fixture.cases {
        let sources: Vec<SourceInput> = case
            .sources
            .iter()
            .map(|source| SourceInput {
                canonical_path: PathBuf::from(&source.canonical_path),
                base_dir: PathBuf::from(&source.base_dir),
                discovery_order: source.discovery_order,
                config_file_kind: config_kind(&source.config_file_kind),
                explicit_project_config: source.explicit_project_config,
                editable: source.editable,
                hooks: source.hooks.clone(),
            })
            .collect();
        let runtime_root = RuntimeRoot {
            enabled: case.runtime_root.enabled,
            timeout_ms: case.runtime_root.timeout_ms,
            max_output_bytes: case.runtime_root.max_output_bytes,
        };

        let entries: Vec<Value> = resolve_entries(&case.workspace_path, &sources, &runtime_root)
            .iter()
            .map(|entry| entry.to_json())
            .collect();
        assert_json(
            &format!("[{}] entries", case.name),
            &case.entries,
            &Value::Array(entries),
        );

        let snapshot = build_bundle_snapshot(
            &case.workspace_identity,
            &case.workspace_path,
            &sources,
            &runtime_root,
            &case.discovered_at,
        )
        .map(|snapshot| snapshot.to_json());
        match (&case.snapshot, snapshot) {
            (Value::Null, None) => {}
            (expected, Some(actual)) => {
                assert_json(&format!("[{}] snapshot", case.name), expected, &actual)
            }
            (expected, None) => panic!("[{}] 期望有快照却得到 None: {expected}", case.name),
        }
    }
}
