//! bundled 工具（bfs / ripgrep / ugrep）二进制解析与 PATH 补丁。
//!
//! 真源：`packages/services/src/runtime-tools/runtimeToolResolver.ts`。
//! 文件系统探测通过 [`ToolFileSystem`] 注入；规范见 `docs/specs/runtime-tools.md`。

use std::path::{Component, Path, PathBuf};

mod login_shell;

pub use login_shell::{
    build_login_shell_args, build_login_shell_execution_options, build_shell_bootstrap_path,
    capture_login_shell_env_snapshot, default_posix_bootstrap_path, extract_captured_env_snapshot,
    parse_null_separated_env_snapshot, resolve_shell_path_for_login_env, CaptureLoginShellOptions,
    LoginShellExecutionOptions, LoginShellExecutor, LoginShellFuture, LoginShellPath,
    DEFAULT_POSIX_BOOTSTRAP_PATH_DARWIN, DEFAULT_POSIX_BOOTSTRAP_PATH_LINUX,
    LOGIN_ENV_CAPTURE_PREFIX, LOGIN_ENV_CAPTURE_SUFFIX,
};
mod agent_runtime;
mod client_platform;

pub use agent_runtime::{
    find_mode_agent_runtime_binary, find_mode_agent_runtime_node_bundle, AgentRuntimeContext,
};
pub use client_platform::resolve_client_config_platform;
pub use runtime_env::EnvMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolId {
    Bfs,
    Ripgrep,
    Ugrep,
}

impl ToolId {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "bfs" => Some(Self::Bfs),
            "ripgrep" => Some(Self::Ripgrep),
            "ugrep" => Some(Self::Ugrep),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bfs => "bfs",
            Self::Ripgrep => "ripgrep",
            Self::Ugrep => "ugrep",
        }
    }

    fn binary_env_var(self) -> &'static str {
        match self {
            Self::Bfs => "MODE_BFS_BINARY",
            Self::Ripgrep => "MODE_RG_BINARY",
            Self::Ugrep => "MODE_UGREP_BINARY",
        }
    }

    fn bundled_resource_dir(self) -> &'static str {
        self.as_str()
    }

    fn binary_name(self) -> &'static str {
        match self {
            Self::Bfs => "bfs",
            Self::Ripgrep => "rg",
            Self::Ugrep => "ugrep",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathFlavor {
    Windows,
    Posix,
}

impl PathFlavor {
    pub fn from_platform(platform: &str) -> Self {
        if platform == "win32" {
            Self::Windows
        } else {
            Self::Posix
        }
    }

    fn delimiter(self) -> char {
        match self {
            Self::Windows => ';',
            Self::Posix => ':',
        }
    }
}

pub trait ToolFileSystem {
    fn exists(&self, path: &Path) -> bool;
    fn is_executable(&self, path: &Path) -> bool;
}

/// 生产实现：`exists` 跟随符号链接；可执行位在 Windows 上只要求是文件。
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeToolFileSystem;

impl ToolFileSystem for NativeToolFileSystem {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_executable(&self, path: &Path) -> bool {
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
        }
        // Node 的 `access(X_OK)` 在 Windows 上等同于存在性检查（目录也会通过）。
        #[cfg(not(unix))]
        {
            let _ = metadata;
            true
        }
    }
}

pub struct RuntimeToolContext<'a> {
    /// Node 的 `process.platform` 取值（win32/darwin/linux）。
    pub platform: String,
    pub arch: String,
    pub cwd: PathBuf,
    pub module_dir: Option<PathBuf>,
    pub resources_path: Option<String>,
    pub env: EnvMap,
    pub fs: &'a dyn ToolFileSystem,
}

impl RuntimeToolContext<'_> {
    fn flavor(&self) -> PathFlavor {
        PathFlavor::from_platform(&self.platform)
    }

    fn entry_segments(&self, tool: ToolId) -> Vec<String> {
        let mut name = tool.binary_name().to_string();
        if self.platform == "win32" {
            name.push_str(".exe");
        }
        vec![name]
    }
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if output.file_name().is_some() {
                    output.pop();
                } else if !output.has_root() {
                    output.push("..");
                }
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

pub(crate) fn resolve_path(base: &Path, parts: &[&str]) -> PathBuf {
    let mut path = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(base)
    };
    for part in parts {
        let part = Path::new(part);
        if part.is_absolute() {
            path = part.to_path_buf();
        } else {
            path.push(part);
        }
    }
    normalize_path(path)
}

/// Node `path.join`/`resolve` 在 Windows 上把分隔符统一成 `\`。
fn native_path_string(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('/', "\\")
    } else {
        text.into_owned()
    }
}

