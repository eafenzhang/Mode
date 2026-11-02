//! Agent/tool 子进程环境键策略。
//!
//! 真源：`packages/shared/src/runtimeEnv.ts` 与
//! `packages/services/src/runtime-tools/agentProxyEnv.ts`。纯函数 + 进程内 CUA
//! 凭据捕获；不含 IO。规范见 `docs/specs/runtime-env.md`。

mod agent_env;

use std::sync::Mutex;

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Map, Value};

pub use agent_env::{
    build_agent_endpoint_origin_env, build_agent_runtime_env, build_agent_workspace_identity_env,
    AgentRuntimeEnvInput, MODE_BASE_URL_ENV, MODE_WORKSPACE_IDENTITY_ENV,
};

pub const MODE_RUNTIME_ENV_KEY: &str = "MODE_RUNTIME_ENV";
pub const MODE_HTTP_PROXY_ENV_KEY: &str = "MODE_HTTP_PROXY";
pub const MODE_NO_PROXY_ENV_KEY: &str = "MODE_NO_PROXY";
pub const MODE_REMOTE_RUNTIME_NETWORK_AUTHORITY_ENV_KEY: &str =
    "MODE_REMOTE_RUNTIME_NETWORK_AUTHORITY";
pub const MODE_REMOTE_HTTP_PROXY_ENV_KEY: &str = "MODE_REMOTE_HTTP_PROXY";
pub const MODE_REMOTE_NO_PROXY_ENV_KEY: &str = "MODE_REMOTE_NO_PROXY";
pub const MODE_AGENT_CA_CERT_ENV_KEY: &str = "MODE_AGENT_CA_CERT";
pub const MODE_TOOL_ENV_PASSTHROUGH_ENV_KEY: &str = "MODE_TOOL_ENV_PASSTHROUGH_JSON";
pub const MODE_DESKTOP_CONTEXT_PROMPT_ENABLED_ENV: &str = "MODE_DESKTOP_CONTEXT_PROMPT_ENABLED";
pub const MODE_CUA_PRODUCT_HELPER_ENV_KEY: &str = "MODE_CUA_PRODUCT_HELPER";
pub const MODE_CUA_BROKER_SOCKET_ENV_KEY: &str = "MODE_CUA_PERMISSION_BROKER_SOCKET";
pub const MODE_CUA_NODE_REPL_HOST_ENV_KEY: &str = "MODE_CUA_NODE_REPL_HOST";
pub const MODE_CUA_DEV_MODE_ENV_KEY: &str = "MODE_CUA_DEV_MODE";
pub const MODE_CUA_PLUGIN_AUTHORITY_ENV_KEY: &str = "MODE_CUA_PLUGIN_AUTHORITY";
pub const MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER_ENV_KEY: &str =
    "MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER";

pub type EnvMap = IndexMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeRuntimeEnv {
    Development,
    Production,
    Test,
}

impl ModeRuntimeEnv {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Production => "production",
            Self::Test => "test",
        }
    }
}

const SANITIZED_RUNTIME_ENV_KEYS: &[&str] = &[
    "NODE_ENV",
    "ELECTRON_RUN_AS_NODE",
    "NODE_NO_WARNINGS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    MODE_REMOTE_RUNTIME_NETWORK_AUTHORITY_ENV_KEY,
    MODE_REMOTE_HTTP_PROXY_ENV_KEY,
    MODE_REMOTE_NO_PROXY_ENV_KEY,
    MODE_CUA_BROKER_SOCKET_ENV_KEY,
    "MODE_CUA_PERMISSION_BROKER_TOKEN",
    MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER_ENV_KEY,
    MODE_CUA_PLUGIN_AUTHORITY_ENV_KEY,
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    "OTEL_EXPORTER_OTLP_HEADERS",
    "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
    "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
    "OTEL_EXPORTER_OTLP_METRICS_HEADERS",
    "OTEL_SERVICE_NAME",
    "OTEL_RESOURCE_ATTRIBUTES",
    "OTEL_EXPORTER_OTLP_COMPRESSION",
    "MODE_MODEL_TELEMETRY_ENABLED",
    "MODE_TELEMETRY_DEVICE_MID",
    "MODE_TELEMETRY_USER_ID",
    "MODE_TELEMETRY_USER_ID_HASH",
    "MODE_TELEMETRY_USER_SUBJECT_ID",
    "MODE_TELEMETRY_IDENTITY_STATE",
    "MODE_TELEMETRY_RUNTIME_SURFACE",
    "MODE_TELEMETRY_RUNTIME_DISTRIBUTION",
];

