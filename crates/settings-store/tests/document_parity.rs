//! 与 TS zod schema 生成的结果对拍：默认值、合并、打补丁。

use serde_json::Value;
use settings_store::{allowed_keys, apply_patch, defaults, merge_persisted, normalize};

#[derive(serde::Deserialize)]
struct Fixture {
    #[serde(rename = "allowedKeys")]
    allowed_keys: Vec<String>,
    defaults: Value,
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    persisted: Value,
    patch: Value,
    expected: Value,
}

fn load() -> Fixture {
    serde_json::from_str(include_str!("../fixtures/settings.json")).expect("fixture 必须是合法 JSON")
}

#[test]
fn defaults_and_allowed_keys_match_ts_schema() {
    let fixture = load();
    assert_eq!(defaults(), &fixture.defaults, "默认值必须与 zod 一致");
    assert_eq!(allowed_keys().len(), fixture.allowed_keys.len());
    for key in &fixture.allowed_keys {
        assert!(allowed_keys().contains(key), "缺少顶层键 {key}");
    }
}

#[test]
fn merge_and_patch_match_ts_expected_results() {
    let fixture = load();
    assert!(!fixture.cases.is_empty());
    for case in &fixture.cases {
        let current = if case.persisted.is_null() {
            defaults().clone()
        } else {
            merge_persisted(&case.persisted)
        };
        let actual = apply_patch(&current, &case.patch);
        assert_eq!(
            actual, case.expected,
            "{}：合并结果与 TS 不一致",
            case.name
        );
    }
}

#[test]
fn drops_unknown_top_level_keys_but_keeps_allowed_optional_ones() {
    let persisted = serde_json::json!({
        "locale": "en-US",
        "notARealSetting": 1,
        "dataBaseDir": "D:/ModeData"
    });
    let merged = merge_persisted(&persisted);
    assert_eq!(merged["locale"], "en-US");
    assert_eq!(merged["dataBaseDir"], "D:/ModeData");
    assert!(merged.get("notARealSetting").is_none());
}

#[test]
fn type_mismatch_falls_back_to_default_for_that_field_only() {
    // 与 TS 的整体回落不同（见 spec「两处差异」），这里只回落坏字段，其余保留。
    let persisted = serde_json::json!({ "locale": 123, "userNickname": "mode" });
    let merged = merge_persisted(&persisted);
    assert_eq!(merged["locale"], defaults()["locale"]);
    assert_eq!(merged["userNickname"], "mode");
}

#[test]
fn recent_projects_are_deduplicated_and_capped_by_normalize() {
    // 与 TS 一致：读取（parse）不去重，只有 update 的收尾会去重并截断。
    let persisted = serde_json::json!({
        "recentProjects": ["/a", "/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h", "/i", "/j", "/k"]
    });
    let merged = normalize(merge_persisted(&persisted));
    let projects = merged["recentProjects"].as_array().expect("数组");
    assert_eq!(projects.len(), 10);
    assert_eq!(projects[0], "/a");
    assert_eq!(projects[1], "/b", "重复项必须被去掉");
}
