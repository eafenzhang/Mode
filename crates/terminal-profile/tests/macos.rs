//! 与 TS macOS Terminal / iTerm2 plist 原始探测的对拍。

use std::path::Path;

use serde_json::Value;
use terminal_profile::{
    detect_iterm2_profile_from_raw, detect_macos_terminal_profile_from_raw,
    terminal_detected_profile_to_json,
};

#[test]
fn matches_typescript_macos_terminal_profiles() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("macos.json");
    let text = std::fs::read_to_string(&path).expect("读取 macos.json");
    let fixture: Value = serde_json::from_str(&text).expect("解析 fixture");
    for (index, case) in fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .enumerate()
    {
        let actual = match case["kind"].as_str().expect("kind") {
            "iterm2" => detect_iterm2_profile_from_raw(&case["plist"]),
            "terminal" => detect_macos_terminal_profile_from_raw(&case["plist"]),
            other => panic!("未知 kind: {other}"),
        };
        let actual = actual
            .as_ref()
            .map(terminal_detected_profile_to_json)
            .unwrap_or(Value::Null);
        assert_eq!(actual, case["expected"], "cases[{index}]");
    }
}