const NON_TOOL_PASSTHROUGH_RUNTIME_ENV_KEYS: &[&str] = &[
    "NODE_ENV",
    "ELECTRON_RUN_AS_NODE",
    "NODE_NO_WARNINGS",
    MODE_CUA_BROKER_SOCKET_ENV_KEY,
    MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER_ENV_KEY,
    MODE_CUA_PLUGIN_AUTHORITY_ENV_KEY,
    MODE_REMOTE_RUNTIME_NETWORK_AUTHORITY_ENV_KEY,
    MODE_REMOTE_HTTP_PROXY_ENV_KEY,
    MODE_REMOTE_NO_PROXY_ENV_KEY,
];

fn is_package_manager_proxy_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    for prefix in ["npm_config_", "yarn_", "pnpm_"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            if matches!(
                rest,
                "http_proxy" | "https_proxy" | "proxy" | "all_proxy" | "no_proxy" | "cafile" | "ca"
            ) {
                return true;
            }
        }
    }
    false
}

pub fn should_sanitize_mode_runtime_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    SANITIZED_RUNTIME_ENV_KEYS
        .iter()
        .any(|candidate| *candidate == upper)
        || is_package_manager_proxy_key(key)
}

fn is_mode_agent_telemetry_env_key(upper_key: &str) -> bool {
    upper_key.starts_with("OTEL_")
        || upper_key.starts_with("MODE_TELEMETRY_")
        || upper_key == "MODE_MODEL_TELEMETRY_ENABLED"
}

pub fn should_capture_mode_tool_env_passthrough_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    if is_mode_agent_telemetry_env_key(&upper) {
        return false;
    }
    if NON_TOOL_PASSTHROUGH_RUNTIME_ENV_KEYS
        .iter()
        .any(|candidate| *candidate == upper)
    {
        return false;
    }
    should_sanitize_mode_runtime_env_key(key)
}

pub fn normalize_mode_runtime_env(value: Option<&str>) -> Option<ModeRuntimeEnv> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "development" => Some(ModeRuntimeEnv::Development),
        "production" => Some(ModeRuntimeEnv::Production),
        "test" => Some(ModeRuntimeEnv::Test),
        _ => None,
    }
}

pub fn resolve_mode_runtime_env(env: &EnvMap, fallback: ModeRuntimeEnv) -> ModeRuntimeEnv {
    normalize_mode_runtime_env(env.get(MODE_RUNTIME_ENV_KEY).map(String::as_str))
        .unwrap_or(fallback)
}

/// `isEffectiveDevelopmentNodeEnv`：只认 `MODE_RUNTIME_ENV`，test 按 production 处理。
pub fn is_effective_development_node_env(env: &EnvMap) -> bool {
    resolve_mode_runtime_env(env, ModeRuntimeEnv::Production) == ModeRuntimeEnv::Development
}

fn env_flag_is_on(value: Option<&String>) -> bool {
    matches!(
        value.map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if value == "1" || value == "true" || value == "on"
    )
}

pub fn is_cua_dev_mode_requested(env: &EnvMap) -> bool {
    env_flag_is_on(env.get(MODE_CUA_DEV_MODE_ENV_KEY))
}

