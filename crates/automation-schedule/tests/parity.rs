//! 与 TS 真源的逐步对拍。
//!
//! fixture 由 `pnpm automation-schedule:fixtures` 生成：同一个进程内切 `TZ`，分别产出
//! UTC（偏移 0）与 Asia/Shanghai（+08:00）两份用例；Rust 侧用同一份固定偏移重放。

use automation_schedule::{
    assert_valid_automation_interval_carrier, build_interval_schedule_rule,
    build_relative_delay_schedule, compute_automation_next_run_at,
    compute_initial_automation_next_run_at, compute_next_run_at, compute_previous_runs,
    compute_schedule_rule_next_run_at, force_interval_carrier_recurring,
    infer_minute_interval_schedule_rule, is_one_shot_automation, is_valid_cron_expr,
    schedule_rule_definition, CarrierInput, IntervalUnit, ScheduleRule,
};
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Deserialize)]
struct Fixture {
    zone: String,
    #[serde(rename = "offsetSecs")]
    offset_secs: i32,
    cases: Value,
}

fn number(value: &Value, key: &str) -> i64 {
    value
        .get(key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("缺少数字字段 {key}: {value}"))
}

fn numeric(value: &Value, key: &str) -> f64 {
    value
        .get(key)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("缺少数字字段 {key}: {value}"))
}

