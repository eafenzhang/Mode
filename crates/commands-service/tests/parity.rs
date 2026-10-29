//! 与 TS 的黄金数据对拍：markdown 解析/生成字节、文件名、list 顺序/字段、
//! enable override 落盘结果与插件命令。
//!
//! fixture 由 `pnpm commands:fixtures` 用真实 TS 实现生成，Rust 侧只能设法通过。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use commands_service::{
    command_file_name, generate_command_file_content, get_command_source_descriptor,
    parse_command_file, CommandAgentSource, CommandConfig, CommandDeleteParams,
    CommandSetEnabledParams, CommandsService, CommandsServiceOptions, ListParams,
};
use serde_json::{json, Map, Value};

const FIXTURE: &str = include_str!("../fixtures/commands.json");
const ROOT_TOKEN: &str = "<ROOT>";

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("fixture 必须是合法 JSON")
}

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-commands-parity-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 按 `/` 分段拼接，和 TS 的 `path.join(root, ...segments)` 同形。
fn p(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|segment| !segment.is_empty())
        .fold(base.to_path_buf(), |path, segment| path.join(segment))
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// 配置/JSON 里的 `<ROOT>` 是整段路径模板：替换后用本平台分隔符重建。
fn materialize_string(value: &str, root: &Path) -> String {
    let Some(index) = value.find(ROOT_TOKEN) else {
        return value.to_string();
    };
    let suffix = value[index + ROOT_TOKEN.len()..].trim_start_matches(['/', '\\']);
    let absolute = suffix
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .fold(root.to_path_buf(), |path, segment| path.join(segment));
    format!("{}{}", &value[..index], absolute.to_string_lossy())
}

fn materialize(value: &Value, root: &Path) -> Value {
    match value {
        Value::String(text) => Value::String(materialize_string(text, root)),
        Value::Array(items) => Value::Array(items.iter().map(|item| materialize(item, root)).collect()),
        Value::Object(object) => {
            let mut result = Map::new();
            for (key, item) in object {
                result.insert(materialize_string(key, root), materialize(item, root));
            }
            Value::Object(result)
        }
        other => other.clone(),
    }
}

/// 结果里的绝对路径归一成 `<ROOT>` + `/`，与 fixture 的生成口径一致。
fn normalize(value: &Value, root: &Path) -> Value {
    let root_text = root.to_string_lossy();
    match value {
        Value::String(text) => Value::String(
            text.replace(root_text.as_ref(), ROOT_TOKEN)
                .replace('\\', "/"),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|item| normalize(item, root)).collect()),
        Value::Object(object) => {
            let mut result = Map::new();
            for (key, item) in object {
                let key = key.replace(root_text.as_ref(), ROOT_TOKEN).replace('\\', "/");
                result.insert(key, normalize(item, root));
            }
            Value::Object(result)
        }
        other => other.clone(),
    }
}

fn config_from(value: &Value) -> CommandConfig {
    serde_json::from_value(value.clone()).expect("config 必须是合法 CommandConfig")
}

#[test]
fn markdown_parse_matches_ts() {
    let fixture = fixture();
    for case in fixture["parser"]["cases"].as_array().unwrap() {
        let content = case["content"].as_str().unwrap();
        let file_path = case["filePath"].as_str().unwrap();
        let parsed = parse_command_file(content, file_path);

        let mut actual = Map::new();
        actual.insert("name".into(), Value::String(parsed.name));
        actual.insert("prompt".into(), Value::String(parsed.prompt));
        actual.insert("content".into(), Value::String(parsed.content));
        actual.insert("filePath".into(), Value::String(parsed.file_path));
        if let Some(description) = parsed.description {
            actual.insert("description".into(), Value::String(description));
        }
        if let Some(argument_hint) = parsed.argument_hint {
            actual.insert("argumentHint".into(), Value::String(argument_hint));
        }

        assert_eq!(
            Value::Object(actual),
            case["expected"],
            "parse case failed for {file_path}"
        );
    }
}

#[test]
fn markdown_generate_matches_ts() {
    let fixture = fixture();
    for case in fixture["generator"]["cases"].as_array().unwrap() {
        let config = config_from(&case["config"]);
        let existing = case["existing"].as_str();
        let actual = generate_command_file_content(&config, existing);
        assert_eq!(
            actual,
            case["expected"].as_str().unwrap(),
            "generate case failed for {config:?}"
        );
    }
}

#[test]
fn command_file_name_matches_ts() {
    let fixture = fixture();
    let descriptor = get_command_source_descriptor(CommandAgentSource::ModeAgent);
    for case in fixture["fileNames"]["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let config = CommandConfig {
            name: name.to_string(),
            prompt: String::new(),
            description: None,
            argument_hint: None,
            file_path: None,
        };
        assert_eq!(
            command_file_name(&config, descriptor),
            case["expected"].as_str().unwrap(),
            "file name case failed for {name}"
        );
    }
}