fn split_path_entries(value: Option<&str>, flavor: PathFlavor) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(flavor.delimiter())
                .filter(|entry| !entry.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn join_unique_path_entries(
    entries: impl IntoIterator<Item = String>,
    flavor: PathFlavor,
) -> String {
    let mut unique = Vec::new();
    for entry in entries {
        if entry.is_empty() || unique.contains(&entry) {
            continue;
        }
        unique.push(entry);
    }
    unique.join(&flavor.delimiter().to_string())
}

pub fn prepend_path_entries(
    current_path: Option<&str>,
    entries: &[String],
    flavor: PathFlavor,
) -> String {
    let current = split_path_entries(current_path, flavor);
    join_unique_path_entries(entries.iter().cloned().chain(current), flavor)
}

pub fn append_path_entries(
    current_path: Option<&str>,
    entries: &[String],
    flavor: PathFlavor,
) -> String {
    let current = split_path_entries(current_path, flavor);
    join_unique_path_entries(current.into_iter().chain(entries.iter().cloned()), flavor)
}

fn is_executable_file(path: &Path, fs: &dyn ToolFileSystem) -> bool {
    fs.exists(path) && fs.is_executable(path)
}

fn resolve_existing_path(
    candidates: &[Option<PathBuf>],
    fs: &dyn ToolFileSystem,
) -> Option<PathBuf> {
    for candidate in candidates.iter().flatten() {
        if is_executable_file(candidate, fs) {
            return Some(candidate.clone());
        }
    }
    None
}

fn platform_key(context: &RuntimeToolContext<'_>) -> String {
    format!("{}-{}", context.platform, context.arch)
}

fn platform_scoped_bundled_tool_roots(context: &RuntimeToolContext<'_>) -> Vec<Option<PathBuf>> {
    let key = platform_key(context);
    let mut roots = vec![
        Some(resolve_path(&context.cwd, &["bundled-tools", key.as_str()])),
        Some(resolve_path(
            &context.cwd,
            &["packages", "desktop", "bundled-tools", key.as_str()],
        )),
        Some(resolve_path(
            &context.cwd,
            &["..", "desktop", "bundled-tools", key.as_str()],
        )),
    ];
    if let Some(module_dir) = &context.module_dir {
        roots.push(Some(resolve_path(
            module_dir,
            &["..", "..", "..", "desktop", "bundled-tools", key.as_str()],
        )));
        roots.push(Some(resolve_path(
            module_dir,
            &["..", "..", "desktop", "bundled-tools", key.as_str()],
        )));
    } else {
        roots.push(None);
        roots.push(None);
    }
    roots
}

fn resolve_command_on_path(command: &str, context: &RuntimeToolContext<'_>) -> Option<String> {
    let path_env = context.env.get("PATH")?;
    let flavor = context.flavor();
    let extensions: Vec<String> = if context.platform == "win32" && !command.contains('.') {
        match context.env.get("PATHEXT") {
            Some(value) => value
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(str::to_string)
                .collect(),
            None => [".EXE", ".CMD", ".BAT", ".COM"]
                .iter()
                .map(|extension| extension.to_string())
                .collect(),
        }
    } else {
        vec![String::new()]
    };
    for entry in path_env.split(flavor.delimiter()) {
        if entry.is_empty() {
            continue;
        }
        for extension in &extensions {
            let candidate = Path::new(entry).join(format!("{command}{extension}"));
            if is_executable_file(&candidate, context.fs) {
                return Some(native_path_string(&candidate));
            }
        }
    }
    None
}

fn strip_exe_suffix(name: &str) -> &str {
    if name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(".exe") {
        &name[..name.len() - 4]
    } else {
        name
    }
}

pub fn find_runtime_tool_binary(tool: ToolId, context: &RuntimeToolContext<'_>) -> Option<String> {
    let segments = context.entry_segments(tool);
    let env_path = context
        .env
        .get(tool.binary_env_var())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    if let Some(env_path) = env_path {
        let candidate = Path::new(env_path);
        if is_executable_file(candidate, context.fs) {
            return Some(env_path.to_string());
        }
    }

    let mut parts: Vec<&str> = vec!["tools", tool.bundled_resource_dir()];
    parts.extend(segments.iter().map(String::as_str));
    let mut candidates: Vec<Option<PathBuf>> = Vec::new();
    if let Some(runtime_root) = context
        .env
        .get("MODE_SERVER_RUNTIME_ROOT")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        candidates.push(Some(resolve_path(Path::new(runtime_root), &parts)));
    } else {
        candidates.push(None);
    }
    if let Some(resources_path) = &context.resources_path {
        candidates.push(Some(resolve_path(Path::new(resources_path), &parts)));
    } else {
        candidates.push(None);
    }
    for root in platform_scoped_bundled_tool_roots(context) {
        match root {
            Some(root) => {
                let mut root_parts: Vec<&str> = vec![tool.bundled_resource_dir()];
                root_parts.extend(segments.iter().map(String::as_str));
                candidates.push(Some(resolve_path(&root, &root_parts)));
            }
            None => candidates.push(None),
        }
    }
    if let Some(candidate) = resolve_existing_path(&candidates, context.fs) {
        return Some(native_path_string(&candidate));
    }

    let binary_name = segments.last()?;
    resolve_command_on_path(strip_exe_suffix(binary_name), context)
}

/// `buildRuntimeToolEnvPatch`：命中的工具写 binary env，并把目录追加到 PATH。
pub fn build_runtime_tool_env_patch(
    tool_ids: &[ToolId],
    context: &RuntimeToolContext<'_>,
) -> EnvMap {
    let mut patch = EnvMap::new();
    let mut path_entries: Vec<String> = Vec::new();
    for tool in tool_ids {
        let Some(binary_path) = find_runtime_tool_binary(*tool, context) else {
            continue;
        };
        patch.insert((*tool).binary_env_var().to_string(), binary_path.clone());
        if let Some(parent) = Path::new(&binary_path).parent() {
            path_entries.push(parent.to_string_lossy().into_owned());
        }
    }
    if !path_entries.is_empty() {
        let path = append_path_entries(
            context.env.get("PATH").map(String::as_str),
            &path_entries,
            context.flavor(),
        );
        patch.insert("PATH".to_string(), path);
    }
    patch
}
