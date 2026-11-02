//! 数据根初始化器对拍：`pnpm data-root-initializer:fixtures`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use data_root::{
    discover_legacy_data_root_candidates, resolve_data_root_action_from_env, DataRootImportResult,
    DataRootInitializer, DataRootManifestCreatedBy, DataRootNonInteractiveAction,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Fixture {
    action: Vec<ActionCase>,
    cases: Vec<InitCase>,
}

#[derive(Debug, Deserialize)]
struct ActionCase {
    value: Option<String>,
    expected: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitCase {
    name: String,
    kind: String,
    base: String,
    files: BTreeMap<String, String>,
    dirs: Vec<String>,
    action: Option<String>,
    cancel: bool,
    expected: InitExpected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitExpected {
    result: Value,
    active_diagnostic_root: Option<String>,
    tree: Vec<Value>,
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-data-root-init-fixture-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn materialize(base: &Path, files: &BTreeMap<String, String>, dirs: &[String]) {
    std::fs::create_dir_all(base).expect("create base");
    for dir in dirs {
        std::fs::create_dir_all(base.join(dir)).expect("create dir");
    }
    for (relative, content) in files {
        let target = base.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&target, content.replace("<root>", &base.to_string_lossy()))
            .expect("write file");
    }
}

fn normalize_text(text: &str, root: &str, env_base: &str, tmp_dir: &str) -> String {
    text.replace(tmp_dir, "<tmp>")
        .replace(env_base, "<root>")
        .replace(&env_base.replace('\\', "\\\\"), "<root>")
        .replace(root, "<root>")
        .replace(&root.replace('\\', "\\\\"), "<root>")
        .replace("2026-01-01T00:00:00.000Z", "<ts>")
        .replace("20260101-000000", "<ts>")
        .replace("mode-startup\\4242", "mode-startup/<pid>")
        .replace("mode-startup/4242", "mode-startup/<pid>")
}

fn normalize_value(value: &mut Value, root: &str, env_base: &str, tmp_dir: &str) {
    match value {
        Value::String(text) => {
            *text = normalize_text(text, root, env_base, tmp_dir);
        }
        Value::Array(items) => {
            for item in items {
                normalize_value(item, root, env_base, tmp_dir);
            }
        }
        Value::Object(map) => {
            for child in map.values_mut() {
                normalize_value(child, root, env_base, tmp_dir);
            }
        }
        _ => {}
    }
}

fn tree_snapshot(base: &Path, root: &str, env_base: &str, tmp_dir: &str) -> Vec<Value> {
    fn walk(
        dir: &Path,
        relative: &str,
        root: &str,
        env_base: &str,
        tmp_dir: &str,
        entries: &mut Vec<Value>,
    ) {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };
        let mut children: Vec<_> = read_dir.flatten().collect();
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_relative = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let target = dir.join(&name);
            let path = normalize_text(&child_relative, root, env_base, tmp_dir);
            if target.is_dir() {
                entries.push(serde_json::json!({ "path": path, "kind": "dir" }));
                walk(&target, &child_relative, root, env_base, tmp_dir, entries);
            } else if target.is_file() {
                let content = std::fs::read_to_string(&target).unwrap_or_default();
                entries.push(serde_json::json!({
                    "path": path,
                    "kind": "file",
                    "content": normalize_text(&content, root, env_base, tmp_dir),
                }));
            }
        }
    }
    let mut entries = Vec::new();
    walk(base, "", root, env_base, tmp_dir, &mut entries);
    entries.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["path"].as_str().unwrap_or_default())
    });
    entries
}

fn action_name(action: DataRootNonInteractiveAction) -> &'static str {
    match action {
        DataRootNonInteractiveAction::Fresh => "fresh",
        DataRootNonInteractiveAction::Migrate => "migrate",
        DataRootNonInteractiveAction::Fail => "fail",
    }
}

#[test]
fn matches_ts_data_root_initializer() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("initializer.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.action {
        let mut env = BTreeMap::new();
        if let Some(value) = &case.value {
            env.insert("MODE_DATA_ROOT_ACTION".to_string(), value.clone());
        }
        assert_eq!(
            action_name(resolve_data_root_action_from_env(&env)),
            case.expected,
            "action {:?}",
            case.value
        );
    }

    let temp = TempDir::new();
    let root = temp.0.clone();
    let env_base_path = root.join("env-base");
    std::fs::create_dir_all(&env_base_path).expect("create env base");
    let env_base_text = env_base_path.to_string_lossy().into_owned();
    let root_text = root.to_string_lossy().into_owned();
    let tmp_dir = root.join("startup-tmp");
    let tmp_text = tmp_dir.to_string_lossy().into_owned();
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("create home");

    for case in &fixture.cases {
        let base = if case.base == "isolated" {
            env_base_path.clone()
        } else {
            root.join(&case.base)
        };
        let _ = std::fs::remove_dir_all(&base);
        materialize(&base, &case.files, &case.dirs);
        let _ = std::fs::remove_dir_all(tmp_dir.join("mode-startup"));

        let now_iso = Arc::new(|| "2026-01-01T00:00:00.000Z".to_string());
        let backup = Arc::new(|| "20260101-000000".to_string());
        let staging = Arc::new(|| "staging-test".to_string());
        let mtime_of = Arc::new(|path: &Path| {
            std::fs::metadata(path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| {
                    modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_millis() as i64)
                })
        });
        let initializer = DataRootInitializer::new(
            &base,
            Some(env_base_path.clone()),
            DataRootManifestCreatedBy::Desktop,
            "0.0.1",
            &tmp_dir,
            4242,
            &home,
            now_iso,
            backup,
            staging,
            1_700_000_000_000,
            mtime_of,
        );

        let result: Result<Value, String> = match case.kind.as_str() {
            "interactive" => initializer
                .initialize_interactive()
                .map(|result| serde_json::to_value(result).expect("序列化结果")),
            "noninteractive" => initializer
                .initialize_non_interactive(match case.action.as_deref() {
                    Some("migrate") => DataRootNonInteractiveAction::Migrate,
                    Some("fail") => DataRootNonInteractiveAction::Fail,
                    _ => DataRootNonInteractiveAction::Fresh,
                })
                .map(|result| serde_json::to_value(result).expect("序列化结果")),
            "import" => {
                let candidates = discover_legacy_data_root_candidates(&base, &home);
                let cancel = case.cancel;
                let is_cancelled = || cancel;
                let result: DataRootImportResult =
                    initializer.execute_import(&candidates, None, Some(&is_cancelled));
                Ok(serde_json::to_value(result).expect("序列化结果"))
            }
            other => panic!("未知 kind: {other}"),
        };
        let mut actual_result = match result {
            Ok(value) => value,
            Err(error) => serde_json::json!({ "error": error }),
        };
        normalize_value(&mut actual_result, &root_text, &env_base_text, &tmp_text);
        assert_eq!(actual_result, case.expected.result, "{}: result", case.name);

        let active = initializer.active_diagnostic_root().map(|path| {
            normalize_text(
                &path.to_string_lossy(),
                &root_text,
                &env_base_text,
                &tmp_text,
            )
        });
        assert_eq!(
            active, case.expected.active_diagnostic_root,
            "{}: activeDiagnosticRoot",
            case.name
        );

        let tree = tree_snapshot(&base, &root_text, &env_base_text, &tmp_text);
        assert_eq!(tree, case.expected.tree, "{}: tree", case.name);
        let _ = std::fs::remove_dir_all(tmp_dir.join("mode-startup"));
    }
}
