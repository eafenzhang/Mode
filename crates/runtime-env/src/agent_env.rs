//! Agent spawn 时的代理、No Proxy、自定义 CA、端点 origin 与 workspace identity 环境补丁。
//!
//! 真源：`packages/services/src/runtime-tools/agentProxyEnv.ts`。

use indexmap::IndexMap;

use crate::{EnvMap, MODE_AGENT_CA_CERT_ENV_KEY, MODE_HTTP_PROXY_ENV_KEY, MODE_NO_PROXY_ENV_KEY};

pub const MODE_WORKSPACE_IDENTITY_ENV: &str = "MODE_WORKSPACE_IDENTITY";
pub const MODE_BASE_URL_ENV: &str = "MODE_BASE_URL";

const PROXY_ENV_KEYS: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    MODE_HTTP_PROXY_ENV_KEY,
];
const NO_PROXY_ENV_KEYS: &[&str] = &["NO_PROXY", "no_proxy", MODE_NO_PROXY_ENV_KEY];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentRuntimeEnvInput {
    pub http_proxy: Option<String>,
    pub no_proxy: Option<String>,
    pub ca_cert_path: Option<String>,
}

fn has_scheme(value: &str) -> bool {
    let Some(index) = value.find("://") else {
        return false;
    };
    let mut chars = value[..index].chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-'))
}

fn normalize_proxy_value(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 已带 scheme（http://、socks5:// 等）原样保留；裸 host:port 补 http://。
    if has_scheme(trimmed) {
        Some(trimmed.to_string())
    } else {
        Some(format!("http://{trimmed}"))
    }
}

fn normalize_no_proxy_value(value: Option<&str>) -> Option<String> {
    let tokens: Vec<&str> = value?
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect();
    (!tokens.is_empty()).then(|| tokens.join(","))
}

fn push_entries(output: &mut EnvMap, keys: &[&str], value: Option<String>) {
    if let Some(value) = value {
        for key in keys {
            output.insert((*key).to_string(), value.clone());
        }
    }
}

/// `buildAgentRuntimeEnv`：代理 + No Proxy + 自定义 CA。
pub fn build_agent_runtime_env(input: &AgentRuntimeEnvInput) -> EnvMap {
    let mut output = EnvMap::new();
    push_entries(
        &mut output,
        PROXY_ENV_KEYS,
        normalize_proxy_value(input.http_proxy.as_deref()),
    );
    push_entries(
        &mut output,
        NO_PROXY_ENV_KEYS,
        normalize_no_proxy_value(input.no_proxy.as_deref()),
    );
    if let Some(trimmed) = input
        .ca_cert_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        // NODE_EXTRA_CA_CERTS 必须在 Node 启动前存在；MODE_AGENT_CA_CERT 给运行时/工具子进程。
        output.insert("NODE_EXTRA_CA_CERTS".to_string(), trimmed.to_string());
        output.insert(MODE_AGENT_CA_CERT_ENV_KEY.to_string(), trimmed.to_string());
    }
    output
}

/// `buildAgentEndpointOriginEnv`。
pub fn build_agent_endpoint_origin_env(endpoint_origin: Option<&str>) -> EnvMap {
    let Some(trimmed) = endpoint_origin
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return EnvMap::new();
    };
    let mut output = EnvMap::new();
    output.insert(MODE_BASE_URL_ENV.to_string(), trimmed.to_string());
    output
}

/// `buildAgentWorkspaceIdentityEnv`。
pub fn build_agent_workspace_identity_env(workspace_identity: Option<&str>) -> EnvMap {
    let Some(trimmed) = workspace_identity
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return EnvMap::new();
    };
    let mut output = IndexMap::new();
    output.insert(MODE_WORKSPACE_IDENTITY_ENV.to_string(), trimmed.to_string());
    output
}