fn prepare_scenario(root: &TempRoot, scenario: &Value) -> PathBuf {
    let home = p(root.path(), "home");
    fs::create_dir_all(&home).unwrap();

    for dir in scenario["dirs"].as_array().unwrap() {
        fs::create_dir_all(p(root.path(), dir.as_str().unwrap())).unwrap();
    }
    for file in scenario["files"].as_array().unwrap() {
        let [relative, content] = [file[0].as_str().unwrap(), file[1].as_str().unwrap()];
        write(&p(root.path(), relative), content);
    }
    for file in scenario
        .get("jsonFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let relative = file[0].as_str().unwrap();
        let materialized = materialize(&file[1], root.path());
        write(
            &p(root.path(), relative),
            &serde_json::to_string(&materialized).unwrap(),
        );
    }
    if let Some(config) = scenario.get("config").filter(|value| !value.is_null()) {
        let materialized = materialize(config, root.path());
        write(
            &home.join(".mode").join("cli").join("config.json"),
            &format!(
                "{}\n",
                serde_json::to_string_pretty(&materialized).unwrap()
            ),
        );
    }
    home
}

#[test]
fn list_matches_ts() {
    let fixture = fixture();
    for scenario in fixture["list"]["scenarios"].as_array().unwrap() {
        let id = scenario["id"].as_str().unwrap();
        let root = TempRoot::new(id);
        let home = prepare_scenario(&root, scenario);

        let list_params = &scenario["listParams"];
        let params = ListParams {
            agent_source: list_params
                .get("agentSource")
                .and_then(Value::as_str)
                .map(|_| CommandAgentSource::ModeAgent),
            workspace_path: list_params
                .get("workspacePath")
                .and_then(Value::as_str)
                .map(|relative| p(root.path(), relative).to_string_lossy().into_owned()),
            workspace_identity: None,
        };

        let mut options = CommandsServiceOptions::with_home(&home);
        options.write_options.use_file_lock = false;
        let service = CommandsService::new(options);
        let result = service.list(&params).unwrap();
        let actual = normalize(&serde_json::to_value(&result).unwrap(), root.path());

        assert_eq!(actual, scenario["expected"], "list scenario failed: {id}");
    }
}

#[test]
fn enable_override_matches_ts() {
    let fixture = fixture();
    for (index, case) in fixture["override"]["cases"].as_array().unwrap().iter().enumerate() {
        let id = case["id"].as_str().unwrap();
        let root = TempRoot::new(&format!("override-{index}"));
        let home = p(root.path(), "home");
        fs::create_dir_all(&home).unwrap();

        if let Some(initial) = case.get("initial").filter(|value| !value.is_null()) {
            let materialized = materialize(initial, root.path());
            write(
                &home.join(".mode").join("cli").join("config.json"),
                &format!(
                    "{}\n",
                    serde_json::to_string_pretty(&materialized).unwrap()
                ),
            );
        }

        let mut options = CommandsServiceOptions::with_home(&home);
        options.write_options.use_file_lock = false;
        let service = CommandsService::new(options);

        for call in case["calls"].as_array().unwrap() {
            service
                .set_command_enabled(&CommandSetEnabledParams {
                    command_id: None,
                    file_path: materialize_string(call["filePath"].as_str().unwrap(), root.path()),
                    enabled: call["enabled"].as_bool().unwrap(),
                    agent_source: None,
                })
                .unwrap();
        }

        let persisted: Value = serde_json::from_str(
            &fs::read_to_string(home.join(".mode").join("cli").join("config.json")).unwrap(),
        )
        .unwrap();
        let actual = normalize(&persisted, root.path());
        assert_eq!(actual, case["expected"], "override case failed: {id}");
    }
}

/// 删除不存在的文件是成功，并清掉该路径的 override（与 TS 一致）。
#[test]
fn delete_clears_override_without_a_file() {
    let root = TempRoot::new("delete-override");
    let home = p(root.path(), "home");
    fs::create_dir_all(&home).unwrap();
    let file = p(&home, ".mode/commands/gone.md");
    let file_path = file.to_string_lossy().into_owned();
    let mut command = Map::new();
    command.insert(file_path.clone(), json!({ "enable": false }));
    let mut config = Map::new();
    config.insert("command".to_string(), Value::Object(command));
    write(
        &home.join(".mode").join("cli").join("config.json"),
        &format!("{}\n", Value::Object(config)),
    );

    let mut options = CommandsServiceOptions::with_home(&home);
    options.write_options.use_file_lock = false;
    let service = CommandsService::new(options);
    service
        .delete_command_file(&CommandDeleteParams {
            command_id: None,
            file_path,
            agent_source: None,
        })
        .unwrap();

    let persisted: Value = serde_json::from_str(
        &fs::read_to_string(home.join(".mode").join("cli").join("config.json")).unwrap(),
    )
    .unwrap();
    assert!(persisted.get("command").is_none());
}

/// fixture 自检：所有路径类断言都不该残留本机绝对根。
#[test]
fn fixture_has_no_absolute_paths() {
    let fixture = fixture();
    let text = serde_json::to_string(&fixture).unwrap();
    assert!(!text.contains(&std::env::temp_dir().to_string_lossy().to_string()));
}
