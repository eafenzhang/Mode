//! 与 TS `runtimeEnv.ts` 的对拍：`pnpm runtime-env:fixtures` 生成的夹具。

use std::path::Path;

use runtime_env::{
    build_agent_endpoint_origin_env, build_agent_runtime_env, build_agent_workspace_identity_env,
    build_mode_tool_env_passthrough_env, is_cua_dev_mode_requested,
    is_effective_development_node_env, is_mode_cua_internal_feature_enabled,
    normalize_mode_runtime_env, read_mode_tool_env_passthrough_env, resolve_mode_runtime_env,
    should_capture_mode_tool_env_passthrough_key, should_sanitize_mode_runtime_env_key,
    AgentRuntimeEnvInput, EnvMap, ModeRuntimeEnv, RuntimeEnvContext,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    normalized: Vec<NormalizedCase>,
    resolved: Vec<ResolvedCase>,
    cua: Vec<CuaCase>,
    sanitize: Vec<SanitizeCase>,
    sequences: Vec<Sequence>,
    keys: Vec<KeyCase>,
    read_passthrough: Vec<PassthroughReadCase>,
    build_passthrough: Vec<PassthroughBuildCase>,
    agent_runtime: Vec<AgentRuntimeCase>,
    agent_endpoint: Vec<StringValueCase>,
    agent_workspace: Vec<StringValueCase>,
    node_env: Vec<NodeEnvCase>,
}

#[derive(Debug, Deserialize)]
struct NodeEnvCase {
    env: std::collections::BTreeMap<String, String>,
    expected: bool,
}

