//! Agent 启动规格解析（所有权切换的配置层）。
//!
//! 顺序与 TS `resolveDefaultModeAgentCommand` 对齐：
//! 1. `MODE_AGENT_SERVER_COMMAND` 显式覆盖（args 取 `MODE_AGENT_SERVER_ARGS_JSON`，
//!    cwd 取 `MODE_AGENT_SERVER_CWD`，缺省为 workspace）；
//! 2. monorepo dev dist：向上找 `apps/mode-cli/packages/cli/dist/mode.cjs`；
//! 3. monorepo dev 源码：`apps/mode-cli/packages/cli/src/main.ts` + `node_modules/.bin/tsx`；
//! 4. 打包资源：`<resources>/agent/glm/mode.cjs`（Node 解释执行）。
//!
//! Electron runtime 与已部署 native binary 分支暂未移植（Tauri 壳没有 Electron；native
//! binary 的候选链依赖远端部署，随所有权切换一起补）。`--surface desktop` 的参数改写
//! 与 TS 一致。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::AgentProcessSpec;

const DEFAULT_ARGS: [&str; 2] = ["app-server", "--stdio"];
const DEV_DIST_ENTRY: &str = "apps/mode-cli/packages/cli/dist/mode.cjs";
const DEV_SOURCE_ENTRY: &str = "apps/mode-cli/packages/cli/src/main.ts";
const DEV_TSX_BIN: &str = "node_modules/.bin/tsx";
const DEV_TSX_BIN_WINDOWS: &str = "node_modules/.bin/tsx.cmd";
const PACKAGED_ENTRY: &str = "agent/glm/mode.cjs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCommandSource {
    Env,
    DevDist,
    DevSource,
    Packaged,
}

#[derive(Debug, Clone)]
pub struct AgentResolveContext {
    pub workspace_path: PathBuf,
    pub env: BTreeMap<String, String>,
    /// `findUpward` 的起点（通常是壳进程 cwd）。
    pub upward_start: PathBuf,
    /// 打包资源目录（`<resource_dir>`）。
    pub resources_dir: Option<PathBuf>,
    /// 随包 Node 候选（如 `<resources>/node/node.exe`），按顺序取第一个存在的文件。
    pub node_candidates: Vec<PathBuf>,
    /// 没有随包 Node 时的 PATH 命令。
    pub node_fallback: String,
    /// `desktop` 时追加 `--surface desktop`；其它值不修改参数。
    pub presentation_surface: Option<String>,
    pub request_timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct ResolvedAgentCommand {
    pub spec: AgentProcessSpec,
    pub storage_preparation_entry: Option<PathBuf>,
    pub source: AgentCommandSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentResolveError {
    ArgsJson(String),
    NotConfigured { workspace_path: String },
}

impl std::fmt::Display for AgentResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArgsJson(message) => write!(formatter, "{message}"),
            Self::NotConfigured { workspace_path } => write!(
                formatter,
                "Mode agent server command is not configured (workspace {workspace_path})"
            ),
        }
    }
}

impl std::error::Error for AgentResolveError {}

pub fn resolve_agent_command(
    context: &AgentResolveContext,
) -> Result<ResolvedAgentCommand, AgentResolveError> {
    if let Some(command) = env_get(&context.env, "MODE_AGENT_SERVER_COMMAND")
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let args = parse_args_json(env_get(&context.env, "MODE_AGENT_SERVER_ARGS_JSON"))?
            .unwrap_or_else(|| DEFAULT_ARGS.iter().map(|arg| arg.to_string()).collect());
        let cwd = env_get(&context.env, "MODE_AGENT_SERVER_CWD")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| context.workspace_path.clone());
        return Ok(finish(
            context,
            command.to_string(),
            args,
            cwd,
            None,
            AgentCommandSource::Env,
        ));
    }

    if let Some(entry) = find_upward(&context.upward_start, DEV_DIST_ENTRY) {
        return Ok(finish(
            context,
            node_command(context),
            vec![
                entry.to_string_lossy().into_owned(),
                DEFAULT_ARGS[0].into(),
                DEFAULT_ARGS[1].into(),
            ],
            context.workspace_path.clone(),
            Some(entry),
            AgentCommandSource::DevDist,
        ));
    }

    if let Some(main) = find_upward(&context.upward_start, DEV_SOURCE_ENTRY) {
        let tsx = tsx_candidates()
            .iter()
            .find_map(|candidate| find_upward(&context.upward_start, candidate));
        if let Some(tsx) = tsx {
            return Ok(finish(
                context,
                tsx.to_string_lossy().into_owned(),
                vec![
                    main.to_string_lossy().into_owned(),
                    DEFAULT_ARGS[0].into(),
                    DEFAULT_ARGS[1].into(),
                ],
                context.workspace_path.clone(),
                None,
                AgentCommandSource::DevSource,
            ));
        }
    }

    if let Some(resources) = &context.resources_dir {
        let entry = resources.join(PACKAGED_ENTRY);
        if entry.is_file() {
            return Ok(finish(
                context,
                node_command(context),
                vec![
                    entry.to_string_lossy().into_owned(),
                    DEFAULT_ARGS[0].into(),
                    DEFAULT_ARGS[1].into(),
                ],
                context.workspace_path.clone(),
                Some(entry),
                AgentCommandSource::Packaged,
            ));
        }
    }

    Err(AgentResolveError::NotConfigured {
        workspace_path: context.workspace_path.to_string_lossy().into_owned(),
    })
}

