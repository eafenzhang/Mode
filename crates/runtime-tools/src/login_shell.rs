//! POSIX login shell 环境采集的纯部分：bootstrap PATH、shell 选择、参数、执行选项与输出解析。
//!
//! 真源：`packages/services/src/runtime-tools/runtimeLoginShellEnvCapture.ts`。
//! 真正拉起进程、超时/进程组终止与缓存仍由调用方实现。

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use runtime_env::EnvMap;

use crate::{prepend_path_entries, PathFlavor, ToolFileSystem};

pub const LOGIN_ENV_CAPTURE_PREFIX: &str = "__MODE_LOGIN_ENV_START__";
pub const LOGIN_ENV_CAPTURE_SUFFIX: &str = "__MODE_LOGIN_ENV_END__";
pub const DEFAULT_POSIX_BOOTSTRAP_PATH_DARWIN: &str =
    "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin";
pub const DEFAULT_POSIX_BOOTSTRAP_PATH_LINUX: &str =
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub fn default_posix_bootstrap_path(platform: &str) -> &'static str {
    if platform == "darwin" {
        DEFAULT_POSIX_BOOTSTRAP_PATH_DARWIN
    } else {
        DEFAULT_POSIX_BOOTSTRAP_PATH_LINUX
    }
}

/// `buildShellBootstrapPath`：把系统目录前置到当前 PATH（POSIX 分隔符）。
///
/// `platform` 用于选择 darwin 与 Linux 的默认目录（TS 的模块级常量）。
pub fn build_shell_bootstrap_path(platform: &str, current_path: Option<&str>) -> String {
    let entries: Vec<String> = default_posix_bootstrap_path(platform)
        .split(':')
        .map(str::to_string)
        .collect();
    prepend_path_entries(current_path, &entries, PathFlavor::from_platform(platform))
}

