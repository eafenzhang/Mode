//! 与 TS Alacritty TOML/YAML 探测的对拍。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde_json::{json, Value};
use terminal_profile::detect_alacritty_font_family_from_raw;

fn detected_to_json(profile: Option<terminal_profile::TerminalDetectedProfile>) -> Value {
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

#[test]
fn matches_typescript_alacritty_detection() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("alacritty.json");
    let text = std::fs::read_to_string(&path).expect("读取 alacritty.json");
    let fixture: Value = serde_json::from_str(&text).expect("解析 fixture");
    for (index, case) in fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .enumerate()
    {
        let env: BTreeMap<String, String> = case["env"]
            .as_object()
            .expect("env")
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or("").to_string()))
            .collect();
        let files: HashMap<String, String> = case["files"]
            .as_object()
            .expect("files")
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or("").to_string()))
            .collect();
        let actual = detect_alacritty_font_family_from_raw(&env, &|path| files.get(path).cloned());
        assert_eq!(detected_to_json(actual), case["expected"], "cases[{index}]");
    }
}
