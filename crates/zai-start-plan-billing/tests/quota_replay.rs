//! 与 TS Zai Start Plan subscription/quota/remaining helper 的对拍。

use std::path::Path;

use serde_json::{json, Value};
use zai_start_plan_billing::{
    build_zai_start_plan_quota, build_zai_start_plan_remaining, build_zai_start_plan_subscription,
    normalize_zai_start_plan_balance_limits, pick_current_zai_start_plan,
    pick_zai_start_plan_billing_cycle,
};

#[test]
fn matches_typescript_start_plan_quota_helpers() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("quota.json");
    let text = std::fs::read_to_string(&path).expect("读取 quota.json");
    let fixture: Value = serde_json::from_str(&text).expect("解析 fixture");
    let plans = &fixture["plans"];
    let balances = &fixture["balances"];

    assert_eq!(
        build_zai_start_plan_subscription(Some(plans)),
        fixture["subscription"]
    );
    assert_eq!(
        build_zai_start_plan_subscription(None),
        fixture["subscriptionEmpty"]
    );
    assert_eq!(
        Value::Array(normalize_zai_start_plan_balance_limits(
            Some(balances),
            Some(plans)
        )),
        fixture["limits"]
    );
    assert_eq!(
        Value::Array(normalize_zai_start_plan_balance_limits(
            Some(balances),
            None
        )),
        fixture["limitsWithoutPlans"]
    );
    assert_eq!(
        build_zai_start_plan_quota(Some(balances), Some(plans)),
        fixture["quota"]
    );
    assert_eq!(
        build_zai_start_plan_quota(Some(&json!([])), Some(plans)),
        fixture["quotaEmpty"]
    );
    assert_eq!(
        build_zai_start_plan_remaining(Some(balances)),
        fixture["remaining"]
    );
    assert_eq!(
        build_zai_start_plan_remaining(Some(&json!([]))),
        fixture["remainingEmpty"]
    );
    assert_eq!(
        pick_current_zai_start_plan(Some(plans)),
        fixture["pick"]["current"]
    );
    assert_eq!(
        pick_current_zai_start_plan(Some(&json!([]))),
        fixture["pick"]["none"]
    );
    assert_eq!(
        pick_current_zai_start_plan(Some(&json!([
            { "plan_id": "other", "name": "Other", "status": "active" }
        ]))),
        fixture["pick"]["nonStart"]
    );
    assert_eq!(
        pick_zai_start_plan_billing_cycle(&plans[0]),
        fixture["billingCycle"]
    );
}
