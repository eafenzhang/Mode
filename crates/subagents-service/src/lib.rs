//! Subagent 管理服务（Rust 实现）。
//!
//! 真源：`packages/services/src/subagents/subagentsService.ts`（clean-room：只读本仓库 TS）。
//! 行为契约与已知差异见 `docs/specs/subagents-service.md`。
//!
//! 覆盖八个方法：`list` / `set_enabled` / `set_built_in_model_override` /
//! `set_plugin_agent_model_override` / `get_primary_user_agents_directory` /
//! `create_agent` / `update_agent` / `delete_agent`，以及 Host 启动前的一次性
//! `prepare_runtime_state`（不暴露为 Renderer RPC）。

mod discovery;
mod markdown;
mod model;
mod state;
mod storage;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;

pub use markdown::{AgentConfig, AgentDiagnostic, AgentSummary};
pub use model::{Selection, SelectionOptions};
pub use storage::StorageOptions;

use crate::discovery::{
    apply_runtime_precedence, attach_enabled_state, create_built_in_agents, discover_file_agents,
    discover_plugin_agents, FileDiscovery,
};
use crate::markdown::{parse_agent_markdown, serialize_agent_markdown};
use crate::model::normalize_selection;
use crate::state::{
    migrate_user_subagent_markdown, read_agent_state, write_agent_state, AgentsState,
};
use crate::storage::{resolve_user_subagent_root, resolve_workspace_subagent_root};

#[derive(Debug, Clone)]
pub struct SubagentsServiceOptions {
    pub storage: StorageOptions,
    /// `isDesktopRuntime`：决定用户级 agent 是否可用。
    pub is_desktop_runtime: Option<bool>,
}

impl SubagentsServiceOptions {
    pub fn from_process() -> Self {
        Self {
            storage: StorageOptions::from_process(),
            is_desktop_runtime: None,
        }
    }

