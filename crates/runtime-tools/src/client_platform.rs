//! 客户端配置平台标识（`MODE_TEST_*` 覆盖 → 发布平台/架构映射）。
//!
//! 真源：`packages/services/src/runtime-tools/clientPlatform.ts`。

use runtime_env::EnvMap;

fn read_override<'a>(env: &'a EnvMap, name: &str) -> Option<&'a str> {
    env.get(name)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

fn release_target(env: &EnvMap, platform: &str) -> String {
    if let Some(override_value) = read_override(env, "MODE_TEST_RELEASE_TARGET") {
        return override_value.to_string();
    }
    match platform {
        "darwin" => "darwin".to_string(),
        "linux" => "linux".to_string(),
        "win32" => "windows".to_string(),
        other => other.to_string(),
    }
}

fn release_arch(env: &EnvMap, arch: &str) -> String {
    if let Some(override_value) = read_override(env, "MODE_TEST_RELEASE_ARCH") {
        return override_value.to_string();
    }
    match arch {
        "x64" => "x86_64".to_string(),
        "arm64" => "aarch64".to_string(),
        other => other.to_string(),
    }
}

/// `resolveClientConfigPlatform`。
pub fn resolve_client_config_platform(env: &EnvMap, platform: &str, arch: &str) -> String {
    if let Some(override_value) = read_override(env, "MODE_TEST_CLIENT_CONFIG_PLATFORM") {
        return override_value.to_string();
    }
    format!(
        "{}-{}",
        release_target(env, platform),
        release_arch(env, arch)
    )
}
