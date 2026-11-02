//! 与 TS `integratedTerminalShells.ts` 的对拍。

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use integrated_terminal_shells::list_integrated_terminal_shell_options;
use serde_json::Value;

#[test]
fn matches_typescript_shell_options() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("shell-options.json");
    let text = std::fs::read_to_string(&path).expect("读取 shell-options.json");
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
        let existing: HashSet<String> = case["existing"]
            .as_array()
            .expect("existing")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let actual: Vec<Value> = list_integrated_terminal_shell_options(
            &env,
            case["platform"].as_str().expect("platform"),
            |candidate| existing.contains(candidate),
        )
        .iter()
        .map(|option| option.to_json())
        .collect();
        assert_eq!(Value::Array(actual), case["expected"], "cases[{index}]");
    }
}
