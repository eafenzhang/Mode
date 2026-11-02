//! 与 TS `mode-api-retry-status.ts` 的对拍。

use std::path::Path;

use mode_api_retry::{
    is_mode_model_retry_recovery_progress_payload,
    mode_api_retry_from_model_network_status_payload, mode_api_retry_from_stream_recovery_payload,
    normalize_mode_api_retry_status, ApiRetryOutcome,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct Fixture {
    normalize: Vec<ValueCase>,
    stream: Vec<ValueCase>,
    network: Vec<PayloadCase>,
    progress: Vec<PayloadCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValueCase {
    name: String,
    has_value: bool,
    #[serde(default)]
    value: Value,
    expected: Value,
}

#[derive(Debug, Deserialize)]
struct PayloadCase {
    name: String,
    payload: Value,
    expected: Value,
}

fn outcome_value(outcome: ApiRetryOutcome) -> Value {
    match outcome {
        ApiRetryOutcome::Undefined => json!({ "kind": "undefined" }),
        ApiRetryOutcome::Null => json!({ "kind": "null" }),
        ApiRetryOutcome::Status(status) => {
            json!({ "kind": "status", "status": serde_json::to_value(status).expect("序列化状态") })
        }
    }
}

#[test]
fn matches_ts_mode_api_retry_status() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("mode-api-retry.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.normalize {
        let value = case.has_value.then_some(&case.value);
        assert_eq!(
            outcome_value(normalize_mode_api_retry_status(value)),
            case.expected,
            "{}: normalize",
            case.name
        );
    }
    for case in &fixture.stream {
        let value = case.has_value.then_some(&case.value);
        assert_eq!(
            outcome_value(mode_api_retry_from_stream_recovery_payload(value)),
            case.expected,
            "{}: stream",
            case.name
        );
    }
    for case in &fixture.network {
        let object = case.payload.as_object().expect("payload 必须是对象");
        assert_eq!(
            outcome_value(mode_api_retry_from_model_network_status_payload(object)),
            case.expected,
            "{}: network",
            case.name
        );
    }
    for case in &fixture.progress {
        let object = case.payload.as_object().expect("payload 必须是对象");
        assert_eq!(
            is_mode_model_retry_recovery_progress_payload(object),
            case.expected.as_bool().expect("expected 布尔"),
            "{}: progress",
            case.name
        );
    }
}
