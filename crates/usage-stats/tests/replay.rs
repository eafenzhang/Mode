//! 与 TS `bigmodelUsageMonitorRange.ts` 的对拍。

use std::path::Path;

use serde_json::Value;
use usage_stats::{
    build_usage_stats_snapshot_from_monitor, normalize_limits, pick_current_subscription_from_list,
    pick_primary_limit, resolve_coding_plan_usage_time_range, resolve_usage_time_range,
    CodingPlanRangeRequest, CodingPlanUsageRange, UsageStatsRange,
};

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("usage-time-ranges.json");
    let text = std::fs::read_to_string(&path).expect("读取 usage-time-ranges.json");
    serde_json::from_str(&text).expect("解析 fixture")
}

#[test]
fn matches_typescript_usage_time_ranges() {
    let fixture = fixture();
    for (index, case) in fixture["usageRanges"]
        .as_array()
        .expect("usageRanges")
        .iter()
        .enumerate()
    {
        let range = UsageStatsRange::parse(case["range"].as_str().expect("range"))
            .unwrap_or_else(|| panic!("未知 usage range"));
        let actual = resolve_usage_time_range(
            range,
            case["now"].as_i64().expect("now"),
            case["offsetSecs"].as_i64().expect("offsetSecs") as i32,
        );
        assert_eq!(actual.to_json(), case["expected"], "usageRanges[{index}]");
    }

    for (index, case) in fixture["quota"]["limits"]
        .as_array()
        .expect("quota.limits")
        .iter()
        .enumerate()
    {
        let limits = case.get("limits").filter(|value| !value.is_null());
        let actual: Vec<Value> = normalize_limits(limits)
            .iter()
            .map(|limit| limit.to_json())
            .collect();
        assert_eq!(
            Value::Array(actual),
            case["expected"],
            "quota.limits[{index}]"
        );
    }

    for (index, case) in fixture["quota"]["primary"]
        .as_array()
        .expect("quota.primary")
        .iter()
        .enumerate()
    {
        let limits = normalize_limits(case.get("limits").filter(|value| !value.is_null()));
        let actual = pick_primary_limit(&limits).map(|limit| limit.to_json());
        assert_eq!(
            actual.unwrap_or(Value::Null),
            case["expected"],
            "quota.primary[{index}]"
        );
    }

    for (index, case) in fixture["quota"]["subscriptions"]
        .as_array()
        .expect("quota.subscriptions")
        .iter()
        .enumerate()
    {
        let actual = pick_current_subscription_from_list(
            Some(&case["payload"]),
            case["offsetSecs"].as_i64().expect("offsetSecs") as i32,
        )
        .map(|subscription| subscription.to_json());
        assert_eq!(
            actual.unwrap_or(Value::Null),
            case["expected"],
            "quota.subscriptions[{index}]"
        );
    }

    for (index, case) in fixture["monitor"]
        .as_array()
        .expect("monitor")
        .iter()
        .enumerate()
    {
        let actual = build_usage_stats_snapshot_from_monitor(
            &case["request"],
            &case["modelData"],
            &case["toolData"],
            1_700_000_000_000,
        );
        assert_eq!(actual, case["expected"], "monitor[{index}]");
    }

    for (index, case) in fixture["codingRanges"]
        .as_array()
        .expect("codingRanges")
        .iter()
        .enumerate()
    {
        let request = &case["request"];
        let actual = resolve_coding_plan_usage_time_range(
            &CodingPlanRangeRequest {
                range: CodingPlanUsageRange::parse(
                    request["range"].as_str().expect("coding range"),
                )
                .unwrap_or_else(|| panic!("未知 coding range")),
                custom_start_date: request
                    .get("customStartDate")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                custom_end_date: request
                    .get("customEndDate")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
            case["now"].as_i64().expect("now"),
            case["offsetSecs"].as_i64().expect("offsetSecs") as i32,
        );
        let expected = &case["expected"];
        if expected["ok"].as_bool().expect("expected.ok") {
            assert_eq!(
                actual.expect("range").to_json(),
                expected["value"],
                "codingRanges[{index}]"
            );
        } else {
            assert_eq!(
                actual.expect_err("应当失败").message(),
                expected["error"].as_str().expect("error"),
                "codingRanges[{index}]"
            );
        }
    }
}
