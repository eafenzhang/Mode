//! Mode Built-in 缓存路径：按平台、App 版本与 Endpoint Origin 隔离 Active/LKG。
//!
//! 真源：`packages/provider-node/src/mode-builtin-cache-paths.ts`。
//! 只做路径与 key 计算；文件读写见 [`crate::materializer`]。

use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// `process.platform` + `process.arch` 的等价映射。
pub fn resolve_mode_builtin_client_platform(platform: &str, arch: &str) -> String {
    let target = if platform == "win32" {
        "windows"
    } else {
        platform
    };
    let arch = match arch {
        "arm64" => "aarch64",
        "x64" => "x86_64",
        "ia32" => "ia32",
        other => other,
    };
    format!("{target}-{arch}")
}

/// 直接读取当前进程平台；供生产装配使用。
pub fn resolve_mode_builtin_client_platform_from_process() -> String {
    let platform = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86" => "ia32",
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    resolve_mode_builtin_client_platform(platform, arch)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeBuiltinCachePathOptions {
    pub environment_config_root: PathBuf,
    pub platform: String,
    pub app_version: String,
    pub mode_endpoint_origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeBuiltinCachePaths {
    pub active_file_path: PathBuf,
    pub control_file_path: PathBuf,
}

/// 按平台与 App 版本隔离 Active/LKG；路径本身就是兼容范围。
pub fn resolve_mode_builtin_cache_paths(
    options: &ModeBuiltinCachePathOptions,
) -> Result<ModeBuiltinCachePaths, String> {
    let platform = normalize_segment(&options.platform, "platform")?;
    let app_version = normalize_segment(&options.app_version, "appVersion")?;
    let endpoint_key = create_mode_builtin_endpoint_key(&options.mode_endpoint_origin)?;
    let directory = options
        .environment_config_root
        .join("runtime")
        .join("provider")
        .join(platform)
        .join(app_version)
        .join(endpoint_key);
    Ok(ModeBuiltinCachePaths {
        active_file_path: directory.join("mode-builtin.json"),
        control_file_path: directory.join("mode-builtin-refresh.json"),
    })
}

/// 将 Mode 控制面 Origin 规范化后映射为安全、稳定且碰撞风险可忽略的缓存路径段。
pub fn create_mode_builtin_endpoint_key(mode_endpoint_origin: &str) -> Result<String, String> {
    let normalized = normalize_mode_builtin_endpoint_origin(mode_endpoint_origin)?;
    let digest = Sha256::digest(normalized.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("endpoint-{}", &hex[..32]))
}

pub fn normalize_mode_builtin_endpoint_origin(value: &str) -> Result<String, String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err("Mode Built-in Endpoint Origin 不能为空".to_string());
    }
    let url = url::Url::parse(normalized)
        .map_err(|_| "Mode Built-in Endpoint Origin 只支持 HTTP(S)".to_string())?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("Mode Built-in Endpoint Origin 只支持 HTTP(S)".to_string());
    }
    Ok(url.origin().ascii_serialization())
}

fn normalize_segment(value: &str, name: &str) -> Result<String, String> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized == "."
        || normalized == ".."
        || normalized.contains('/')
        || normalized.contains('\\')
    {
        return Err(format!("Mode Built-in {name} 不是合法路径段"));
    }
    Ok(normalized.to_string())
}