    pub fn with_home(home_dir: PathBuf, is_desktop_runtime: bool) -> Self {
        Self {
            storage: StorageOptions::with_home(home_dir),
            is_desktop_runtime: Some(is_desktop_runtime),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentsError {
    pub message: String,
}

impl fmt::Display for SubagentsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SubagentsError {}

impl SubagentsError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<std::io::Error> for SubagentsError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListMode {
    AllRuntimeScopes,
    SettingsUserOnly,
}

#[derive(Debug, Clone)]
pub struct ListParams {
    pub workspace_path: PathBuf,
    pub workspace_identity: Option<String>,
    pub mode: Option<ListMode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsCapability {
    pub user_scope_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_scope_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentsListResult {
    pub agents: Vec<Value>,
    pub user_agents: Vec<Value>,
    pub plugin_agents: Vec<Value>,
    pub capability: AgentsCapability,
    pub diagnostics: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub agent: AgentSummary,
}

impl AgentResult {
    pub fn to_json(&self) -> Value {
        serde_json::json!({ "agent": self.agent.to_json() })
    }
}

#[derive(Debug, Clone, Default)]
pub struct CreateAgentParams {
    pub config: AgentConfig,
    pub scope: Option<String>,
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateAgentParams {
    pub agent_id: String,
    pub config: AgentConfig,
    pub old_file_path: Option<String>,
    pub scope: Option<String>,
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeleteAgentParams {
    pub agent_id: String,
    pub file_path: String,
}

pub struct SubagentsService {
    options: SubagentsServiceOptions,
    /// TS 用 promise 链串行化所有写操作；这里用互斥锁复刻同一语义。
    write_lock: Mutex<()>,
}

impl SubagentsService {
    pub fn new(options: SubagentsServiceOptions) -> Self {
        Self {
            options,
            write_lock: Mutex::new(()),
        }
    }

    pub fn user_scope_available(&self) -> bool {
        match self.options.is_desktop_runtime {
            Some(value) => value,
            None => std::env::var("MODE_PROCESS_LABEL")
                .map(|value| !value.is_empty())
                .unwrap_or(false),
        }
    }

    fn capability(&self) -> AgentsCapability {
        if self.user_scope_available() {
            AgentsCapability {
                user_scope_available: true,
                user_scope_reason: None,
            }
        } else {
            AgentsCapability {
                user_scope_available: false,
                user_scope_reason: Some("desktop_only".into()),
            }
        }
    }

    /// Host 启动 Agent 前的一次性存储导入，不暴露为 Renderer RPC。
    pub fn prepare_runtime_state(&self) {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _failures =
            migrate_user_subagent_markdown(&resolve_user_subagent_root(&self.options.storage));
        let path = crate::storage::resolve_subagent_state_file(&self.options.storage);
        crate::state::best_effort_migrate_state_file(&path);
    }

    pub fn list(&self, params: &ListParams) -> AgentsListResult {
        let capability = self.capability();
        let mode = params.mode.unwrap_or(ListMode::AllRuntimeScopes);
        let mut diagnostics: Vec<AgentDiagnostic> = Vec::new();
        if capability.user_scope_available {
            let failures =
                migrate_user_subagent_markdown(&resolve_user_subagent_root(&self.options.storage));
            for path in failures {
                diagnostics.push(AgentDiagnostic {
                    code: "agent_read_failed".into(),
                    message: "Subagent Markdown migration failed; original file preserved".into(),
                    path: path.to_string_lossy().into_owned(),
                });
            }
        }
        let state = read_agent_state(&self.options.storage);
        let built_in_agents = create_built_in_agents(&state.built_in_overrides);
        let file_agents = discover_file_agents(FileDiscovery {
            diagnostics: &mut diagnostics,
            include_user_agents: capability.user_scope_available,
            include_workspace_agents: mode == ListMode::AllRuntimeScopes,
            storage: &self.options.storage,
            workspace_path: &params.workspace_path,
        });
        let mut user_agents: Vec<AgentSummary> = file_agents
            .iter()
            .filter(|agent| agent.scope == "user")
            .cloned()
            .collect();
        let mut workspace_agents: Vec<AgentSummary> = file_agents
            .iter()
            .filter(|agent| agent.scope == "workspace")
            .cloned()
            .collect();
        sort_by_name(&mut user_agents);
        sort_by_name(&mut workspace_agents);

        let reserved_names: Vec<String> = built_in_agents
            .iter()
            .map(|agent| agent.name.clone())
            .chain(user_agents.iter().map(|agent| agent.name.clone()))
            .chain(workspace_agents.iter().map(|agent| agent.name.clone()))
            .collect();
        let plugin_discovery = discover_plugin_agents(
            &mut diagnostics,
            &state,
            &reserved_names,
            &self.options.storage,
        );

        let discovered = if mode == ListMode::SettingsUserOnly {
            let mut agents = built_in_agents.clone();
            agents.extend(user_agents.clone());
            agents
        } else {
            let mut agents = built_in_agents.clone();
            agents.extend(user_agents.clone());
            agents.extend(workspace_agents.clone());
            agents.extend(plugin_discovery.runtime_agents.clone());
            apply_runtime_precedence(agents)
        };
        let agents = attach_enabled_state(discovered, &state);
        let user_agents = agents
            .iter()
            .filter(|agent| agent.source == "user")
            .cloned()
            .collect::<Vec<_>>();
        let plugin_agents = attach_enabled_state(plugin_discovery.profiles, &state);

        AgentsListResult {
            agents: agents.iter().map(AgentSummary::to_json).collect(),
            user_agents: user_agents.iter().map(AgentSummary::to_json).collect(),
            plugin_agents: plugin_agents.iter().map(AgentSummary::to_json).collect(),
            capability,
            diagnostics: diagnostics.iter().map(AgentDiagnostic::to_json).collect(),
        }
    }

    pub fn set_enabled(&self, agent_id: &str, enabled: bool) -> Result<(), SubagentsError> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = read_agent_state(&self.options.storage);
        let mut disabled: Vec<String> = state
            .disabled_ids
            .iter()
            .filter(|id| is_not(id, agent_id))
            .cloned()
            .collect();
        if !enabled {
            disabled.push(agent_id.to_string());
        }
        disabled.sort();
        state.disabled_ids = disabled;
        write_agent_state(&self.options.storage, &state)?;
        Ok(())
    }

    pub fn set_built_in_model_override(
        &self,
        agent_name: &str,
        selection: Option<Selection>,
    ) -> Result<(), SubagentsError> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = read_agent_state(&self.options.storage);
        let selection = normalize_selection(selection).map_err(SubagentsError::new)?;
        match selection {
            Some(selection) => {
                state
                    .built_in_overrides
                    .insert(agent_name.to_string(), selection);
            }
            None => {
                state.built_in_overrides.remove(agent_name);
            }
        }
        write_agent_state(&self.options.storage, &state)?;
        Ok(())
    }

    pub fn set_plugin_agent_model_override(
        &self,
        agent_id: &str,
        selection: Option<Selection>,
    ) -> Result<(), SubagentsError> {
        if !agent_id.starts_with("plugin:") || agent_id.trim() != agent_id {
            return Err(SubagentsError::new("无效插件 Subagent 身份"));
        }
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = read_agent_state(&self.options.storage);
        let selection = normalize_selection(selection).map_err(SubagentsError::new)?;
        match selection {
            Some(selection) => {
                state
                    .plugin_overrides
                    .insert(agent_id.to_string(), selection);
            }
            None => {
                state.plugin_overrides.remove(agent_id);
            }
        }
        write_agent_state(&self.options.storage, &state)?;
        Ok(())
    }

    pub fn get_primary_user_agents_directory(&self) -> Result<PathBuf, SubagentsError> {
        let path = resolve_user_subagent_root(&self.options.storage);
        std::fs::create_dir_all(&path)?;
        Ok(path)
    }

    pub fn create_agent(&self, params: &CreateAgentParams) -> Result<AgentResult, SubagentsError> {
        validate_user_agent_config(&params.config)?;
        assert_not_built_in_name(&params.config.name)?;
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let scope = params.scope.as_deref().unwrap_or("user");
        let agent_dir = self.agent_dir(scope, params.workspace_path.as_deref())?;
        let file_path = agent_dir.join(format!("{}.md", params.config.name.trim().to_lowercase()));
        std::fs::create_dir_all(&agent_dir)?;
        if file_path.exists() {
            return Err(exists_error(&file_path));
        }
        let config = normalize_config(&params.config)?;
        let content = serialize_agent_markdown(&config);
        let agent = parse_saved_agent(
            &content,
            &file_path,
            scope,
            params.workspace_path.as_deref(),
        )?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options.open(&file_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                exists_error(&file_path)
            } else {
                SubagentsError::from(error)
            }
        })?;
        use std::io::Write;
        file.write_all(content.as_bytes())?;
        Ok(AgentResult { agent })
    }

    pub fn update_agent(&self, params: &UpdateAgentParams) -> Result<AgentResult, SubagentsError> {
        validate_user_agent_config(&params.config)?;
        assert_not_built_in_name(&params.config.name)?;
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let scope = params.scope.as_deref().unwrap_or("user");
        let agent_dir = self.agent_dir(scope, params.workspace_path.as_deref())?;
        let file_path = agent_dir.join(format!("{}.md", params.config.name.trim().to_lowercase()));
        std::fs::create_dir_all(&agent_dir)?;
        if let Some(old_file_path) = &params.old_file_path {
            if old_file_path != &file_path.to_string_lossy() && file_path.exists() {
                return Err(exists_error(&file_path));
            }
        }
        let config = normalize_config(&params.config)?;
        let content = serialize_agent_markdown(&config);
        let agent = parse_saved_agent(
            &content,
            &file_path,
            scope,
            params.workspace_path.as_deref(),
        )?;
        std::fs::write(&file_path, content.as_bytes())?;
        migrate_disabled_agent_id(&self.options.storage, &params.agent_id, &agent.id)?;
        if let Some(old_file_path) = &params.old_file_path {
            if old_file_path != &file_path.to_string_lossy() {
                let _ = std::fs::remove_file(old_file_path);
            }
        }
        Ok(AgentResult { agent })
    }

    pub fn delete_agent(&self, params: &DeleteAgentParams) -> Result<(), SubagentsError> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = std::fs::remove_file(&params.file_path);
        let mut state = read_agent_state(&self.options.storage);
        state.disabled_ids = state
            .disabled_ids
            .iter()
            .filter(|id| is_not(id, &params.agent_id))
            .cloned()
            .collect();
        state.disabled_ids.sort();
        write_agent_state(&self.options.storage, &state)?;
        Ok(())
    }

    fn agent_dir(
        &self,
        scope: &str,
        workspace_path: Option<&str>,
    ) -> Result<PathBuf, SubagentsError> {
        if scope == "workspace" {
            let workspace_path = workspace_path
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    SubagentsError::new("Workspace path is required for workspace subagents")
                })?;
            Ok(resolve_workspace_subagent_root(Path::new(workspace_path)))
        } else {
            Ok(resolve_user_subagent_root(&self.options.storage))
        }
    }
}

