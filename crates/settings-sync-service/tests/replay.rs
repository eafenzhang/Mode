//! fixture 回放对拍（`docs/specs/settings-sync-service.md`）。
//!
//! `pnpm settings-sync:fixtures` 用 TS 真源执行六个用例，记录 detect /
//! importSelected / Claude AGENTS.md 迁移 / 首启状态的结果与结束树；
//! 本测试在 Rust 侧重放并逐项深比较。字符串按生成脚本规则归一：沙盒根替换为
//! `<root>`、Windows 路径折叠为 `/`、JSON 文件按键排序后比较。

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use settings_sync_service::{
    DetectParams, FirstRunPromptState, ImportParams, Selection, SettingsBridge,
    SettingsSyncService, SettingsSyncServiceOptions,
};

const FIXTURE: &str = include_str!("../fixtures/settings-sync.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    name: String,
    files: Vec<FixtureFile>,
    ops: Vec<Op>,
    expected: Vec<Value>,
    tree: Vec<FixtureFile>,
}

#[derive(Deserialize)]
struct FixtureFile {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Op {
    kind: String,
    categories: Option<Vec<String>>,
    intent: Option<String>,
    use_workspace: Option<bool>,
    selections: Option<Vec<Selection>>,
    overwrite: Option<bool>,
}

#[derive(Default)]
struct MemorySettings {
    settings: Mutex<Map<String, Value>>,
}

impl SettingsBridge for MemorySettings {
    fn get_settings(&self) -> Result<Value, String> {
        Ok(Value::Object(self.settings.lock().unwrap().clone()))
    }

    fn update_settings(&self, patch: &Value) -> Result<(), String> {
        let mut settings = self.settings.lock().unwrap();
        if let Value::Object(patch) = patch {
            for (key, value) in patch {
                settings.insert(key.clone(), value.clone());
            }
        }
        Ok(())
    }
}

struct Roots {
    native: PathBuf,
    native_text: String,
    forward: String,
    escaped: String,
}

impl Roots {
    fn create(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let base = std::env::temp_dir().join(format!(
            "zc-settings-sync-replay-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create sandbox");
        let native = canonical_long_path(&base);
        let native_text = native.to_string_lossy().into_owned();
        let forward = native_text.replace('\\', "/");
        let escaped = native_text.replace('\\', "\\\\");
        Self {
            native,
            native_text,
            forward,
            escaped,
        }
    }

    fn normalize_text(&self, text: &str) -> String {
        let mut replaced = text.replace(&self.escaped, "<root>");
        replaced = replaced.replace(&self.native_text, "<root>");
        replaced = replaced.replace(&self.forward, "<root>");
        if cfg!(windows) {
            replaced = replaced.replace("\\\\", "/");
            replaced = replaced.replace('\\', "/");
        }
        replaced
    }

    fn resolve_placeholder(&self, value: &str) -> PathBuf {
        match value.strip_prefix("<root>/") {
            Some(rest) => rest
                .split('/')
                .filter(|segment| !segment.is_empty())
                .fold(self.native.clone(), |path, segment| path.join(segment)),
            None => PathBuf::from(value),
        }
    }
}

fn canonical_long_path(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    canonical
}

fn normalize_value(roots: &Roots, value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(roots.normalize_text(text)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_value(roots, item))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, child) in map {
                out.insert(key.clone(), normalize_value(roots, child));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn normalize_json_file(roots: &Roots, content: &str) -> String {
    let parsed: Value = serde_json::from_str(content).expect("json file parses");
    roots.normalize_text(&serde_json::to_string_pretty(&parsed).expect("json file serializes"))
}

fn locale_compare(left: &str, right: &str) -> Ordering {
    let case_rank = |text: &str| -> Vec<u8> {
        text.chars()
            .map(|value| u8::from(value.is_uppercase()))
            .collect()
    };
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| case_rank(left).cmp(&case_rank(right)))
}

struct Replay<'a> {
    roots: Roots,
    case: &'a FixtureCase,
    service: SettingsSyncService,
}

impl<'a> Replay<'a> {
    fn workspace(&self) -> PathBuf {
        self.roots.native.join("ws")
    }

    fn materialize(&self) {
        for file in &self.case.files {
            let target = self.roots.native.join(&file.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("materialize parent dirs");
            }
            std::fs::write(&target, file.content.clone()).expect("materialize file");
        }
    }

