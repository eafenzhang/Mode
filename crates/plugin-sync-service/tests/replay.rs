//! TS 真源对拍：重放 `scripts/generate-plugin-sync-fixtures.ts` 的用例。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use plugin_sync_service::{
    check_remote_user_plugin_write_access, collect_local_user_plugin_candidates,
    list_remote_user_plugin_statuses, normalize_relative_path, RemoteStatusQuery,
};
use serde_json::{json, Value};

static CASE_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn unique_sandbox() -> PathBuf {
    let index = CASE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "zc-plugin-sync-replay-{}-{index}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create sandbox");
    plugin_discovery::js::realpath(&path).expect("realpath sandbox")
}

fn materialize(sandbox: &Path, files: &Value) {
    for file in files.as_array().cloned().unwrap_or_default() {
        let relative = file["path"].as_str().expect("file path");
        let target = sandbox.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(target, file["content"].as_str().unwrap_or_default()).expect("write file");
    }
}

fn normalize_text(root: &str, text: &str) -> String {
    let forward = root.replace('\\', "/");
    let escaped = root.replace('\\', "\\\\");
    let mut replaced = text.replace(&escaped, "<root>");
    replaced = replaced.replace(root, "<root>");
    replaced = replaced.replace(&forward, "<root>");
    if cfg!(windows) {
        replaced = replaced.replace("\\\\", "/").replace('\\', "/");
    }
    replaced
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn replace_short_hash_suffix(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.len() < 9 || bytes[bytes.len() - 9] != b'-' {
        return text.to_string();
    }
    let suffix = &text[text.len() - 8..];
    if suffix
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        format!("{}<hash8>", &text[..text.len() - 8])
    } else {
        text.to_string()
    }
}

fn normalize_value(root: &str, value: &Value) -> Value {
    match value {
        Value::String(text) => {
            let replaced = normalize_text(root, text);
            if is_sha256(&replaced) {
                Value::String("<sha256>".to_string())
            } else {
                Value::String(replace_short_hash_suffix(&replaced))
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_value(root, item))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, child)| (key.clone(), normalize_value(root, child)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn error_value(message: String) -> Value {
    if message.starts_with("invalid json object") {
        json!({ "error": "<json-error>" })
    } else {
        json!({ "error": message })
    }
}

fn setup_case(sandbox: &Path, setup: &Value) -> PathBuf {
    let home = sandbox.join("home");
    std::fs::create_dir_all(&home).expect("home");
    materialize(sandbox, &setup["files"]);
    if setup["skipConfig"].as_bool().unwrap_or(false) {
        return home;
    }
    let config_path = home.join(".mode").join("cli").join("config.json");
    std::fs::create_dir_all(config_path.parent().unwrap()).expect("config dir");
    if setup["invalidConfig"].as_bool().unwrap_or(false) {
        std::fs::write(config_path, "{not json").expect("invalid config");
        return home;
    }
    let dirs: Vec<String> = setup["dirs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|dir| {
            let relative = dir.as_str().expect("dir");
            sandbox
                .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let config = json!({
        "plugins": {
            "dirs": dirs,
            "enabledPlugins": {
                "plugin a@inline": false,
                "pluginb@inline": true,
                "not-a-bool@inline": "yes"
            }
        }
    });
    std::fs::write(config_path, format!("{config}\n")).expect("write config");
    home
}

fn run_case(case: &Value) {
    let name = case["name"].as_str().expect("case name");
    let sandbox = unique_sandbox();
    let root = sandbox.to_string_lossy().into_owned();
    let home = setup_case(&sandbox, &case["setup"]);
    let expected = case["expected"].as_array().expect("expected");
    let ops = case["ops"].as_array().expect("ops");
    assert_eq!(expected.len(), ops.len(), "{name}: op/expected length");

    for (index, op) in ops.iter().enumerate() {
        let kind = op["kind"].as_str().expect("op kind");
        let actual: Result<Value, String> = match kind {
            "candidates" => collect_local_user_plugin_candidates(&home).map(|candidates| {
                json!({
                    "candidates": serde_json::to_value(&candidates).expect("candidates json"),
                    "maxArchiveBytes": 50_u64 * 1024 * 1024
                })
            }),
            "statuses" => {
                let queries: Vec<RemoteStatusQuery> = op["plugins"]
                    .as_array()
                    .expect("plugins")
                    .iter()
                    .map(|plugin| RemoteStatusQuery {
                        plugin_id: plugin["pluginId"].as_str().expect("pluginId").to_string(),
                        directory_name: plugin["directoryName"]
                            .as_str()
                            .expect("directoryName")
                            .to_string(),
                    })
                    .collect();
                list_remote_user_plugin_statuses(&home, &queries).map(|statuses| {
                    json!({ "statuses": serde_json::to_value(&statuses).expect("statuses json") })
                })
            }
            "writeAccess" => Ok(
                serde_json::to_value(check_remote_user_plugin_write_access(&home))
                    .expect("write access json"),
            ),
            other => panic!("{name}: unknown op {other}"),
        };
        let actual = match actual {
            Ok(value) => value,
            Err(message) => error_value(message),
        };
        assert_eq!(
            normalize_value(&root, &actual),
            normalize_value(&root, &expected[index]),
            "{name}: op #{index} ({kind}) mismatch"
        );
    }

    // 相对路径校验与平台无关，顺带覆盖安全边界。
    assert!(normalize_relative_path("../escape").is_err());
    assert!(normalize_relative_path("a\\b").is_err());
    assert!(normalize_relative_path("a/b").is_ok());
    let _ = std::fs::remove_dir_all(&sandbox);
}

#[test]
fn replay_fixtures() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/plugin-sync.json")).expect("fixture json");
    let cases = fixture["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        run_case(case);
    }
}
