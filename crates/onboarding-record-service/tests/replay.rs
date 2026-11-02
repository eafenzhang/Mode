//! 与 TS `createOnboardingRecordService` 的对拍：
//! `pnpm onboarding-record:fixtures` 生成的 `fixtures/records.json` 逐 op 重放。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use onboarding_record_service::{
    iso8601_utc, OnboardingPreferencesPatch, OnboardingRecordEntryInput,
    OnboardingRecordEnvironment, OnboardingRecordService,
};
use serde::Deserialize;
use serde_json::{json, Value};

const NOW_MS: i64 = 1_772_323_200_123;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    ops: Vec<Value>,
    expected: Vec<Value>,
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-onboarding-record-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }

    fn record_path(&self) -> PathBuf {
        self.0
            .join(".mode")
            .join("v2")
            .join("onboarding-record.json")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct MutableEnvironment {
    user_id: Mutex<Option<String>>,
    has_task: Mutex<bool>,
}

impl OnboardingRecordEnvironment for MutableEnvironment {
    fn load_user_id(&self) -> Result<Option<String>, String> {
        Ok(self.user_id.lock().unwrap().clone())
    }

    fn has_existing_local_task(&self) -> Result<bool, String> {
        Ok(*self.has_task.lock().unwrap())
    }
}

fn is_iso_at(bytes: &[u8], start: usize) -> bool {
    start + 24 <= bytes.len()
        && bytes[start + 4] == b'-'
        && bytes[start + 7] == b'-'
        && bytes[start + 10] == b'T'
        && bytes[start + 13] == b':'
        && bytes[start + 16] == b':'
        && bytes[start + 19] == b'.'
        && bytes[start + 23] == b'Z'
        && (0..24).all(|offset| {
            matches!(offset, 4 | 7 | 10 | 13 | 16 | 19 | 23)
                || bytes[start + offset].is_ascii_digit()
        })
}

/// 夹具里的时间戳统一成 `<ts>`（含 rawFile 文本内嵌的时间）。
fn normalize_string(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if is_iso_at(bytes, index) {
            output.push_str("<ts>");
            index += 24;
        } else {
            let ch = text[index..].chars().next().expect("valid utf-8 boundary");
            output.push(ch);
            index += ch.len_utf8();
        }
    }
    output
}

fn normalize_value(value: &mut Value) {
    match value {
        Value::String(text) => *text = normalize_string(text),
        Value::Array(items) => {
            for item in items {
                normalize_value(item);
            }
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                normalize_value(child);
            }
        }
        _ => {}
    }
}

fn result_value<T: serde::Serialize>(result: Result<T, String>) -> Value {
    match result {
        Ok(value) => serde_json::to_value(value).expect("serialize result"),
        Err(message) => json!({ "error": message }),
    }
}

fn run_case(case: &Case) {
    let dir = TempDir::new(&case.name);
    let record_path = dir.record_path();
    let environment = Arc::new(MutableEnvironment::default());
    let service = OnboardingRecordService::new(
        &record_path,
        Arc::clone(&environment) as Arc<dyn OnboardingRecordEnvironment>,
        Arc::new(|| NOW_MS),
    );
    assert!(
        iso8601_utc(NOW_MS).starts_with("2026-03-01T"),
        "固定时钟口径变更"
    );
    assert_eq!(
        case.ops.len(),
        case.expected.len(),
        "{}: ops 与 expected 数量不一致",
        case.name
    );

    for (index, (op, expected)) in case.ops.iter().zip(case.expected.iter()).enumerate() {
        let kind = op
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{} op {index}: 缺少 kind", case.name));
        let mut actual = match kind {
            "setUser" => {
                *environment.user_id.lock().unwrap() =
                    op.get("userId").and_then(Value::as_str).map(str::to_string);
                Value::Null
            }
            "setHasTask" => {
                *environment.has_task.lock().unwrap() =
                    op.get("hasTask").and_then(Value::as_bool).unwrap_or(false);
                Value::Null
            }
            "append" => {
                let device_mid = op["deviceMid"].as_str().expect("deviceMid");
                let entry: OnboardingRecordEntryInput =
                    serde_json::from_value(op["entry"].clone()).expect("entry input");
                result_value(
                    service
                        .append_record(device_mid, &entry)
                        .map(|()| Value::Null),
                )
            }
            "claim" => result_value(service.claim_anonymous_record().map(|()| Value::Null)),
            "shouldOnboard" => {
                let device_mid = op["deviceMid"].as_str().expect("deviceMid");
                result_value(service.should_onboard(device_mid))
            }
            "dismiss" => {
                let device_mid = op["deviceMid"].as_str().expect("deviceMid");
                result_value(service.dismiss_onboarding(device_mid).map(|()| Value::Null))
            }
            "latest" => result_value(service.get_latest_entry()),
            "syncSettings" => result_value(service.sync_settings_from_record()),
            "updatePrefs" => {
                let patch: OnboardingPreferencesPatch =
                    serde_json::from_value(op.get("patch").cloned().unwrap_or_else(|| json!({})))
                        .expect("preferences patch");
                result_value(
                    service
                        .update_record_preferences(&patch)
                        .map(|()| Value::Null),
                )
            }
            "getRecords" => result_value(service.get_records()),
            "rawFile" => match std::fs::read_to_string(&record_path) {
                Ok(text) => Value::String(text),
                Err(error) => json!({ "error": error.to_string() }),
            },
            "clear" => result_value(service.clear_records().map(|()| Value::Null)),
            "setupFile" => {
                let content = op["content"].as_str().expect("content");
                let parent = record_path.parent().expect("record parent");
                std::fs::create_dir_all(parent).expect("create record dir");
                std::fs::write(&record_path, content).expect("seed record file");
                Value::Null
            }
            other => panic!("{} op {index}: 未支持的 op {other}", case.name),
        };
        normalize_value(&mut actual);
        assert_eq!(
            actual, *expected,
            "{} op {index} ({kind}) 期望 {expected}，实得 {actual}",
            case.name
        );
    }
}

#[test]
fn matches_ts_onboarding_record_service() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("records.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));
    for case in &fixture.cases {
        run_case(case);
    }
}
