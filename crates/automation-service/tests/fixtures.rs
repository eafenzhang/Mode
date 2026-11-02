//! 与 TS `AutomationService` 的对拍：`pnpm automation-service:fixtures` 生成的
//! `service-utc.json` / `service-asia-shanghai.json` 逐 op 重放。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use automation_repo::{AutomationRepo, AutomationScope};
use automation_service::{AutomationService, AutomationServiceOptions, CreateParams, UpdateParams};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    zone: String,
    iana: String,
    offset_secs: i32,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    now: i64,
    workspace_path: String,
    ops: Vec<Value>,
    expected: Vec<Value>,
}

struct TempDb(PathBuf);

impl TempDb {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-automation-service-fixture-{}-{}-{}.sqlite",
            label,
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("sqlite-shm"));
        let _ = std::fs::remove_file(self.0.with_extension("sqlite-wal"));
    }
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn normalize_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map.contains_key("automationId") {
                map.insert("automationId".to_string(), json!("<automation-id>"));
            }
            if map.contains_key("runId") {
                map.insert("runId".to_string(), json!("<run-id>"));
            }
            for child in map.values_mut() {
                normalize_value(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_value(item);
            }
        }
        _ => {}
    }
}

fn expected_is_stale(expected: &Value) -> bool {
    expected.get("staleOneShot") == Some(&Value::Bool(true))
}

/// 数字按 f64 比较（TS 的 15 与 Rust 的 15.0 是同一个值）。
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
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| json_equal(value, other)))
        }
        _ => left == right,
    }
}

fn is_stale_message(message: &str) -> bool {
    message.starts_with("一次性定时任务的目标时间（")
}

fn result_value<T: serde::Serialize>(result: Result<T, String>) -> Value {
    match result {
        Ok(value) => serde_json::to_value(value).expect("serialize result"),
        Err(message) => json!({ "error": message }),
    }
}

fn run_case(zone: &str, case: &Case, offset_secs: i32) {
    let db = TempDb::new(&format!("{zone}-{}", case.name));
    let now = case.now;
    let service = AutomationService::new(
        AutomationRepo::with_clock(&db.0, move || now),
        AutomationServiceOptions { offset_secs },
    );
    let scope = AutomationScope {
        workspace_path: Some(case.workspace_path.clone()),
        workspace_identity: None,
    };
    let mut automation_ids: Vec<String> = Vec::new();
    let mut run_ids: Vec<String> = Vec::new();

    assert_eq!(
        case.ops.len(),
        case.expected.len(),
        "{} / {}: ops 与 expected 数量不一致",
        zone,
        case.name
    );

    for (index, (op, expected)) in case.ops.iter().zip(case.expected.iter()).enumerate() {
        let kind = op
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{zone} / {} op {index}: 缺少 kind", case.name));
        let mut actual = match kind {
            "create" => {
                let params: CreateParams = serde_json::from_value(op["params"].clone())
                    .unwrap_or_else(|error| {
                        panic!(
                            "{zone} / {} op {index}: create 参数无法解析: {error}",
                            case.name
                        )
                    });
                let result = service.create(&params, &scope, now);
                automation_ids.push(
                    result
                        .as_ref()
                        .map(|automation| automation.automation_id.clone())
                        .unwrap_or_default(),
                );
                run_ids.push(String::new());
                result_value(result)
            }
            "update" => {
                let id = match op.get("fromOp").and_then(Value::as_u64) {
                    Some(from) => automation_ids[from as usize].clone(),
                    None => op["automationId"]
                        .as_str()
                        .unwrap_or_else(|| panic!("{zone} / {} op {index}: 缺少 id", case.name))
                        .to_string(),
                };
                let params: UpdateParams = serde_json::from_value(op["params"].clone())
                    .unwrap_or_else(|error| {
                        panic!(
                            "{zone} / {} op {index}: update 参数无法解析: {error}",
                            case.name
                        )
                    });
                let result = service.update(&id, &params, &scope, now);
                run_ids.push(String::new());
                result_value(result)
            }
            "list" => {
                run_ids.push(String::new());
                result_value(service.list(&scope))
            }
            "listAll" => {
                run_ids.push(String::new());
                result_value(service.list_all())
            }
            "setEnabled" => {
                let id = automation_ids[op["fromOp"].as_u64().expect("fromOp") as usize].clone();
                let enabled = op["enabled"].as_bool().expect("enabled");
                run_ids.push(String::new());
                result_value(
                    service
                        .set_enabled(&id, enabled, &scope)
                        .map(|()| Value::Null),
                )
            }
            "restart" => {
                let id = automation_ids[op["fromOp"].as_u64().expect("fromOp") as usize].clone();
                run_ids.push(String::new());
                result_value(service.restart(&id, &scope, now).map(|()| Value::Null))
            }
            "runNow" => {
                let id = automation_ids[op["fromOp"].as_u64().expect("fromOp") as usize].clone();
                let result = service.run_now(&id, &scope, now);
                run_ids.push(
                    result
                        .as_ref()
                        .ok()
                        .and_then(|claimed| claimed.as_ref())
                        .map(|claimed| claimed.run.run_id.clone())
                        .unwrap_or_default(),
                );
                result_value(result)
            }
            "listRuns" => {
                let id = automation_ids[op["fromOp"].as_u64().expect("fromOp") as usize].clone();
                run_ids.push(String::new());
                result_value(service.list_runs(&id, &scope))
            }
            "deleteRun" => {
                let id = run_ids[op["runFromOp"].as_u64().expect("runFromOp") as usize].clone();
                run_ids.push(String::new());
                result_value(service.delete_run(&id, &scope).map(|()| Value::Null))
            }
            "delete" => {
                let id = automation_ids[op["fromOp"].as_u64().expect("fromOp") as usize].clone();
                run_ids.push(String::new());
                result_value(service.delete(&id, &scope))
            }
            other => panic!("{zone} / {} op {index}: 未支持的 op {other}", case.name),
        };

        if expected_is_stale(expected) {
            let message = actual
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "{zone} / {} op {index}: 期望 stale 错误，实得 {actual}",
                        case.name
                    )
                });
            assert!(
                is_stale_message(message),
                "{zone} / {} op {index}: stale 错误文案不符: {message}",
                case.name
            );
            actual = expected.clone();
        } else {
            normalize_value(&mut actual);
        }

        assert!(
            json_equal(&actual, expected),
            "{zone} / {} op {index} ({kind}) 期望 {expected}，实得 {actual}",
            case.name
        );
    }
}

fn run_zone(file: &str) {
    let path = fixtures_dir().join(file);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture = serde_json::from_str(&raw)
        .unwrap_or_else(|error| panic!("解析 {} 失败: {error}", path.display()));
    for case in &fixture.cases {
        run_case(&fixture.zone, case, fixture.offset_secs);
    }
    assert_eq!(
        fixture.zone,
        file.trim_start_matches("service-")
            .trim_end_matches(".json")
    );
    assert!(!fixture.iana.is_empty());
}

#[test]
fn matches_ts_service_utc() {
    run_zone("service-utc.json");
}

#[test]
fn matches_ts_service_asia_shanghai() {
    run_zone("service-asia-shanghai.json");
}