fn is_not(value: &str, other: &str) -> bool {
    value != other
}

fn sort_by_name(agents: &mut [AgentSummary]) {
    agents.sort_by(|left, right| plugin_discovery::js::locale_compare(&left.name, &right.name));
}

fn exists_error(path: &Path) -> SubagentsError {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    SubagentsError::new(format!("Agent file \"{name}\" already exists"))
}

fn validate_user_agent_config(config: &AgentConfig) -> Result<(), SubagentsError> {
    let trimmed_name = config.name.trim();
    let name_length = trimmed_name.encode_utf16().count();
    if !(3..=50).contains(&name_length) {
        return Err(SubagentsError::new(
            "Name must be between 3 and 50 characters",
        ));
    }
    if !trimmed_name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(SubagentsError::new(
            "Name can only contain letters, numbers, and hyphens",
        ));
    }
    if config.description.trim().is_empty() {
        return Err(SubagentsError::new("Description is required"));
    }
    if config.system_prompt.trim().is_empty() {
        return Err(SubagentsError::new("System prompt is required"));
    }
    Ok(())
}

fn assert_not_built_in_name(name: &str) -> Result<(), SubagentsError> {
    let trimmed = name.trim();
    if matches!(trimmed, "general-purpose" | "Explore") {
        return Err(SubagentsError::new(format!(
            "Agent name \"{trimmed}\" is reserved by a built-in agent"
        )));
    }
    Ok(())
}