/// `resolveShellPathForLoginEnv`：$SHELL → /bin/zsh → /bin/bash → /bin/sh。
pub fn resolve_shell_path_for_login_env(env: &EnvMap, fs: &dyn ToolFileSystem) -> Option<String> {
    let candidates = [
        env.get("SHELL").map(String::as_str),
        Some("/bin/zsh"),
        Some("/bin/bash"),
        Some("/bin/sh"),
    ];
    for candidate in candidates {
        let Some(candidate) = candidate.filter(|value| !value.is_empty()) else {
            continue;
        };
        let path = std::path::Path::new(candidate);
        if fs.exists(path) && fs.is_executable(path) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn login_shell_command() -> String {
    format!(
        "printf '%s\\0' '{LOGIN_ENV_CAPTURE_PREFIX}'; env -0; printf '%s\\0' '{LOGIN_ENV_CAPTURE_SUFFIX}'"
    )
}

/// `buildLoginShellArgs`：zsh/bash 走交互式登录 shell，其余走 `-lc`。
pub fn build_login_shell_args(shell_path: &str) -> Vec<String> {
    let interactive = shell_path.ends_with("/zsh") || shell_path.ends_with("/bash");
    vec![
        if interactive { "-ilc" } else { "-lc" }.to_string(),
        login_shell_command(),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginShellExecutionOptions {
    pub timeout_ms: u64,
    pub max_buffer_bytes: usize,
    pub env: EnvMap,
}

/// `buildLoginShellExecutionOptions`：探测 shell 用最小系统 PATH，并固定 TERM/CI。
pub fn build_login_shell_execution_options(
    platform: &str,
    base_env: &EnvMap,
    timeout_ms: u64,
) -> LoginShellExecutionOptions {
    let mut env = base_env.clone();
    env.insert(
        "PATH".to_string(),
        build_shell_bootstrap_path(platform, base_env.get("PATH").map(String::as_str)),
    );
    env.insert("TERM".to_string(), "dumb".to_string());
    env.insert("CI".to_string(), "1".to_string());
    LoginShellExecutionOptions {
        timeout_ms,
        max_buffer_bytes: 2 * 1024 * 1024,
        env,
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

/// `parseNullSeparatedEnvSnapshot`。
pub fn parse_null_separated_env_snapshot(raw_output: &str) -> EnvMap {
    let mut result = EnvMap::new();
    for entry in raw_output.split('\0') {
        if entry.is_empty() {
            continue;
        }
        let Some(separator) = entry.find('=') else {
            continue;
        };
        if separator == 0 {
            continue;
        }
        let key = entry[..separator].trim();
        if !is_identifier_key(key) {
            continue;
        }
        result.insert(key.to_string(), entry[separator + 1..].to_string());
    }
    result
}

pub type LoginShellFuture<'a> = Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

/// 注入的 login shell 执行器；超时由捕获层负责，执行器负责终止自己的子进程。
pub trait LoginShellExecutor: Send + Sync {
    fn execute(
        &self,
        shell_path: &str,
        args: &[String],
        options: &LoginShellExecutionOptions,
    ) -> LoginShellFuture<'_>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginShellPath {
    /// 对应 TS `shellPath: undefined`：按 `$SHELL` → `/bin/*` 解析。
    Resolve,
    /// 对应 TS `shellPath: null`：明确不采集。
    None,
    Explicit(String),
}

pub struct CaptureLoginShellOptions<'a> {
    pub base_env: &'a EnvMap,
    /// 对应 TS `options.platform`，只用于 win32/VITEST 短路。
    pub platform: &'a str,
    /// 对应 TS 的 `process.platform`，决定 bootstrap PATH 的默认目录与分隔符。
    pub process_platform: &'a str,
    pub shell_path: LoginShellPath,
    pub timeout_ms: u64,
    pub executor: &'a dyn LoginShellExecutor,
}

/// `captureLoginShellEnvSnapshot`：短路 → shell 选择 → 执行（带 deadline）→ 解析。
/// 任何失败都返回 `None`（与 TS 的 catch 一致）。
pub async fn capture_login_shell_env_snapshot(
    options: &CaptureLoginShellOptions<'_>,
) -> Option<EnvMap> {
    if options.platform == "win32"
        || options
            .base_env
            .get("VITEST")
            .is_some_and(|value| !value.is_empty())
    {
        return None;
    }
    let shell = match &options.shell_path {
        LoginShellPath::None => return None,
        LoginShellPath::Explicit(shell) => Some(shell.clone()),
        LoginShellPath::Resolve => {
            // 解析需要文件系统；调用方应通过显式路径或预先校验。这里用生产实现。
            resolve_shell_path_for_login_env(options.base_env, &crate::NativeToolFileSystem)
        }
    }?;
    let args = build_login_shell_args(&shell);
    let execution_options = build_login_shell_execution_options(
        options.process_platform,
        options.base_env,
        options.timeout_ms,
    );
    let execution = options.executor.execute(&shell, &args, &execution_options);
    match tokio::time::timeout(Duration::from_millis(options.timeout_ms), execution).await {
        Ok(Ok(output)) => extract_captured_env_snapshot(&output),
        Ok(Err(_)) | Err(_) => None,
    }
}

/// `extractCapturedEnvSnapshot`：取最后一对标记之间的 `env -0` 输出。
pub fn extract_captured_env_snapshot(raw_output: &str) -> Option<EnvMap> {
    let start_marker = format!("{LOGIN_ENV_CAPTURE_PREFIX}\0");
    let end_marker = format!("{LOGIN_ENV_CAPTURE_SUFFIX}\0");
    let start_index = raw_output.rfind(&start_marker)?;
    let end_index = raw_output.rfind(&end_marker)?;
    if end_index <= start_index {
        return None;
    }
    Some(parse_null_separated_env_snapshot(
        &raw_output[start_index + start_marker.len()..end_index],
    ))
}