#[derive(Debug, Deserialize)]
struct NormalizedCase {
    value: Option<String>,
    expected: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResolvedCase {
    env: Value,
    fallback: Option<String>,
    expected: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CuaCase {
    env: Value,
    dev_mode: bool,
    internal_enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SanitizeCase {
    name: String,
    env: Value,
    expected: Value,
    in_place: Value,
    captured: Value,
}

#[derive(Debug, Deserialize)]
struct Sequence {
    name: String,
    steps: Vec<SequenceStep>,
}

#[derive(Debug, Deserialize)]
struct SequenceStep {
    env: Value,
    expected: Value,
    captured: Value,
}

#[derive(Debug, Deserialize)]
struct KeyCase {
    key: String,
    sanitize: bool,
    capture: bool,
}

#[derive(Debug, Deserialize)]
struct PassthroughReadCase {
    name: String,
    env: Value,
    expected: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRuntimeCase {
    input: AgentInput,
    expected: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentInput {
    http_proxy: Option<String>,
    no_proxy: Option<String>,
    ca_cert_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StringValueCase {
    input: Option<String>,
    expected: Value,
}

#[derive(Debug, Deserialize)]
struct PassthroughBuildCase {
    name: String,
    env: Value,
    expected: Value,
}

fn env_map(value: &Value) -> EnvMap {
    value
        .as_object()
        .unwrap_or_else(|| panic!("env 必须是对象: {value}"))
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_string())))
        .collect()
}

fn passthrough_payload(value: &Value) -> Value {
    let Some(object) = value.as_object() else {
        return Value::Null;
    };
    if object.is_empty() {
        return json!({});
    }
    let raw = object
        .get(runtime_env::MODE_TOOL_ENV_PASSTHROUGH_ENV_KEY)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("缺少 passthrough 键: {value}"));
    serde_json::from_str(raw).unwrap_or_else(|error| panic!("passthrough 不是 JSON: {error}"))
}

#[test]
fn matches_ts_runtime_env() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("runtime-env.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for (index, case) in fixture.normalized.iter().enumerate() {
        let actual = normalize_mode_runtime_env(case.value.as_deref()).map(ModeRuntimeEnv::as_str);
        assert_eq!(actual, case.expected.as_deref(), "normalized[{index}]");
    }
    for (index, case) in fixture.resolved.iter().enumerate() {
        let env = env_map(&case.env);
        let fallback = match &case.fallback {
            Some(value) => normalize_mode_runtime_env(Some(value)).expect("合法 fallback"),
            None => ModeRuntimeEnv::Production,
        };
        let actual = resolve_mode_runtime_env(&env, fallback).as_str();
        assert_eq!(actual, case.expected, "resolved[{index}]");
    }
    for (index, case) in fixture.cua.iter().enumerate() {
        let env = env_map(&case.env);
        assert_eq!(
            is_cua_dev_mode_requested(&env),
            case.dev_mode,
            "cua[{index}].devMode"
        );
        assert_eq!(
            is_mode_cua_internal_feature_enabled(&env),
            case.internal_enabled,
            "cua[{index}].internalEnabled"
        );
    }
    for case in &fixture.sanitize {
        let context = RuntimeEnvContext::new();
        let env = env_map(&case.env);
        let sanitized = context.sanitize(&env);
        assert_eq!(
            serde_json::to_value(sanitized).expect("序列化"),
            case.expected,
            "{}: sanitize",
            case.name
        );
        let mut in_place = env.clone();
        context.sanitize_in_place(&mut in_place);
        assert_eq!(
            serde_json::to_value(in_place).expect("序列化"),
            case.in_place,
            "{}: sanitizeInPlace",
            case.name
        );
        assert_eq!(
            serde_json::to_value(context.captured_credentials()).expect("序列化"),
            case.captured,
            "{}: captured",
            case.name
        );
    }
    for sequence in &fixture.sequences {
        let context = RuntimeEnvContext::new();
        for (index, step) in sequence.steps.iter().enumerate() {
            let env = env_map(&step.env);
            let sanitized = context.sanitize(&env);
            assert_eq!(
                serde_json::to_value(sanitized).expect("序列化"),
                step.expected,
                "{} step {index}: sanitize",
                sequence.name
            );
            assert_eq!(
                serde_json::to_value(context.captured_credentials()).expect("序列化"),
                step.captured,
                "{} step {index}: captured",
                sequence.name
            );
        }
    }
    for case in &fixture.keys {
        assert_eq!(
            should_sanitize_mode_runtime_env_key(&case.key),
            case.sanitize,
            "{}: sanitize key",
            case.key
        );
        assert_eq!(
            should_capture_mode_tool_env_passthrough_key(&case.key),
            case.capture,
            "{}: capture key",
            case.key
        );
    }
    for case in &fixture.read_passthrough {
        let actual = read_mode_tool_env_passthrough_env(&env_map(&case.env));
        assert_eq!(
            serde_json::to_value(actual).expect("序列化"),
            case.expected,
            "{}: readPassthrough",
            case.name
        );
    }
    for case in &fixture.build_passthrough {
        let actual = passthrough_payload(
            &serde_json::to_value(build_mode_tool_env_passthrough_env(&env_map(&case.env)))
                .expect("序列化"),
        );
        let expected = passthrough_payload(&case.expected);
        assert_eq!(actual, expected, "{}: buildPassthrough", case.name);
    }
    for (index, case) in fixture.agent_runtime.iter().enumerate() {
        let actual = build_agent_runtime_env(&AgentRuntimeEnvInput {
            http_proxy: case.input.http_proxy.clone(),
            no_proxy: case.input.no_proxy.clone(),
            ca_cert_path: case.input.ca_cert_path.clone(),
        });
        assert_eq!(
            serde_json::to_value(actual).expect("序列化"),
            case.expected,
            "agentRuntime[{index}]"
        );
    }
    for (index, case) in fixture.agent_endpoint.iter().enumerate() {
        let actual = build_agent_endpoint_origin_env(case.input.as_deref());
        assert_eq!(
            serde_json::to_value(actual).expect("序列化"),
            case.expected,
            "agentEndpoint[{index}]"
        );
    }
    for (index, case) in fixture.agent_workspace.iter().enumerate() {
        let actual = build_agent_workspace_identity_env(case.input.as_deref());
        assert_eq!(
            serde_json::to_value(actual).expect("序列化"),
            case.expected,
            "agentWorkspace[{index}]"
        );
    }

    for (index, case) in fixture.node_env.iter().enumerate() {
        let env: EnvMap = case
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        assert_eq!(
            is_effective_development_node_env(&env),
            case.expected,
            "nodeEnv[{index}]"
        );
    }
}
