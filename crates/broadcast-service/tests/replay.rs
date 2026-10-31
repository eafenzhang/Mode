//! fixture 回放对拍（`docs/specs/broadcast-service.md`）。
//!
//! `pnpm broadcast:fixtures` 用 TS 真源执行 claim 状态机与 send 用例；
//! token / retryAfterMs 归一后逐项深比较。

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use broadcast_service::{BroadcastService, ClaimLease};

const FIXTURE: &str = include_str!("../fixtures/broadcast.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    name: String,
    ops: Vec<Op>,
    expected: Vec<Value>,
}

#[derive(Deserialize)]
struct Op {
    kind: String,
    key: Option<String>,
    message: Option<Value>,
}

fn normalize_value(key: Option<&str>, value: &Value) -> Value {
    match value {
        Value::String(_) if key == Some("token") => Value::String("<token>".to_string()),
        Value::Number(_) if key == Some("retryAfterMs") => Value::String("<retry>".to_string()),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_value(None, item))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (child_key, child) in map {
                out.insert(child_key.clone(), normalize_value(Some(child_key), child));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn run_op(
    service: &Arc<BroadcastService>,
    leases: &mut HashMap<String, ClaimLease>,
    op: &Op,
) -> Value {
    match op.kind.as_str() {
        "send" => match service.send(op.message.as_ref().unwrap_or(&Value::Null)) {
            Ok(_) => json!({ "ok": true }),
            Err(message) => json!({ "error": message }),
        },
        "acquire" => {
            let key = op.key.clone().unwrap_or_default();
            let result = service.acquire_claim(&key);
            if let broadcast_service::ClaimAcquireResult::Acquired { lease } = &result {
                leases.insert(key, lease.clone());
            }
            serde_json::to_value(&result).expect("claim result serializes")
        }
        "commit" => {
            if let Some(lease) = leases.get(op.key.as_deref().unwrap_or("")) {
                service.commit_claim(lease);
            }
            json!({ "ok": true })
        }
        "release" => {
            if let Some(lease) = leases.get(op.key.as_deref().unwrap_or("")) {
                service.release_claim(lease);
            }
            json!({ "ok": true })
        }
        "tryClaim" => Value::Bool(service.try_claim(op.key.as_deref().unwrap_or(""))),
        other => panic!("fixture 只包含已知操作，遇到 {other}"),
    }
}

#[test]
fn replay_ts_fixtures() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
    let mut failures = Vec::new();
    for case in &fixture.cases {
        let service = Arc::new(BroadcastService::new());
        let mut leases = HashMap::new();
        if case.expected.len() != case.ops.len() {
            failures.push(format!("[case {}] ops 与 expected 数量不一致", case.name));
            continue;
        }
        for (index, op) in case.ops.iter().enumerate() {
            let actual = normalize_value(None, &run_op(&service, &mut leases, op));
            let expected = normalize_value(None, &case.expected[index]);
            if actual != expected {
                failures.push(format!(
                    "[case {}] op[{index}] {} 结果不一致\n--- expected ---\n{}\n--- actual ---\n{}",
                    case.name,
                    op.kind,
                    serde_json::to_string_pretty(&expected).unwrap_or_default(),
                    serde_json::to_string_pretty(&actual).unwrap_or_default(),
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "fixture 回放存在差异：\n{}",
        failures.join("\n\n")
    );
}
