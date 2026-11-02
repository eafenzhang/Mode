//! 与 TS 终端系统 profile 探测器的对拍。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde_json::{json, Value};
use terminal_profile::{
    detect_kitty_font_family_from_raw, detect_vs_code_terminal_font_family_from_raw,
    detect_windows_terminal_font_family_from_raw, TerminalDetectedProfile,
};

fn detected_to_json(profile: Option<TerminalDetectedProfile>) -> Value {
    let Some(profile) = profile else {
        return Value::Null;
    };
    let mut object = serde_json::Map::new();
    if let Some(font_family) = profile.font_family {
        object.insert("fontFamily".to_string(), json!(font_family));
    }
    if let Some(font_size) = profile.font_size {
        object.insert("fontSize".to_string(), json!(font_size));
    }
    if let Some(theme) = profile.theme {
        object.insert(
            "theme".to_string(),
            Value::Object(
                theme
                    .iter()
                    .map(|(key, value)| (key.clone(), json!(value)))
                    .collect(),
            ),
        );
    }
    Value::Object(object)
}

fn env_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .expect("env")
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or("").to_string()))
        .collect()
}

#[test]
fn matches_typescript_terminal_profile_detection() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("detect.json");
    let text = std::fs::read_to_string(&path).expect("读取 detect.json");
    let fixture: Value = serde_json::from_str(&text).expect("解析 fixture");
    for (index, case) in fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .enumerate()
    {
        let env = env_map(&case["env"]);
        let files: HashMap<String, String> = case["files"]
            .as_object()
            .expect("files")
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or("").to_string()))
            .collect();
        let actual = match case["kind"].as_str().expect("kind") {
            "windows" => detect_windows_terminal_font_family_from_raw(
                case["platform"].as_str().expect("platform"),
                &env,
                &|path| files.get(path).cloned(),
            ),
            "vscode" => {
                detect_vs_code_terminal_font_family_from_raw(&env, &|path| files.get(path).cloned())
            }
            "kitty" => detect_kitty_font_family_from_raw(&env, &|path| files.get(path).cloned()),
            other => panic!("未知 kind: {other}"),
        };
        assert_eq!(detected_to_json(actual), case["expected"], "cases[{index}]");
    }
}