fn finish(
    context: &AgentResolveContext,
    command: String,
    args: Vec<String>,
    requested_cwd: PathBuf,
    storage_preparation_entry: Option<PathBuf>,
    source: AgentCommandSource,
) -> ResolvedAgentCommand {
    let args = apply_presentation_surface(args, context.presentation_surface.as_deref());
    ResolvedAgentCommand {
        spec: AgentProcessSpec {
            command,
            args,
            cwd: resolve_spawn_cwd(&requested_cwd, &context.workspace_path),
            env: Vec::new(),
            request_timeout: context.request_timeout,
            // env 显式命令不承诺上报 storage 状态；dev dist/源码与打包入口都会上报。
            supports_storage_startup: matches!(
                source,
                AgentCommandSource::DevDist
                    | AgentCommandSource::DevSource
                    | AgentCommandSource::Packaged
            ),
        },
        storage_preparation_entry,
        source,
    }
}

/// cwd 失效时回落到 workspace；再失效则由 spawn 继承壳进程 cwd（None）。
fn resolve_spawn_cwd(requested: &Path, workspace: &Path) -> Option<PathBuf> {
    if requested.is_dir() {
        return Some(requested.to_path_buf());
    }
    if workspace.is_dir() {
        return Some(workspace.to_path_buf());
    }
    None
}

fn node_command(context: &AgentResolveContext) -> String {
    context
        .node_candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
        .unwrap_or_else(|| context.node_fallback.clone())
}

fn tsx_candidates() -> [&'static str; 2] {
    if cfg!(windows) {
        [DEV_TSX_BIN_WINDOWS, DEV_TSX_BIN]
    } else {
        [DEV_TSX_BIN, DEV_TSX_BIN_WINDOWS]
    }
}

/// `--surface desktop` 改写：先移除已有 `--surface`/`--surface=`，再追加。
/// 只有明确非 option 的下一 token 才属于 `--surface`（与 TS 修复一致）。
fn apply_presentation_surface(args: Vec<String>, surface: Option<&str>) -> Vec<String> {
    if surface != Some("desktop") {
        return args;
    }
    let mut stripped = Vec::with_capacity(args.len() + 2);
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--surface" {
            if let Some(next) = args.get(index + 1) {
                if !next.starts_with('-') {
                    index += 1;
                }
            }
            index += 1;
            continue;
        }
        if arg.starts_with("--surface=") {
            index += 1;
            continue;
        }
        stripped.push(arg.clone());
        index += 1;
    }
    stripped.push("--surface".into());
    stripped.push("desktop".into());
    stripped
}

fn parse_args_json(raw: Option<&str>) -> Result<Option<Vec<String>>, AgentResolveError> {
    let Some(trimmed) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let parsed: serde_json::Value = serde_json::from_str(trimmed).map_err(|error| {
        AgentResolveError::ArgsJson(format!(
            "MODE_AGENT_SERVER_ARGS_JSON must be a JSON string array: {error}"
        ))
    })?;
    let Some(items) = parsed.as_array() else {
        return Err(AgentResolveError::ArgsJson(
            "MODE_AGENT_SERVER_ARGS_JSON must be a JSON string array".into(),
        ));
    };
    let mut args = Vec::with_capacity(items.len());
    for item in items {
        let Some(text) = item.as_str() else {
            return Err(AgentResolveError::ArgsJson(
                "MODE_AGENT_SERVER_ARGS_JSON must be a JSON string array".into(),
            ));
        };
        args.push(text.to_string());
    }
    Ok(Some(args))
}

fn env_get<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    if let Some(value) = env.get(key) {
        return Some(value.as_str());
    }
    // Windows 环境变量名大小写不敏感；在 Unix 上也宽容处理，避免配置因大小写失效。
    let lower = key.to_lowercase();
    env.iter()
        .find(|(candidate, _)| candidate.to_lowercase() == lower)
        .map(|(_, value)| value.as_str())
}

