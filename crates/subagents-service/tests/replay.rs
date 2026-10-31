//! fixture 回放对拍（`docs/specs/subagents-service.md`）。
//!
//! `pnpm subagents:fixtures` 用 TS 真源在临时沙盒里执行七个用例，记录八个方法的结果
//! 与服务结束后的整棵树；本测试在 Rust 侧重放同一批文件与操作序列并逐项深比较。
//! 字符串按生成脚本规则归一：沙盒根替换为 `<root>`、Windows 路径折叠为 `/`；
//! `agents-state.json` 在对拍前按键排序（两端的 JSON 键序实现不同）。

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use subagents_service::{
    AgentConfig, CreateAgentParams, DeleteAgentParams, ListMode, ListParams, Selection,
    SubagentsService, SubagentsServiceOptions, UpdateAgentParams,
};

const FIXTURE: &str = include_str!("../fixtures/subagents.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    name: String,
    desktop: bool,
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
    mode: Option<String>,
    agent_id: Option<String>,
    resolve_name: Option<String>,
    enabled: Option<bool>,
    agent_name: Option<String>,
    model_selection: Option<Selection>,
    config: Option<AgentConfig>,
    scope: Option<String>,
    use_workspace: Option<bool>,
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
            "zc-subagents-replay-{tag}-{}-{nanos}",
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
            let mut out = serde_json::Map::new();
            for (key, child) in map {
                out.insert(key.clone(), normalize_value(roots, child));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn normalize_state_file(roots: &Roots, content: &str) -> String {
    let parsed: Value = serde_json::from_str(content).expect("state file is JSON");
    roots.normalize_text(&serde_json::to_string_pretty(&parsed).expect("state file serializes"))
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
    service: SubagentsService,
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
            std::fs::write(&target, file.content.replace("<root>", &self.roots.forward))
                .expect("materialize file");
        }
    }

    fn list_params(&self, mode: Option<&str>) -> ListParams {
        ListParams {
            workspace_path: self.workspace(),
            workspace_identity: None,
            mode: match mode {
                Some("settingsUserOnly") => Some(ListMode::SettingsUserOnly),
                _ => None,
            },
        }
    }

    fn resolve_agent(&self, name: &str) -> Result<(String, String), String> {
        let listed = self.service.list(&self.list_params(None));
        let agents = listed.agents;
        let found = agents
            .iter()
            .find(|agent| agent.get("name").and_then(Value::as_str) == Some(name));
        match found {
            Some(agent) => Ok((
                agent
                    .get("id")
                    .and_then(Value::as_str)
                    .expect("agent id")
                    .to_string(),
                agent
                    .get("path")
                    .and_then(Value::as_str)
                    .expect("agent path")
                    .to_string(),
            )),
            None => Err(format!("agent not found by name: {name}")),
        }
    }

    fn run_op(&self, op: &Op) -> Result<Value, String> {
        match op.kind.as_str() {
            "list" => Ok(serde_json::to_value(
                self.service.list(&self.list_params(op.mode.as_deref())),
            )
            .expect("list serializes")),
            "setEnabled" => {
                let agent_id = match &op.agent_id {
                    Some(id) => id.clone(),
                    None => {
                        self.resolve_agent(op.resolve_name.as_deref().unwrap_or(""))?
                            .0
                    }
                };
                self.service
                    .set_enabled(&agent_id, op.enabled.unwrap_or(false))
                    .map_err(|error| error.message)?;
                Ok(json!({ "ok": true }))
            }
            "setBuiltInModelOverride" => {
                self.service
                    .set_built_in_model_override(
                        op.agent_name.as_deref().unwrap_or("general-purpose"),
                        op.model_selection.clone(),
                    )
                    .map_err(|error| error.message)?;
                Ok(json!({ "ok": true }))
            }
            "setPluginAgentModelOverride" => {
                self.service
                    .set_plugin_agent_model_override(
                        op.agent_id.as_deref().unwrap_or(""),
                        op.model_selection.clone(),
                    )
                    .map_err(|error| error.message)?;
                Ok(json!({ "ok": true }))
            }
            "getPrimaryUserAgentsDirectory" => {
                let path = self
                    .service
                    .get_primary_user_agents_directory()
                    .map_err(|error| error.message)?;
                Ok(json!({ "path": path.to_string_lossy() }))
            }
            "createAgent" => {
                let agent = self
                    .service
                    .create_agent(&CreateAgentParams {
                        config: op.config.clone().unwrap_or_default(),
                        scope: op.scope.clone(),
                        workspace_path: op
                            .use_workspace
                            .unwrap_or(false)
                            .then(|| self.workspace().to_string_lossy().into_owned()),
                    })
                    .map_err(|error| error.message)?;
                Ok(agent.to_json())
            }
            "updateAgent" => {
                let agent_id = match &op.agent_id {
                    Some(id) => id.clone(),
                    None => {
                        self.resolve_agent(op.resolve_name.as_deref().unwrap_or(""))?
                            .0
                    }
                };
                let old_file_path = match &op.agent_id {
                    Some(_) => None,
                    None => Some(
                        self.resolve_agent(op.resolve_name.as_deref().unwrap_or(""))?
                            .1,
                    ),
                };
                let agent = self
                    .service
                    .update_agent(&UpdateAgentParams {
                        agent_id,
                        config: op.config.clone().unwrap_or_default(),
                        old_file_path,
                        scope: op.scope.clone(),
                        workspace_path: op
                            .use_workspace
                            .unwrap_or(false)
                            .then(|| self.workspace().to_string_lossy().into_owned()),
                    })
                    .map_err(|error| error.message)?;
                Ok(agent.to_json())
            }
            "deleteAgent" => {
                let (agent_id, file_path) = match &op.agent_id {
                    Some(id) => (id.clone(), String::new()),
                    None => self.resolve_agent(op.resolve_name.as_deref().unwrap_or(""))?,
                };
                self.service
                    .delete_agent(&DeleteAgentParams {
                        agent_id,
                        file_path,
                    })
                    .map_err(|error| error.message)?;
                Ok(json!({ "ok": true }))
            }
            "prepareRuntimeState" => {
                self.service.prepare_runtime_state();
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
                let content = if name.ends_with("agents-state.json") {
                    normalize_state_file(&self.roots, &text)
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
        let service = SubagentsService::new(SubagentsServiceOptions::with_home(
            roots.native.join("home"),
            case.desktop,
        ));
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
