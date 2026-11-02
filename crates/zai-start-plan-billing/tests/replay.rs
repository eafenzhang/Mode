//! 与 TS `zaiStartPlanBilling.ts` 纯逻辑的对拍。

use std::path::Path;

use serde_json::Value;
use zai_start_plan_billing::{
    build_zai_start_plan_balance_url_for, normalize_start_plan_expiry,
    resolve_zai_start_plan_balance_model_ids,
};

#[test]
fn matches_typescript_billing_helpers() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("billing.json");
    let text = std::fs::read_to_string(&path).expect("读取 billing.json");
    let fixture: Value = serde_json::from_str(&text).expect("解析 fixture");

    for (index, case) in fixture["urls"].as_array().expect("urls").iter().enumerate() {
        let actual = build_zai_start_plan_balance_url_for(
            case["baseUrl"].as_str().expect("baseUrl"),
            case["appVersion"].as_str().expect("appVersion"),
        )
        .expect("url");
        assert_eq!(actual, case["expected"], "urls[{index}]");
    }

    for (index, case) in fixture["modelIds"]
        .as_array()
        .expect("modelIds")
        .iter()
        .enumerate()
    {
        let actual: Vec<Value> = resolve_zai_start_plan_balance_model_ids(&case["payload"])
            .into_iter()
            .map(Value::String)
            .collect();
        assert_eq!(Value::Array(actual), case["expected"], "modelIds[{index}]");
    }

    for (index, case) in fixture["expiry"]
        .as_array()
        .expect("expiry")
        .iter()
        .enumerate()
    {
        let actual = normalize_start_plan_expiry(
            &case["payload"],
            case["responseTime"].as_f64(),
            case["now"].as_f64().expect("now"),
        );
        assert_eq!(actual, case["expected"], "expiry[{index}]");
    }
}
