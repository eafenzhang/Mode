//! Mode Agent 运行时（原生二进制 / JS bundle）资源查找。
//!
//! 真源：`packages/services/src/runtime-tools/providerRuntimeResolver.ts`。
//! 只做候选路径与存在性判断，不负责 spawn。

use std::path::{Path, PathBuf};

use runtime_env::EnvMap;

use crate::{resolve_path, ToolFileSystem};

pub const AGENT_BINARY_ENV_VAR: &str = "GLM_BINARY_PATH";
pub const AGENT_BUNDLED_RESOURCE_DIR: &str = "glm";
pub const AGENT_BINARY_NAME: &str = "mode-agent";
pub const AGENT_NODE_BUNDLE_ENTRY: &str = "mode.cjs";
pub const MODE_DATA_ROOT_DIR_NAME: &str = ".mode";

pub struct AgentRuntimeContext<'a> {
    pub platform: String,
    pub arch: String,
    pub cwd: PathBuf,
    pub module_dir: Option<PathBuf>,
    pub resources_path: Option<String>,
    pub home_dir: PathBuf,
    pub env: EnvMap,
    pub fs: &'a dyn ToolFileSystem,
}

fn resolve_existing_path(
    candidates: &[Option<PathBuf>],
    fs: &dyn ToolFileSystem,
) -> Option<String> {
    for candidate in candidates.iter().flatten() {
        if fs.exists(candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// Node 侧平台代号（`process.platform`）。Rust 的 `std::env::consts::OS` 给出
/// `windows`/`macos`，而随包目录与 `.exe` 后缀按 `win32-*` 布局；这里统一归一。
fn platform_token(platform: &str) -> &str {
    match platform {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

/// Node 侧架构代号（`process.arch`）：`x86_64` → `x64`、`aarch64` → `arm64`。
fn arch_token(arch: &str) -> &str {
    match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

fn platform_key(context: &AgentRuntimeContext<'_>) -> String {
    format!(
        "{}-{}",
        platform_token(&context.platform),
        arch_token(&context.arch)
    )
}

fn platform_scoped_bundled_agent_roots(context: &AgentRuntimeContext<'_>) -> Vec<Option<PathBuf>> {
    let key = platform_key(context);
    let mut roots = vec![
        Some(resolve_path(&context.cwd, &["bundled-agents", &key])),
        Some(resolve_path(
            &context.cwd,
            &["packages", "desktop", "bundled-agents", &key],
        )),
        Some(resolve_path(
            &context.cwd,
            &["..", "desktop", "bundled-agents", &key],
        )),
    ];
    if let Some(module_dir) = &context.module_dir {
        roots.push(Some(resolve_path(
            module_dir,
            &["..", "..", "desktop", "bundled-agents", &key],
        )));
        roots.push(Some(resolve_path(
            module_dir,
            &["..", "..", "bundled-agents", &key],
        )));
    } else {
        roots.push(None);
        roots.push(None);
    }
    roots
}

fn legacy_bundled_resource_roots(context: &AgentRuntimeContext<'_>) -> Vec<Option<PathBuf>> {
    let mut roots = vec![
        Some(resolve_path(&context.cwd, &["bundled-resources"])),
        Some(resolve_path(
            &context.cwd,
            &["packages", "desktop", "bundled-resources"],
        )),
        Some(resolve_path(
            &context.cwd,
            &["..", "desktop", "bundled-resources"],
        )),
    ];
    if let Some(module_dir) = &context.module_dir {
        roots.push(Some(resolve_path(
            module_dir,
            &["..", "..", "desktop", "bundled-resources"],
        )));
        roots.push(Some(resolve_path(
            module_dir,
            &["..", "..", "bundled-resources"],
        )));
    } else {
        roots.push(None);
        roots.push(None);
    }
    roots
}

fn find_resource(context: &AgentRuntimeContext<'_>, entry_segments: &[String]) -> Option<String> {
    let mut resource_segments: Vec<&str> = vec![AGENT_BUNDLED_RESOURCE_DIR];
    resource_segments.extend(entry_segments.iter().map(String::as_str));

    let mut candidates: Vec<Option<PathBuf>> = Vec::new();
    if let Some(resources_path) = &context.resources_path {
        // Tauri 发运集把 `ship/agent` 映射成 `<resources>/agent`，因此随包二进制
        // 实际在 `<resources>/agent/glm/...`；同时保留旧的 `<resources>/glm/...` 口径。
        let mut packaged_segments: Vec<&str> = vec!["agent"];
        packaged_segments.extend(resource_segments.iter().copied());
        candidates.push(Some(resolve_path(
            Path::new(resources_path),
            &packaged_segments,
        )));
        candidates.push(Some(resolve_path(
            Path::new(resources_path),
            &resource_segments,
        )));
    } else {
        candidates.push(None);
    }
    let mut home_segments = vec![MODE_DATA_ROOT_DIR_NAME, "server", "agents"];
    home_segments.extend(resource_segments.iter());
    candidates.push(Some(resolve_path(&context.home_dir, &home_segments)));
    for root in platform_scoped_bundled_agent_roots(context) {
        match root {
            Some(root) => {
                let mut root_segments: Vec<&str> = vec![AGENT_BUNDLED_RESOURCE_DIR];
                root_segments.extend(entry_segments.iter().map(String::as_str));
                candidates.push(Some(resolve_path(&root, &root_segments)));
            }
            None => candidates.push(None),
        }
    }
    for root in legacy_bundled_resource_roots(context) {
        match root {
            Some(root) => candidates.push(Some(resolve_path(&root, &resource_segments))),
            None => candidates.push(None),
        }
    }
    resolve_existing_path(&candidates, context.fs)
}

fn binary_entry_segments(platform: &str) -> Vec<String> {
    let mut name = AGENT_BINARY_NAME.to_string();
    if matches!(platform_token(platform), "win32") {
        name.push_str(".exe");
    }
    vec![name]
}

pub fn find_mode_agent_runtime_binary(context: &AgentRuntimeContext<'_>) -> Option<String> {
    let env_path = context.env.get(AGENT_BINARY_ENV_VAR);
    if let Some(env_path) = env_path.filter(|value| !value.is_empty()) {
        if context.fs.exists(Path::new(env_path)) {
            return Some(env_path.clone());
        }
    }
    find_resource(context, &binary_entry_segments(&context.platform))
}

pub fn find_mode_agent_runtime_node_bundle(context: &AgentRuntimeContext<'_>) -> Option<String> {
    find_resource(context, &[AGENT_NODE_BUNDLE_ENTRY.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeFs(Vec<PathBuf>);

    impl ToolFileSystem for FakeFs {
        fn exists(&self, path: &Path) -> bool {
            self.0.iter().any(|candidate| candidate == path)
        }

        fn is_executable(&self, _path: &Path) -> bool {
            true
        }
    }

    /// Rust 常量是 `windows`/`x86_64`，随包目录是 `win32-x64` 且二进制带 `.exe`。
    #[test]
    fn rust_os_tokens_resolve_the_node_bundled_layout() {
        let cwd = PathBuf::from("/ws");
        let expected = resolve_path(
            &cwd,
            &["bundled-agents", "win32-x64", "glm", "mode-agent.exe"],
        );
        let fs = FakeFs(vec![expected.clone()]);
        let context = AgentRuntimeContext {
            platform: "windows".to_string(),
            arch: "x86_64".to_string(),
            cwd,
            module_dir: None,
            resources_path: None,
            home_dir: PathBuf::from("/home"),
            env: EnvMap::default(),
            fs: &fs,
        };
        assert_eq!(
            find_mode_agent_runtime_binary(&context),
            Some(expected.to_string_lossy().into_owned())
        );
    }
}