fn opt_number(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

fn rule_from(value: &Value) -> ScheduleRule {
    serde_json::from_value(value.clone()).unwrap_or_else(|error| panic!("规则解析失败: {error}"))
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

fn assert_option(label: &str, expected: Option<i64>, actual: Option<i64>) {
    assert_eq!(expected, actual, "{label} 不一致");
}

fn run_zone(fixture: Fixture) {
    let zone = fixture.zone.clone();
    let offset = fixture.offset_secs;
    let cases = &fixture.cases;
    let label = |name: &str, index: usize| format!("[{zone}] {name}#{index}");

    for (index, case) in cases["oneShot"].as_array().unwrap().iter().enumerate() {
        let recurring = case["recurring"].as_bool().unwrap();
        let max_runs = case.get("maxRuns").and_then(Value::as_i64);
        assert_eq!(
            is_one_shot_automation(recurring, max_runs),
            case["expected"].as_bool().unwrap(),
            "{}",
            label("oneShot", index)
        );
    }

    for (index, case) in cases["definitions"].as_array().unwrap().iter().enumerate() {
        let rule = rule_from(&case["rule"]);
        assert_eq!(
            schedule_rule_definition(&rule),
            case["expected"].as_str().unwrap(),
            "{}",
            label("definitions", index)
        );
    }

    for (index, case) in cases["relativeDelay"].as_array().unwrap().iter().enumerate() {
        let (cron_expr, rule) = build_relative_delay_schedule(
            numeric(case, "delayMinutes"),
            number(case, "from"),
            offset,
        );
        let expected = &case["expected"];
        assert_eq!(
            cron_expr,
            expected["cronExpr"].as_str().unwrap(),
            "{}",
            label("relativeDelay.cronExpr", index)
        );
        assert_json(
            &label("relativeDelay.scheduleRule", index),
            &expected["scheduleRule"],
            &serde_json::to_value(&rule).unwrap(),
        );
    }

    for (index, case) in cases["inferMinute"].as_array().unwrap().iter().enumerate() {
        let actual = infer_minute_interval_schedule_rule(
            case["cronExpr"].as_str().unwrap(),
            number(case, "anchorAt"),
            offset,
        );
        let expected = &case["expected"];
        if expected.is_null() {
            assert!(actual.is_none(), "{}", label("inferMinute", index));
        } else {
            assert_json(
                &label("inferMinute", index),
                expected,
                &serde_json::to_value(actual.unwrap()).unwrap(),
            );
        }
    }

    for (index, case) in cases["buildIntervalRules"].as_array().unwrap().iter().enumerate() {
        let unit: IntervalUnit = serde_json::from_value(case["unit"].clone()).unwrap();
        let actual = build_interval_schedule_rule(
            unit,
            numeric(case, "interval"),
            case["cronExpr"].as_str().unwrap(),
            number(case, "anchorAt"),
            offset,
        );
        assert_json(
            &label("buildIntervalRules", index),
            &case["expected"],
            &serde_json::to_value(&actual).unwrap(),
        );
    }

    for (index, case) in cases["cron"].as_array().unwrap().iter().enumerate() {
        let expr = case["expr"].as_str().unwrap();
        for (run_index, run) in case["runs"].as_array().unwrap().iter().enumerate() {
            let from = number(run, "from");
            assert_option(
                &format!("{} from={from}", label("cron.next", index)),
                opt_number(run, "next"),
                compute_next_run_at(expr, from, offset),
            );
            if let Some(previous) = run["previous"].as_array() {
                let expected: Vec<i64> = previous.iter().map(|value| value.as_i64().unwrap()).collect();
                let actual = compute_previous_runs(expr, from, expected.len(), offset);
                assert_eq!(
                    expected,
                    actual,
                    "{} run#{run_index}",
                    label("cron.previous", index)
                );
            }
        }
    }

    for (index, case) in cases["scheduleRuleNext"].as_array().unwrap().iter().enumerate() {
        let rule = rule_from(&case["rule"]);
        for run in case["runs"].as_array().unwrap() {
            let from = number(run, "from");
            assert_option(
                &format!("{} from={from}", label("scheduleRuleNext", index)),
                opt_number(run, "next"),
                compute_schedule_rule_next_run_at(&rule, from, offset),
            );
        }
    }

    for (index, case) in cases["initialNext"].as_array().unwrap().iter().enumerate() {
        let input = &case["input"];
        let rule = input.get("scheduleRule").and_then(|value| {
            if value.is_null() {
                None
            } else {
                Some(rule_from(value))
            }
        });
        let result = compute_initial_automation_next_run_at(
            input["cronExpr"].as_str().unwrap(),
            input["recurring"].as_bool().unwrap(),
            rule.as_ref(),
            number(input, "from"),
            offset,
        );
        match (&case["error"], result) {
            (Value::Null, Ok(next)) => assert_option(
                &label("initialNext", index),
                opt_number(case, "next"),
                next,
            ),
            (error, Err(stale)) => {
                assert_eq!(
                    error["name"].as_str().unwrap(),
                    "StaleOneShotAutomationScheduleError",
                    "{}",
                    label("initialNext", index)
                );
                assert_eq!(
                    error["targetAt"].as_i64(),
                    Some(stale.target_at),
                    "{} 目标时刻",
                    label("initialNext", index)
                );
            }
            (expected, actual) => panic!(
                "{} 结果类别不一致：期望 {expected:?}，实得 {actual:?}",
                label("initialNext", index)
            ),
        }
    }

    for (index, case) in cases["automationNext"].as_array().unwrap().iter().enumerate() {
        let rule = case.get("scheduleRule").and_then(|value| {
            if value.is_null() {
                None
            } else {
                Some(rule_from(value))
            }
        });
        assert_option(
            &label("automationNext", index),
            opt_number(case, "next"),
            compute_automation_next_run_at(
                case["cronExpr"].as_str().unwrap(),
                rule.as_ref(),
                number(case, "from"),
                offset,
            ),
        );
    }

    for (index, case) in cases["fuzz"].as_array().unwrap().iter().enumerate() {
        let expr = case["expr"].as_str().unwrap();
        assert_eq!(
            is_valid_cron_expr(expr),
            case["valid"].as_bool().unwrap(),
            "{}",
            label("fuzz.valid", index)
        );
        for run in case["runs"].as_array().unwrap() {
            let from = number(run, "from");
            assert_option(
                &format!("{} expr={expr} from={from}", label("fuzz.next", index)),
                opt_number(run, "next"),
                compute_next_run_at(expr, from, offset),
            );
        }
    }

    for (index, case) in cases["validCron"].as_array().unwrap().iter().enumerate() {
        let expr = case["expr"].as_str().unwrap();
        assert_eq!(
            is_valid_cron_expr(expr),
            case["expected"].as_bool().unwrap(),
            "{}",
            label("validCron", index)
        );
    }

    for (index, case) in cases["carrier"].as_array().unwrap().iter().enumerate() {
        let input: CarrierInput = case["input"].as_object().unwrap().clone();
        let actual = assert_valid_automation_interval_carrier(&input);
        match &case["error"] {
            Value::Null => assert!(actual.is_ok(), "{} 期望通过：{actual:?}", label("carrier", index)),
            expected => {
                let error = actual.expect_err("期望校验失败");
                assert_eq!(
                    error.to_string(),
                    expected.as_str().unwrap(),
                    "{} 错误文案",
                    label("carrier", index)
                );
            }
        }
    }

    for (index, case) in cases["forceRecurring"].as_array().unwrap().iter().enumerate() {
        let input: CarrierInput = case["input"].as_object().unwrap().clone();
        let actual = force_interval_carrier_recurring(&input);
        let actual_value = Value::Object(actual);
        let expected: &Map<String, Value> = case["expected"].as_object().unwrap();
        assert_json(
            &label("forceRecurring", index),
            &Value::Object(expected.clone()),
            &actual_value,
        );
    }
}

fn load(path: &str) -> Fixture {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{path}: {error}"));
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("{path} 解析失败: {error}"))
}

#[test]
fn matches_ts_golden_cases() {
    let base = env!("CARGO_MANIFEST_DIR");
    for zone in ["utc", "asia-shanghai"] {
        let path = format!("{base}/fixtures/schedule-{zone}.json");
        run_zone(load(&path));
    }
}