fn normalize_config(config: &AgentConfig) -> Result<AgentConfig, SubagentsError> {
    let selection =
        normalize_selection(config.model_selection.clone()).map_err(SubagentsError::new)?;
    Ok(AgentConfig {
        name: config.name.trim().to_string(),
        description: config.description.trim().to_string(),
        system_prompt: config.system_prompt.trim().to_string(),
        model_selection: selection,
        ..config.clone()
    })
}

fn parse_saved_agent(
    content: &str,
    path: &Path,
    scope: &str,
    workspace_path: Option<&str>,
) -> Result<AgentSummary, SubagentsError> {
    let path_text = path.to_string_lossy().into_owned();
    let parsed = parse_agent_markdown(content, &path_text, scope);
    if let Some(diagnostic) = parsed.diagnostic {
        return Err(SubagentsError::new(diagnostic.message));
    }
    let Some(mut agent) = parsed.agent else {
        return Err(SubagentsError::new(format!(
            "Failed to parse saved agent: {path_text}"
        )));
    };
    if scope == "workspace" {
        agent.project_path = workspace_path.map(str::to_string);
    }
    Ok(agent)
}

fn migrate_disabled_agent_id(
    storage: &StorageOptions,
    previous_agent_id: &str,
    next_agent_id: &str,
) -> Result<(), SubagentsError> {
    if previous_agent_id == next_agent_id {
        return Ok(());
    }
    let state = read_agent_state(storage);
    if !state.disabled_ids.iter().any(|id| id == previous_agent_id) {
        return Ok(());
    }
    let mut disabled: Vec<String> = state
        .disabled_ids
        .iter()
        .filter(|id| id.as_str() != previous_agent_id)
        .cloned()
        .collect();
    disabled.push(next_agent_id.to_string());
    disabled.sort();
    let next = AgentsState {
        disabled_ids: disabled,
        ..state
    };
    write_agent_state(storage, &next)?;
    Ok(())
}
