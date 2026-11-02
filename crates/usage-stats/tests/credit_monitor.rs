//! 与 TS `buildCodingPlanUsageSnapshotFromMonitor` 的对拍。

use std::path::Path;

use serde_json::Value;
use usage_stats::build_coding_plan_usage_snapshot_from_monitor;

#[test]
fn matches_typescript_coding_plan_monitor() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("coding-plan-monitor.json");
    let text = std::fs::read_to_string(&path).expect("读取 coding-plan-monitor.json");
    let fixture: Value = serde_json::from_str(&text).expect("解析 fixture");
    let now = fixture["now"].as_i64().expect("now");
    for (index, case) in fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .enumerate()
    {
        let actual = build_coding_plan_usage_snapshot_from_monitor(&case["params"], now);
        assert_eq!(actual, case["expected"], "cases[{index}]");
    }
}
