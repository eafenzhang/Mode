//! `runtime-paths.ts` 的 Rust 对应：MODE_* 环境变量路径解析。

use std::collections::BTreeMap;
use std::fmt;

pub const MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV: &str = "MODE_BUILTIN_PROVIDER_CONFIG_FILE";
pub const MODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE_ENV: &str =
    "MODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE";
pub const MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV: &str = "MODE_PERSONAL_PROVIDER_CONFIG_FILE";
pub const PERSONAL_PROVIDER_CONFIG_FILE_NAME: &str = "provider_config.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRuntimePaths {
    pub mode_builtin_file_path: String,
    pub personal_file_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePathError(String);

impl RuntimePathError {
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuntimePathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RuntimePathError {}

pub fn resolve_node_provider_runtime_paths(
    env: &BTreeMap<String, String>,
) -> Result<Option<ProviderRuntimePaths>, RuntimePathError> {
    let mode_builtin = env
        .get(MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    let personal = env
        .get(MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    if mode_builtin.is_none() && personal.is_none() {
        return Ok(None);
    }
    let (Some(mode_builtin), Some(personal)) = (mode_builtin, personal) else {
        return Err(RuntimePathError(
            "Mode Built-in 与 Personal Provider Config 路径必须同时提供".to_string(),
        ));
    };
    Ok(Some(ProviderRuntimePaths {
        mode_builtin_file_path: mode_builtin.to_string(),
        personal_file_path: personal.to_string(),
    }))
}

pub fn create_node_provider_runtime_path_env(
    paths: &ProviderRuntimePaths,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV.to_string(),
            paths.mode_builtin_file_path.clone(),
        ),
        (
            MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV.to_string(),
            paths.personal_file_path.clone(),
        ),
    ])
}