fn find_upward(start: &Path, relative: &str) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let candidate = current.join(relative);
        if candidate.exists() {
            return Some(candidate);
        }
        let parent = current.parent()?;
        if parent == current {
            return None;
        }
        current = parent.to_path_buf();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(workspace: &Path) -> AgentResolveContext {
        AgentResolveContext {
            workspace_path: workspace.to_path_buf(),
            env: BTreeMap::new(),
            upward_start: workspace.to_path_buf(),
            resources_dir: None,
            node_candidates: Vec::new(),
            node_fallback: "node".into(),
            presentation_surface: Some("desktop".into()),
            request_timeout: Duration::from_secs(60),
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mode-agent-resolve-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn env_override_wins_and_parses_args_and_cwd() {
        let workspace = temp_dir("env");
        std::fs::create_dir_all(workspace.join("sub")).unwrap();
        let mut ctx = context(&workspace);
        ctx.env.insert("MODE_AGENT_SERVER_COMMAND".into(), " C:/node.exe ".into());
        ctx.env
            .insert("MODE_AGENT_SERVER_ARGS_JSON".into(), r#"["x","--flag"]"#.into());
        ctx.env.insert(
            "MODE_AGENT_SERVER_CWD".into(),
            workspace.join("sub").to_string_lossy().into_owned(),
        );

        let resolved = resolve_agent_command(&ctx).unwrap();
        assert_eq!(resolved.source, AgentCommandSource::Env);
        assert_eq!(resolved.spec.command, "C:/node.exe");
        assert_eq!(resolved.spec.args, vec!["x", "--flag", "--surface", "desktop"]);
        assert_eq!(resolved.spec.cwd, Some(workspace.join("sub")));
        assert!(resolved.storage_preparation_entry.is_none());
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn invalid_args_json_is_an_error() {
        let workspace = temp_dir("args");
        let mut ctx = context(&workspace);
        ctx.env.insert("MODE_AGENT_SERVER_COMMAND".into(), "node".into());
        ctx.env
            .insert("MODE_AGENT_SERVER_ARGS_JSON".into(), "[1,2]".into());
        assert!(matches!(
            resolve_agent_command(&ctx),
            Err(AgentResolveError::ArgsJson(_))
        ));
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn dev_dist_found_upward_uses_node_and_workspace() {
        let workspace = temp_dir("devdist");
        let nested = workspace.join("packages/app");
        std::fs::create_dir_all(&nested).unwrap();
        let entry = workspace.join(DEV_DIST_ENTRY);
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "").unwrap();
        let node = workspace.join("node/node.exe");
        std::fs::create_dir_all(node.parent().unwrap()).unwrap();
        std::fs::write(&node, "").unwrap();

        let mut ctx = context(&workspace);
        ctx.upward_start = nested;
        ctx.node_candidates = vec![node.clone()];
        let resolved = resolve_agent_command(&ctx).unwrap();
        assert_eq!(resolved.source, AgentCommandSource::DevDist);
        assert_eq!(resolved.spec.command, node.to_string_lossy());
        assert_eq!(
            resolved.spec.args,
            vec![
                entry.to_string_lossy().into_owned(),
                "app-server".into(),
                "--stdio".into(),
                "--surface".into(),
                "desktop".into()
            ]
        );
        assert_eq!(resolved.storage_preparation_entry.as_deref(), Some(entry.as_path()));
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn packaged_entry_is_used_when_dev_tree_is_absent() {
        let workspace = temp_dir("packaged");
        let resources = temp_dir("packaged-res");
        let entry = resources.join(PACKAGED_ENTRY);
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "").unwrap();
        let node = resources.join("node/node.exe");
        std::fs::create_dir_all(node.parent().unwrap()).unwrap();
        std::fs::write(&node, "").unwrap();

        let mut ctx = context(&workspace);
        ctx.resources_dir = Some(resources.clone());
        ctx.node_candidates = vec![node.clone()];
        let resolved = resolve_agent_command(&ctx).unwrap();
        assert_eq!(resolved.source, AgentCommandSource::Packaged);
        assert_eq!(resolved.spec.command, node.to_string_lossy());
        assert_eq!(resolved.storage_preparation_entry.as_deref(), Some(entry.as_path()));
        std::fs::remove_dir_all(&workspace).ok();
        std::fs::remove_dir_all(&resources).ok();
    }

    #[test]
    fn missing_everything_is_not_configured() {
        let workspace = temp_dir("missing");
        let ctx = context(&workspace);
        assert!(matches!(
            resolve_agent_command(&ctx),
            Err(AgentResolveError::NotConfigured { .. })
        ));
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn surface_rewrite_only_for_desktop_and_strips_existing() {
        assert_eq!(
            apply_presentation_surface(
                vec!["a".into(), "--surface".into(), "remote".into(), "b".into()],
                Some("desktop")
            ),
            vec!["a", "b", "--surface", "desktop"]
        );
        assert_eq!(
            apply_presentation_surface(
                vec!["--surface=remote".into(), "--stdio".into()],
                Some("desktop")
            ),
            vec!["--stdio", "--surface", "desktop"]
        );
        assert_eq!(
            apply_presentation_surface(vec!["--stdio".into()], Some("remote")),
            vec!["--stdio"]
        );
    }
}
