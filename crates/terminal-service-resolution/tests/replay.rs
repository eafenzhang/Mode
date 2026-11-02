//! 与 TS 终端启动解析面的对拍。

use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;
use terminal_service_resolution::{
    resolve_terminal_cwd_with, resolve_terminal_env_with, resolve_terminal_shell_with,
    resolve_terminal_windows_pty_info_with, should_fallback_from_conpty_dll_with,
    terminal_env_to_json, windows_pty_info_to_json,
};

fn env_map(value: &Value) -> IndexMap<String, String> {
    value
        .as_object()
        .expect("env")
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or("").to_string()))
        .collect()
}

#[test]
fn matches_typescript_terminal_resolution() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("resolution.json");
    let text = std::fs::read_to_string(&path).expect("读取 resolution.json");
    let fixture: Value = serde_json::from_str(&text).expect("解析 fixture");
    for (index, case) in fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .enumerate()
    {
        let expected = &case["expected"];
        let actual = match case["kind"].as_str().expect("kind") {
            "windows-pty" => {
                let info = resolve_terminal_windows_pty_info_with(
                    case["platform"].as_str().expect("platform"),
                    case["release"].as_str().expect("release"),
                );
                info.map(|info| windows_pty_info_to_json(&info))
                    .unwrap_or(Value::Null)
            }
            "conpty" => Value::Array(
                case["errors"]
                    .as_array()
                    .expect("errors")
                    .iter()
                    .map(|error| {
                        Value::Bool(should_fallback_from_conpty_dll_with(
                            error.as_str().expect("error"),
                        ))
                    })
                    .collect(),
            ),
            "shell" => {
                let env = env_map(&case["env"]);
                let executable: Vec<String> = case["executable"]
                    .as_array()
                    .expect("executable")
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
                match resolve_terminal_shell_with(
                    case["platform"].as_str().expect("platform"),
                    &env,
                    |candidate| executable.iter().any(|path| path == candidate),
                ) {
                    Ok(value) => serde_json::json!({ "ok": true, "value": value }),
                    Err(error) => serde_json::json!({ "ok": false, "error": error }),
                }
            }
            "cwd" => {
                let env = env_map(&case["env"]);
                let directories: Vec<String> = case["directories"]
                    .as_array()
                    .expect("directories")
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
                match resolve_terminal_cwd_with(
                    case["cwd"].as_str(),
                    &env,
                    case["osHome"].as_str().unwrap_or(""),
                    |candidate| directories.iter().any(|path| path == candidate),
                ) {
                    Ok(value) => serde_json::json!({ "ok": true, "value": value }),
                    Err(error) => serde_json::json!({ "ok": false, "error": error }),
                }
            }
            "env" => terminal_env_to_json(&resolve_terminal_env_with(
                case["platform"].as_str().expect("platform"),
                &env_map(&case["env"]),
                case["pathDelimiter"]
                    .as_str()
                    .and_then(|value| value.chars().next())
                    .expect("pathDelimiter"),
            )),
            other => panic!("未知 kind: {other}"),
        };
        assert_eq!(actual, expected.clone(), "cases[{index}]");
    }
}
