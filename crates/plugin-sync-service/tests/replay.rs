//! TS 真源对拍：重放 `scripts/generate-plugin-sync-fixtures.ts` 的用例。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use plugin_sync_service::{
    check_remote_user_plugin_write_access, collect_local_user_plugin_candidates,
    list_remote_user_plugin_statuses, normalize_relative_path, PluginSyncService,
    PluginSyncServiceOptions, RemoteStatusQuery,
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
    for length in [12usize, 8] {
        if bytes.len() < length + 1 || bytes[bytes.len() - length - 1] != b'-' {
            continue;
        }
        let suffix = &text[text.len() - length..];
        if suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return format!("{}<hash{length}>", &text[..text.len() - length]);
        }
    }
    text.to_string()
}

fn normalize_value(root: &str, key: Option<&str>, value: &Value) -> Value {
    if key == Some("archive") {
        return Value::String("<archive>".to_string());
    }
    if key == Some("archiveBytes") {
        return Value::String("<bytes>".to_string());
    }
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
                .map(|item| normalize_value(root, None, item))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(child_key, child)| {
                    (
                        child_key.clone(),
                        normalize_value(root, Some(child_key), child),
                    )
                })
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

fn write_config(sandbox: &Path, home: &Path, dirs: &[String]) -> Result<(), String> {
    let _ = sandbox;
    let config_path = home.join(".mode").join("cli").join("config.json");
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
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
    std::fs::write(
        config_path,
        format!(
            "{}
",
            serde_json::to_string_pretty(&config).map_err(|error| error.to_string())?
        ),
    )
    .map_err(|error| error.to_string())
}

fn read_tree(root: &Path) -> Vec<(String, String)> {
    fn walk(dir: &Path, prefix: &str, output: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "marketplace-sources" {
                continue;
            }
            let absolute = entry.path();
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if absolute.is_dir() {
                walk(&absolute, &relative, output);
            } else {
                output.push((
                    relative,
                    std::fs::read_to_string(&absolute).unwrap_or_default(),
                ));
            }
        }
    }
    let mut output = Vec::new();
    if root.exists() {
        walk(root, "", &mut output);
    }
    output
}

fn read_config(home: &Path) -> Value {
    let path = home.join(".mode").join("cli").join("config.json");
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            serde_json::from_str(&raw).unwrap_or_else(|_| json!({ "error": "<json-error>" }))
        }
        Err(_) => Value::Null,
    }
}

