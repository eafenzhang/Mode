//! Agent 运行时装配（S0 健康面 / S2 翻面的壳侧唯一所有者）。
//!
//! spec：`docs/specs/host-ownership-switch.md` §5。S0 只接健康只读面：
//! registry 的工厂在这里装配完整启动规格（env 覆盖 → dev dist/源码 → 打包
//! `mode.cjs` → 已部署 native binary 兜底，见 `agent-process::resolve_agent_command`
//! 与 `docs/specs/agent-process.md`），但只有 `agent-health` 频道消费它——
//! `list`/`describe` 都是只读，不会触发 spawn。S2 翻面时 `mode-agent` 频道
//! 复用同一个 registry，不另建第二所有者。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    resolve_agent_command, AgentControlClient, AgentProcessError, AgentResolveContext,
    AgentRuntimeRegistry, SpecFactory,
};
use runtime_env::EnvMap;
use runtime_tools::{find_mode_agent_runtime_binary, AgentRuntimeContext, NativeToolFileSystem};

/// 壳侧 Agent 运行时配置（与 Node Host 同口径的资源定位输入）。
pub struct AgentRuntimeConfig {
    pub resources_dir: Option<PathBuf>,
    /// 数据根（workspace cwd 失效时的回落，与 Node Host 同树）。
    pub data_root: PathBuf,
}

fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn node_candidates(resources_dir: &Option<PathBuf>) -> Vec<PathBuf> {
    let binary = if cfg!(windows) { "node.exe" } else { "node" };
    match resources_dir {
        Some(dir) => vec![dir.join("node").join(binary)],
        None => Vec::new(),
    }
}

/// 装配 registry + 只读控制面（S0）。工厂完整实现启动规格解析，
/// 但 S0 阶段只有 `agent-health` 消费它（只读、不 spawn）。
pub fn build_control_client(config: &AgentRuntimeConfig) -> Arc<AgentControlClient> {
    let resources_dir = config.resources_dir.clone();
    let data_root = config.data_root.clone();
    let factory: SpecFactory = Arc::new(move |target| {
        let workspace_path = PathBuf::from(&target.workspace_path);
        let env: BTreeMap<String, String> = std::env::vars().collect();
        let env_map: EnvMap = env.clone().into_iter().collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| data_root.clone());
        let home = home_dir();
        let native_binary = find_mode_agent_runtime_binary(&AgentRuntimeContext {
            platform: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cwd: cwd.clone(),
            module_dir: None,
            resources_path: resources_dir
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            home_dir: home,
            env: env_map,
            fs: &NativeToolFileSystem,
        });
        let context = AgentResolveContext {
            workspace_path: workspace_path.clone(),
            env,
            // TS 的 findUpward 从宿主进程 cwd 起步；开发树在 cwd 之下才可见。
            upward_start: cwd,
            resources_dir: resources_dir.clone(),
            node_candidates: node_candidates(&resources_dir),
            node_fallback: "node".into(),
            deployed_native_binary: native_binary,
            presentation_surface: Some("desktop".into()),
            request_timeout: Duration::from_secs(180),
        };
        let resolved =
            resolve_agent_command(&context).map_err(|error| AgentProcessError::Spawn {
                command: String::new(),
                message: error.to_string(),
            })?;
        Ok(resolved.spec)
    });
    Arc::new(AgentControlClient::new(Arc::new(
        AgentRuntimeRegistry::new(factory),
    )))
}
