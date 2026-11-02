//! 与 TS `runtimeToolResolver.ts` 的对拍：
//! `pnpm runtime-tools:fixtures` 生成的 `fixtures/runtime-tools.json`。
//!
//! 夹具在 Windows/x64 上生成；POSIX 分支由 Rust 原生 `std::path` 保证，不在夹具口径内。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use runtime_env::EnvMap;
use std::sync::Mutex;
use std::time::Duration;

use runtime_tools::{
    build_login_shell_args, build_login_shell_execution_options, build_runtime_tool_env_patch,
    build_shell_bootstrap_path, capture_login_shell_env_snapshot, extract_captured_env_snapshot,
    find_mode_agent_runtime_binary, find_mode_agent_runtime_node_bundle,
    resolve_client_config_platform, resolve_shell_path_for_login_env, AgentRuntimeContext,
    CaptureLoginShellOptions, LoginShellExecutionOptions, LoginShellExecutor, LoginShellFuture,
    LoginShellPath, NativeToolFileSystem, RuntimeToolContext, ToolId,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    platform: String,
    arch: String,
    cases: Vec<Case>,
    bootstrap: Vec<BootstrapCase>,
    login: Vec<LoginShellCase>,
    agent_runtime: Vec<AgentRuntimeCase>,
    client_platform: Vec<ClientPlatformCase>,
    capture: Vec<CaptureCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureCase {
    name: String,
    base_env: BTreeMap<String, String>,
    platform: String,
    #[serde(default)]
    shell_path: Option<Value>,
    timeout_ms: u64,
    behavior: String,
    delay_ms: Option<u64>,
    output: String,
    expected: CaptureExpected,
}

#[derive(Debug, Deserialize)]
struct CaptureExpected {
    captured: Option<Value>,
    called: bool,
}

struct CaptureExecutor {
    behavior: String,
    delay_ms: u64,
    output: String,
    called: Mutex<bool>,
}

impl LoginShellExecutor for CaptureExecutor {
    fn execute(
        &self,
        _shell_path: &str,
        _args: &[String],
        _options: &LoginShellExecutionOptions,
    ) -> LoginShellFuture<'_> {
        *self.called.lock().expect("called lock") = true;
        let behavior = self.behavior.clone();
        let delay_ms = self.delay_ms;
        let output = self.output.clone();
        Box::pin(async move {
            match behavior.as_str() {
                "throw" => Err("executor failed".to_string()),
                "delay" => {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    Ok(output)
                }
                _ => Ok(output),
            }
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientPlatformCase {
    platform: String,
    arch: String,
    env: BTreeMap<String, String>,
    expected: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRuntimeCase {
    name: String,
    cwd: String,
    env: BTreeMap<String, String>,
    setup: Vec<String>,
    expected: AgentRuntimeExpected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRuntimeExpected {
    binary: Option<String>,
    node_bundle: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapCase {
    current_path: Option<String>,
    expected: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginShellCase {
    name: String,
    base_env: BTreeMap<String, String>,
    #[serde(default)]
    shell_path: Option<Value>,
    shell_file: Option<String>,
    timeout_ms: Option<u64>,
    output: String,
    expected: LoginShellExpected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginShellExpected {
    resolved_shell: Option<String>,
    args: Option<Vec<String>>,
    options: Option<LoginShellOptions>,
    captured: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginShellOptions {
    timeout: u64,
    max_buffer: usize,
    env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    cwd: String,
    tool_ids: Vec<String>,
    env: BTreeMap<String, String>,
    setup: Vec<String>,
    expected: Value,
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-runtime-tools-fixture-{}-{}",
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

fn normalize_value(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace(root, "<root>"),
        Value::Object(map) => {
            for child in map.values_mut() {
                normalize_value(child, root);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_value(item, root);
            }
        }
        _ => {}
    }
}

#[test]
fn matches_ts_runtime_tool_resolver() {
    if !cfg!(windows) {
        return;
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("runtime-tools.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));
    assert_eq!(fixture.platform, "win32");
    assert_eq!(fixture.arch, "x64");

    for case in &fixture.cases {
        let temp = TempDir::new();
        let root = temp.0.clone();
        for relative in &case.setup {
            let target = root.join(relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("create setup dir");
            }
            std::fs::write(&target, b"").expect("write setup file");
        }
        let cwd = root.join(&case.cwd);
        std::fs::create_dir_all(&cwd).expect("create cwd");
        let root_text = root.to_string_lossy().into_owned();
        let mut env = EnvMap::new();
        for (key, value) in &case.env {
            env.insert(key.clone(), value.replace("<root>", &root_text));
        }
        let tool_ids: Vec<ToolId> = case
            .tool_ids
            .iter()
            .map(|id| ToolId::parse(id).unwrap_or_else(|| panic!("未知工具 {id}")))
            .collect();
        let context = RuntimeToolContext {
            platform: fixture.platform.clone(),
            arch: fixture.arch.clone(),
            cwd,
            module_dir: None,
            resources_path: None,
            env,
            fs: &NativeToolFileSystem,
        };
        let patch = build_runtime_tool_env_patch(&tool_ids, &context);
        let mut actual = serde_json::to_value(patch).expect("序列化 patch");
        normalize_value(&mut actual, &root_text);
        assert_eq!(actual, case.expected, "{}: 工具环境补丁不一致", case.name);
    }

    for case in &fixture.bootstrap {
        let actual = build_shell_bootstrap_path(&fixture.platform, case.current_path.as_deref());
        assert_eq!(
            actual, case.expected,
            "bootstrap currentPath={:?}",
            case.current_path
        );
    }

    for case in &fixture.login {
        let temp = TempDir::new();
        let root = temp.0.clone();
        if let Some(relative) = &case.shell_file {
            let target = root.join(relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("create shell dir");
            }
            std::fs::write(&target, b"").expect("write shell file");
        }
        let root_text = root.to_string_lossy().into_owned();
        let mut env = EnvMap::new();
        for (key, value) in &case.base_env {
            env.insert(key.clone(), value.replace("<root>", &root_text));
        }
        let resolved = match &case.shell_path {
            Some(Value::Null) => None,
            Some(Value::String(shell)) => Some(shell.clone()),
            Some(other) => panic!("shellPath 类型非法: {other}"),
            None => resolve_shell_path_for_login_env(&env, &NativeToolFileSystem),
        };
        let normalized_shell = resolved
            .as_deref()
            .map(|shell| shell.replace(&root_text, "<root>"));
        assert_eq!(
            normalized_shell, case.expected.resolved_shell,
            "{}: resolved shell",
            case.name
        );
        match resolved {
            None => {
                assert!(
                    case.expected.args.is_none(),
                    "{}: 无 shell 时不应有 args",
                    case.name
                );
                assert!(
                    case.expected.options.is_none(),
                    "{}: 无 shell 时不应有 options",
                    case.name
                );
                assert!(
                    case.expected.captured.is_none(),
                    "{}: 无 shell 时不应有 captured",
                    case.name
                );
            }
            Some(shell) => {
                assert_eq!(
                    Some(build_login_shell_args(&shell)),
                    case.expected.args,
                    "{}: args",
                    case.name
                );
                let options = build_login_shell_execution_options(
                    &fixture.platform,
                    &env,
                    case.timeout_ms.unwrap_or(4_000),
                );
                let expected_options = case.expected.options.as_ref().expect("options");
                assert_eq!(
                    options.timeout_ms, expected_options.timeout,
                    "{}: timeout",
                    case.name
                );
                assert_eq!(
                    options.max_buffer_bytes, expected_options.max_buffer,
                    "{}: maxBuffer",
                    case.name
                );
                let mut actual_env = serde_json::to_value(options.env).expect("序列化 env");
                normalize_value(&mut actual_env, &root_text);
                let expected_env = serde_json::to_value(&expected_options.env).expect("序列化 env");
                assert_eq!(actual_env, expected_env, "{}: options env", case.name);
                let mut actual_captured = extract_captured_env_snapshot(&case.output)
                    .map(|snapshot| serde_json::to_value(snapshot).expect("序列化 snapshot"))
                    .unwrap_or(Value::Null);
                normalize_value(&mut actual_captured, &root_text);
                let expected_captured = case.expected.captured.clone().unwrap_or(Value::Null);
                assert_eq!(
                    actual_captured, expected_captured,
                    "{}: captured",
                    case.name
                );
            }
        }
    }

    for case in &fixture.agent_runtime {
        let temp = TempDir::new();
        let root = temp.0.clone();
        for relative in &case.setup {
            let target = root.join(relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).expect("create agent setup dir");
            }
            std::fs::write(&target, b"").expect("write agent setup file");
        }
        let cwd = root.join(&case.cwd);
        std::fs::create_dir_all(&cwd).expect("create agent cwd");
        let root_text = root.to_string_lossy().into_owned();
        let mut env = EnvMap::new();
        for (key, value) in &case.env {
            env.insert(key.clone(), value.replace("<root>", &root_text));
        }
        let context = AgentRuntimeContext {
            platform: fixture.platform.clone(),
            arch: fixture.arch.clone(),
            cwd,
            module_dir: None,
            resources_path: None,
            home_dir: root.join("home"),
            env,
            fs: &NativeToolFileSystem,
        };
        let binary =
            find_mode_agent_runtime_binary(&context).map(|path| path.replace(&root_text, "<root>"));
        let node_bundle = find_mode_agent_runtime_node_bundle(&context)
            .map(|path| path.replace(&root_text, "<root>"));
        assert_eq!(binary, case.expected.binary, "{}: binary", case.name);
        assert_eq!(
            node_bundle, case.expected.node_bundle,
            "{}: nodeBundle",
            case.name
        );
    }

    for (index, case) in fixture.client_platform.iter().enumerate() {
        let env: EnvMap = case
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let actual = resolve_client_config_platform(&env, &case.platform, &case.arch);
        assert_eq!(actual, case.expected, "clientPlatform[{index}]");
    }
}

#[tokio::test]
async fn matches_ts_login_shell_capture() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("runtime-tools.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.capture {
        let env: EnvMap = case
            .base_env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let shell_path = match &case.shell_path {
            None => LoginShellPath::Resolve,
            Some(Value::Null) => LoginShellPath::None,
            Some(Value::String(shell)) => LoginShellPath::Explicit(shell.clone()),
            Some(other) => panic!("{}: shellPath 类型非法: {other}", case.name),
        };
        let executor = CaptureExecutor {
            behavior: case.behavior.clone(),
            delay_ms: case.delay_ms.unwrap_or(50),
            output: case.output.clone(),
            called: Mutex::new(false),
        };
        let captured = capture_login_shell_env_snapshot(&CaptureLoginShellOptions {
            base_env: &env,
            platform: &case.platform,
            process_platform: "win32",
            shell_path,
            timeout_ms: case.timeout_ms,
            executor: &executor,
        })
        .await;
        let actual = captured
            .map(|snapshot| serde_json::to_value(snapshot).expect("序列化 snapshot"))
            .unwrap_or(Value::Null);
        let expected = case.expected.captured.clone().unwrap_or(Value::Null);
        assert_eq!(actual, expected, "{}: captured", case.name);
        assert_eq!(
            *executor.called.lock().expect("called lock"),
            case.expected.called,
            "{}: executor called",
            case.name
        );
    }
}