    fn resolve_selection(&self, selection: &Selection) -> Selection {
        let map_paths = |paths: &Option<Vec<String>>| {
            paths.as_ref().map(|paths| {
                paths
                    .iter()
                    .map(|path| {
                        self.roots
                            .resolve_placeholder(path)
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect()
            })
        };
        Selection {
            agent: selection.agent.clone(),
            category: selection.category.clone(),
            source_scope: selection.source_scope.clone(),
            target_scope: selection.target_scope.clone(),
            import_mode: selection.import_mode.clone(),
            skill_paths: map_paths(&selection.skill_paths),
            command_paths: map_paths(&selection.command_paths),
            plugin_paths: map_paths(&selection.plugin_paths),
            mcp_server_paths: map_paths(&selection.mcp_server_paths),
        }
    }

    fn run_op(&self, op: &Op) -> Result<Value, String> {
        match op.kind.as_str() {
            "detect" => {
                let result = self.service.detect(&DetectParams {
                    workspace_path: op
                        .use_workspace
                        .unwrap_or(false)
                        .then(|| self.workspace().to_string_lossy().into_owned()),
                    workspace_identity: None,
                    categories: op.categories.clone(),
                    intent: op.intent.clone(),
                });
                Ok(serde_json::to_value(&result).expect("discovery serializes"))
            }
            "importSelected" => {
                let selections = op
                    .selections
                    .clone()
                    .unwrap_or_default()
                    .iter()
                    .map(|selection| self.resolve_selection(selection))
                    .collect();
                let result = self.service.import_selected(&ImportParams {
                    workspace_path: op
                        .use_workspace
                        .unwrap_or(false)
                        .then(|| self.workspace().to_string_lossy().into_owned()),
                    workspace_identity: None,
                    selections,
                });
                Ok(serde_json::to_value(&result).expect("import serializes"))
            }
            "agentsFileStatus" => Ok(serde_json::to_value(
                self.service.get_claude_agents_file_migration_status(),
            )
            .expect("status serializes")),
            "copyAgentsFile" => Ok(serde_json::to_value(
                self.service
                    .copy_claude_agents_file_to_mode_agents_file(op.overwrite.unwrap_or(false)),
            )
            .expect("copy serializes")),
            "firstRunState" => {
                let state: FirstRunPromptState = self.service.get_first_run_prompt_state()?;
                Ok(serde_json::to_value(&state).expect("state serializes"))
            }
            "markFirstRun" => {
                self.service.mark_first_run_prompt_handled()?;
                Ok(json!({ "ok": true }))
            }
            other => panic!("fixture 只包含已知操作，遇到 {other}"),
        }
    }

    fn capture_tree(&self) -> Vec<(String, String)> {
        let mut files = Vec::new();
        self.walk_tree(&self.roots.native, "", &mut files);
        files.sort_by(|left, right| locale_compare(&left.0, &right.0));
        files
    }

    fn walk_tree(&self, directory: &Path, relative: &str, files: &mut Vec<(String, String)>) {
        let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(directory)
            .expect("read tree directory")
            .flatten()
            .collect();
        entries.sort_by(|left, right| {
            locale_compare(
                &left.file_name().to_string_lossy(),
                &right.file_name().to_string_lossy(),
            )
        });
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_relative = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let file_type = entry.file_type().expect("entry file type");
            if file_type.is_dir() {
                self.walk_tree(&entry.path(), &child_relative, files);
            } else if file_type.is_file() {
                let text =
                    String::from_utf8_lossy(&std::fs::read(entry.path()).expect("read tree file"))
                        .into_owned();
                let content = if name.ends_with(".json") {
                    normalize_json_file(&self.roots, &text)
                } else {
                    self.roots.normalize_text(&text)
                };
                files.push((child_relative, content));
            }
        }
    }

    fn run(&self) -> Vec<String> {
        let mut failures = Vec::new();
        self.materialize();

        if self.case.expected.len() != self.case.ops.len() {
            failures.push(format!(
                "expected 数量 {} 与 ops 数量 {} 不一致",
                self.case.expected.len(),
                self.case.ops.len()
            ));
            return failures;
        }
        for (index, op) in self.case.ops.iter().enumerate() {
            let result = match self.run_op(op) {
                Ok(value) => value,
                Err(message) => json!({ "error": message }),
            };
            let actual = normalize_value(&self.roots, &result);
            let expected = &self.case.expected[index];
            if actual != *expected {
                failures.push(format!(
                    "op[{index}] {} 结果不一致\n--- expected ---\n{}\n--- actual ---\n{}",
                    op.kind,
                    serde_json::to_string_pretty(expected).unwrap_or_default(),
                    serde_json::to_string_pretty(&actual).unwrap_or_default(),
                ));
            }
        }

        let tree = self.capture_tree();
        if tree.len() != self.case.tree.len() {
            let expected_paths: Vec<&str> = self
                .case
                .tree
                .iter()
                .map(|file| file.path.as_str())
                .collect();
            let actual_paths: Vec<&str> = tree.iter().map(|(path, _)| path.as_str()).collect();
            failures.push(format!(
                "结束树文件数不一致：expected {:?}\nactual {:?}",
                expected_paths, actual_paths
            ));
            return failures;
        }
        for (index, (path, content)) in tree.iter().enumerate() {
            let expected = &self.case.tree[index];
            if path != &expected.path {
                failures.push(format!(
                    "结束树第 {index} 项路径不一致：expected {:?} actual {:?}",
                    expected.path, path
                ));
                continue;
            }
            if content != &expected.content {
                failures.push(format!(
                    "结束树 {path} 内容不一致\n--- expected ---\n{}\n--- actual ---\n{}",
                    expected.content, content
                ));
            }
        }
        failures
    }
}

#[test]
fn replay_ts_fixtures() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
    let mut all_failures = Vec::new();
    for case in &fixture.cases {
        let roots = Roots::create(&case.name);
        let service = SettingsSyncService::new(
            SettingsSyncServiceOptions {
                home_dir: Some(roots.native.join("home")),
            },
            Arc::new(MemorySettings::default()),
        );
        let replay = Replay {
            roots,
            case,
            service,
        };
        for failure in replay.run() {
            all_failures.push(format!("[case {}] {failure}", case.name));
        }
        let _ = std::fs::remove_dir_all(&replay.roots.native);
    }
    assert!(
        all_failures.is_empty(),
        "fixture 回放存在差异：\n{}",
        all_failures.join("\n\n")
    );
}