fn run_case(case: &Value) {
    let name = case["name"].as_str().expect("case name");
    let sandbox = unique_sandbox();
    let root = sandbox.to_string_lossy().into_owned();
    let home = setup_case(&sandbox, &case["setup"]);
    let service = PluginSyncService::new(PluginSyncServiceOptions::new(home.clone()));
    let mut archives: Vec<Vec<u8>> = Vec::new();
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
            "export" => {
                let names: Vec<String> = op["pluginNames"]
                    .as_array()
                    .expect("pluginNames")
                    .iter()
                    .map(|name| name.as_str().expect("name").to_string())
                    .collect();
                match collect_local_user_plugin_candidates(&home) {
                    Ok(candidates) => {
                        let mut ids = Vec::with_capacity(names.len());
                        let mut missing = None;
                        for name in &names {
                            match candidates.iter().find(|candidate| &candidate.name == name) {
                                Some(candidate) => ids.push(candidate.id.clone()),
                                None => {
                                    missing = Some(name.clone());
                                    break;
                                }
                            }
                        }
                        match missing {
                            Some(name) => Err(format!("fixture candidate not found: {name}")),
                            None => service.export_plugins_archive(&ids).and_then(|result| {
                                archives.push(result.archive.clone());
                                serde_json::to_value(&result).map_err(|error| error.to_string())
                            }),
                        }
                    }
                    Err(message) => Err(message),
                }
            }
            "setDirs" => {
                let dirs: Vec<String> = op["dirs"]
                    .as_array()
                    .expect("dirs")
                    .iter()
                    .map(|dir| {
                        let relative = dir.as_str().expect("dir");
                        sandbox
                            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect();
                write_config(&sandbox, &home, &dirs).map(|()| json!({ "ok": true }))
            }
            "import" => {
                let index = op["from"].as_u64().expect("from") as usize;
                service
                    .import_plugins_archive(&archives[index], false)
                    .and_then(|result| {
                        serde_json::to_value(&result).map_err(|error| error.to_string())
                    })
            }
            "exportMarketplace" => {
                let marketplace_id = op["marketplaceId"].as_str().expect("marketplaceId");
                let names: Vec<String> = op["pluginNames"]
                    .as_array()
                    .expect("pluginNames")
                    .iter()
                    .map(|name| name.as_str().expect("name").to_string())
                    .collect();
                let source_path = op["sourcePath"].as_str().expect("sourcePath");
                let source = json!({
                    "source": "directory",
                    "path": sandbox
                        .join(source_path.replace('/', std::path::MAIN_SEPARATOR_STR))
                        .to_string_lossy()
                });
                service
                    .export_marketplace_source_archive(marketplace_id, &names, &source)
                    .and_then(|result| {
                        archives.push(result.archive.clone());
                        serde_json::to_value(&result).map_err(|error| error.to_string())
                    })
            }
            "importMarketplace" => {
                let index = op["from"].as_u64().expect("from") as usize;
                service
                    .import_marketplace_source_archive(&archives[index], false)
                    .and_then(|result| {
                        serde_json::to_value(&result).map_err(|error| error.to_string())
                    })
            }
            "importMarketplaceOverwrite" => {
                let index = op["from"].as_u64().expect("from") as usize;
                match service.import_marketplace_source_archive(&archives[index], true) {
                    Ok(_) => Err("overwrite unexpectedly succeeded".to_string()),
                    Err(message) => Err(message),
                }
            }
            "importOverwrite" => {
                let index = op["from"].as_u64().expect("from") as usize;
                match service.import_plugins_archive(&archives[index], true) {
                    Ok(_) => Err("overwrite unexpectedly succeeded".to_string()),
                    Err(message) => Err(message),
                }
            }
            other => panic!("{name}: unknown op {other}"),
        };
        let actual = match actual {
            Ok(value) => value,
            Err(message) => error_value(message),
        };
        assert_eq!(
            normalize_value(&root, None, &actual),
            normalize_value(&root, None, &expected[index]),
            "{name}: op #{index} ({kind}) mismatch"
        );
    }

    let expected_tree: Vec<(String, String)> = case["tree"]
        .as_array()
        .expect("tree")
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().expect("tree path").to_string(),
                entry["content"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        read_tree(&home.join(".mode").join("plugins")),
        expected_tree,
        "{name}: plugin tree mismatch"
    );
    assert_eq!(
        normalize_value(&root, None, &read_config(&home)),
        normalize_value(&root, None, &case["config"]),
        "{name}: config mismatch"
    );

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

#[test]
fn imports_ts_produced_archive() {
    use base64::Engine as _;

    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/plugin-sync.json")).expect("fixture json");
    let encoded = fixture["cases"][0]["archiveBase64"]
        .as_str()
        .expect("TS archive base64");
    let archive = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("decode archive");

    let sandbox = unique_sandbox();
    let home = sandbox.join("home");
    std::fs::create_dir_all(&home).expect("home");
    let service = PluginSyncService::new(PluginSyncServiceOptions::new(home.clone()));
    let result = service
        .import_plugins_archive(&archive, false)
        .expect("import TS archive");
    assert_eq!(result.results.len(), 2, "TS 归档包含两个插件");
    assert!(result
        .results
        .iter()
        .all(|item| item.status == plugin_sync_service::PluginSyncImportStatus::Synced));
    assert!(home
        .join(".mode")
        .join("plugins")
        .join("plugin-a")
        .join("skills")
        .join("demo")
        .join("SKILL.md")
        .exists());
    assert!(home
        .join(".mode")
        .join("plugins")
        .join("plugin-b")
        .join("hooks")
        .join("hooks.json")
        .exists());
    let _ = std::fs::remove_dir_all(&sandbox);
}

#[test]
fn imports_ts_marketplace_source_archive() {
    use base64::Engine as _;

    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/plugin-sync.json")).expect("fixture json");
    let encoded = fixture["cases"][0]["marketplaceArchiveBase64"]
        .as_str()
        .expect("TS marketplace archive base64");
    let archive = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("decode archive");

    let sandbox = unique_sandbox();
    let home = sandbox.join("home");
    std::fs::create_dir_all(&home).expect("home");
    let service = PluginSyncService::new(PluginSyncServiceOptions::new(home.clone()));
    let result = service
        .import_marketplace_source_archive(&archive, false)
        .expect("import TS marketplace archive");
    assert_eq!(result.marketplace_id, "mkt");
    assert_eq!(
        result.status,
        plugin_sync_service::PluginSyncMarketplaceImportStatus::Synced
    );
    let manifest_path = std::path::Path::new(&result.path).join("marketplace.json");
    let manifest = std::fs::read_to_string(manifest_path).expect("marketplace.json");
    assert!(manifest.contains("\"mkt\""));
    assert!(std::path::Path::new(&result.path)
        .join("plugins")
        .join("alpha")
        .join(".mode-plugin")
        .join("plugin.json")
        .exists());
    let _ = std::fs::remove_dir_all(&sandbox);
}
