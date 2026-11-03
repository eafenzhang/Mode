//! 与 TS 自适应码率/画质降档状态机的对拍。

use std::path::Path;

use remote_desktop_streaming::{
    adaptive_bitrate_state_to_json, adaptive_bitrate_step_to_json, initial_adaptive_bitrate_state,
    next_lower_quality, quality_downshift_step_to_json, step_adaptive_bitrate,
    step_quality_downshift, AdaptiveBitrateSample, AdaptiveBitrateState, QualityDownshiftState,
    RemoteDesktopQuality,
};
use serde_json::Value;

fn read_fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("adaptive.json");
    let text = std::fs::read_to_string(&path).expect("读取 adaptive.json");
    serde_json::from_str(&text).expect("解析 fixture")
}

fn state_from_json(value: &Value) -> AdaptiveBitrateState {
    AdaptiveBitrateState {
        current_bps: value["currentBps"].as_i64().expect("currentBps"),
        high_bps: value["highBps"].as_i64().expect("highBps"),
        low_bps: value["lowBps"].as_i64().expect("lowBps"),
        last_switch_at: value["lastSwitchAt"].as_i64(),
        healthy_streak: value["healthyStreak"].as_i64().expect("healthyStreak"),
    }
}

fn sample_from_json(value: &Value) -> (AdaptiveBitrateSample, i64) {
    (
        AdaptiveBitrateSample {
            rtt_ms: value["rttMs"].as_f64(),
            dropped_frames: value["droppedFrames"].as_i64().expect("droppedFrames"),
        },
        value["nowMs"].as_i64().expect("nowMs"),
    )
}

fn quality_state_from_json(value: &Value) -> QualityDownshiftState {
    QualityDownshiftState {
        congested_streak: value["congestedStreak"].as_i64().expect("congestedStreak"),
        last_downshift_at: value["lastDownshiftAt"].as_i64(),
    }
}

fn quality_from_str(value: &str) -> RemoteDesktopQuality {
    RemoteDesktopQuality::parse(value).expect("合法画质档位")
}

#[test]
fn matches_typescript_initial_state() {
    let fixture = read_fixture();
    for (index, case) in fixture["initial"]
        .as_array()
        .expect("initial")
        .iter()
        .enumerate()
    {
        let state = initial_adaptive_bitrate_state(
            case["highBps"].as_i64().expect("highBps"),
            case["lowBps"].as_i64().expect("lowBps"),
        );
        assert_eq!(
            adaptive_bitrate_state_to_json(&state),
            case["expected"].clone(),
            "initial[{index}]"
        );
    }
}

#[test]
fn matches_typescript_adaptive_bitrate_steps() {
    let fixture = read_fixture();
    for (index, case) in fixture["adaptive"]
        .as_array()
        .expect("adaptive")
        .iter()
        .enumerate()
    {
        let mut state = state_from_json(&case["initialState"]);
        let steps = case["steps"].as_array().expect("steps");
        let expected_states = case["states"].as_array().expect("states");
        let expected_next = case["nextBps"].as_array().expect("nextBps");
        for (step_index, step) in steps.iter().enumerate() {
            let (sample, now_ms) = sample_from_json(step);
            let result = step_adaptive_bitrate(&state, &sample, now_ms);
            let actual = adaptive_bitrate_step_to_json(&result);
            assert_eq!(
                actual["state"], expected_states[step_index],
                "adaptive[{index}].states[{step_index}]"
            );
            assert_eq!(
                actual["nextBps"], expected_next[step_index],
                "adaptive[{index}].nextBps[{step_index}]"
            );
            state = result.state;
        }
    }
}

#[test]
fn matches_typescript_lower_quality_chain() {
    let fixture = read_fixture();
    for (index, case) in fixture["lowerQuality"]
        .as_array()
        .expect("lowerQuality")
        .iter()
        .enumerate()
    {
        let actual =
            next_lower_quality(quality_from_str(case["quality"].as_str().expect("quality")));
        let expected = match case["expected"].as_str() {
            Some(quality) => Value::String(quality.to_string()),
            None => Value::Null,
        };
        let actual = match actual {
            Some(quality) => Value::String(quality.as_str().to_string()),
            None => Value::Null,
        };
        assert_eq!(actual, expected, "lowerQuality[{index}]");
    }
}

#[test]
fn matches_typescript_quality_downshift_steps() {
    let fixture = read_fixture();
    for (index, case) in fixture["qualityDownshift"]
        .as_array()
        .expect("qualityDownshift")
        .iter()
        .enumerate()
    {
        let mut state = quality_state_from_json(&case["initialState"]);
        let steps = case["steps"].as_array().expect("steps");
        let expected_states = case["states"].as_array().expect("states");
        let expected_downshift = case["downshift"].as_array().expect("downshift");
        for (step_index, step) in steps.iter().enumerate() {
            let (sample, now_ms) = sample_from_json(step);
            let result = step_quality_downshift(&state, &sample, now_ms);
            let actual = quality_downshift_step_to_json(&result);
            assert_eq!(
                actual["state"], expected_states[step_index],
                "qualityDownshift[{index}].states[{step_index}]"
            );
            assert_eq!(
                actual["downshift"], expected_downshift[step_index],
                "qualityDownshift[{index}].downshift[{step_index}]"
            );
            state = result.state;
        }
    }
}
