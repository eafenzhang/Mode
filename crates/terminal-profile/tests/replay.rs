//! 与 TS `composeTerminalFontProfile` 的对拍。

use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;
use terminal_profile::{compose_terminal_font_profile, TerminalDetectedProfile};

fn detected_profile(value: &Value) -> Option<TerminalDetectedProfile> {
    if value.is_null() {
        return None;
    }
    let theme = value.get("theme").and_then(Value::as_object).map(|theme| {
        theme
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or("").to_string()))
            .collect::<IndexMap<String, String>>()
    });
    Some(TerminalDetectedProfile {
        font_family: value
            .get("fontFamily")
            .and_then(Value::as_str)
            .map(str::to_string),
        font_size: value.get("fontSize").and_then(Value::as_f64),
        theme,
    })
}

#[test]
fn matches_typescript_terminal_profile_compose() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("compose.json");
    let text = std::fs::read_to_string(&path).expect("读取 compose.json");
    let fixture: Value = serde_json::from_str(&text).expect("解析 fixture");
    for (index, case) in fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .enumerate()
    {
        let detected = detected_profile(&case["detectedProfile"]);
        let actual = compose_terminal_font_profile(
            case["settings"]
                .get("terminalFontFamily")
                .and_then(Value::as_str),
            detected.as_ref(),
        )
        .to_json();
        assert_eq!(actual, case["expected"], "cases[{index}]");
    }
}