pub fn is_mode_cua_internal_feature_enabled(env: &EnvMap) -> bool {
    if is_cua_dev_mode_requested(env) {
        return true;
    }
    let explicit = env
        .get(MODE_CUA_PRODUCT_HELPER_ENV_KEY)
        .map(|value| value.trim().to_ascii_lowercase());
    !matches!(explicit.as_deref(), Some("0" | "false" | "off"))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedCuaBrokerCredentials {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_authority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_marker: Option<String>,
}

fn trimmed(env: &EnvMap, key: &str) -> Option<String> {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 进程内 CUA 凭据捕获状态（对应 TS 模块级变量）。
#[derive(Default)]
pub struct RuntimeEnvContext {
    captured: Mutex<Option<CapturedCuaBrokerCredentials>>,
}

impl RuntimeEnvContext {
    pub fn new() -> Self {
        Self::default()
    }

    fn capture(&self, env: &EnvMap) {
        let socket = trimmed(env, MODE_CUA_BROKER_SOCKET_ENV_KEY);
        let plugin_authority = trimmed(env, MODE_CUA_PLUGIN_AUTHORITY_ENV_KEY);
        let refresh_marker = trimmed(env, MODE_CUA_PERMISSION_BROKER_REFRESH_MARKER_ENV_KEY);
        let mut captured = self.captured.lock().expect("captured credentials lock");
        match (socket, plugin_authority) {
            (Some(socket), Some(plugin_authority)) => {
                *captured = Some(CapturedCuaBrokerCredentials {
                    socket: Some(socket),
                    plugin_authority: Some(plugin_authority),
                    refresh_marker,
                });
            }
            (None, None) => {}
            // 半组凭据说明上游注入不完整或正在轮换：清掉旧快照并 fail-closed。
            _ => *captured = None,
        }
    }

    pub fn captured_credentials(&self) -> CapturedCuaBrokerCredentials {
        self.captured
            .lock()
            .expect("captured credentials lock")
            .clone()
            .unwrap_or_default()
    }

    pub fn reset_for_test(&self) {
        *self.captured.lock().expect("captured credentials lock") = None;
    }

    pub fn sanitize(&self, env: &EnvMap) -> EnvMap {
        self.capture(env);
        env.iter()
            .filter(|(key, _)| !should_sanitize_mode_runtime_env_key(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    pub fn sanitize_in_place(&self, env: &mut EnvMap) {
        self.capture(env);
        env.retain(|key, _| !should_sanitize_mode_runtime_env_key(key));
    }
}

fn is_identifier_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// `readModeToolEnvPassthroughEnv`。
pub fn read_mode_tool_env_passthrough_env(env: &EnvMap) -> EnvMap {
    let Some(raw) = env
        .get(MODE_TOOL_ENV_PASSTHROUGH_ENV_KEY)
        .filter(|raw| !raw.is_empty())
    else {
        return EnvMap::new();
    };
    let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(raw) else {
        return EnvMap::new();
    };
    let mut captured = EnvMap::new();
    for (key, value) in parsed {
        let Some(value) = value.as_str() else {
            continue;
        };
        if is_identifier_key(&key) && should_capture_mode_tool_env_passthrough_key(&key) {
            captured.insert(key, value.to_string());
        }
    }
    captured
}

/// `buildModeToolEnvPassthroughEnv`：把可透传键合并后按 key 排序编码为 JSON。
pub fn build_mode_tool_env_passthrough_env(env: &EnvMap) -> EnvMap {
    let mut captured = read_mode_tool_env_passthrough_env(env);
    for (key, value) in env {
        if should_capture_mode_tool_env_passthrough_key(key) {
            captured.insert(key.clone(), value.clone());
        }
    }
    if captured.is_empty() {
        return EnvMap::new();
    }
    let mut entries: Vec<(&String, &String)> = captured.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    let mut object = Map::new();
    for (key, value) in entries {
        object.insert(key.clone(), Value::String(value.clone()));
    }
    let encoded = serde_json::to_string(&Value::Object(object)).expect("JSON 序列化不会失败");
    let mut output = EnvMap::new();
    output.insert(MODE_TOOL_ENV_PASSTHROUGH_ENV_KEY.to_string(), encoded);
    output
}
