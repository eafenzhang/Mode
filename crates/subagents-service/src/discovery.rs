//! Subagent 发现：内置、用户/工作区 Markdown、插件 agent。
//!
//! 真源：`subagentsService.ts` 的 `createBuiltInAgents` / `discoverFileAgents` /
//! `collectAgentMarkdownPaths` / `discoverPluginAgents` / `readEnabledPluginRecords` /
//! `collectPluginAgentRoots` / `applyPluginAgentOverrides` / `applyRuntimePrecedence`。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::markdown::{parse_agent_markdown, AgentDiagnostic, AgentSummary};
use crate::model::Selection;
use crate::state::AgentsState;
use crate::storage::{
    resolve_mode_storage_root, resolve_user_subagent_root, resolve_workspace_subagent_root,
    StorageOptions,
};
use plugin_discovery::js;
use plugin_discovery::{
    canonical_plugin_id, is_default_enabled_official_plugin_id, read_installed_plugin_records,
    read_plugin_config, read_plugin_manifest_object, scan_official_plugin_cache_roots,
    MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID,
};

const BUILT_IN_AGENT_NAMES: [&str; 2] = ["general-purpose", "Explore"];

pub fn create_built_in_agents(overrides: &BTreeMap<String, Selection>) -> Vec<AgentSummary> {
    let general_purpose = overrides.get("general-purpose").cloned();
    let explore = overrides.get("Explore").cloned();
    vec![
        AgentSummary {
            id: "built-in:built-in:general-purpose".into(),
            name: "general-purpose".into(),
            description: "General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks.".into(),
            system_prompt: String::new(),
            color: Some("blue".into()),
            model_selection: general_purpose.clone(),
            default_model_selection: None,
            model_selection_override: general_purpose,
            tools: Some(vec!["*".into()]),
            disallowed_tools: None,
            inject_agents_md: Some(true),
            skills: None,
            permission_mode: None,
            max_turns: None,
            background: None,
            mcp_servers: None,
            path: "built-in:general-purpose".into(),
            scope: "built-in".into(),
            source: "built-in".into(),
            enabled: true,
            read_only: Some(true),
            project_path: None,
            plugin_id: None,
            plugin_name: None,
            diagnostics: None,
        },
        AgentSummary {
            id: "built-in:built-in:explore".into(),
            name: "Explore".into(),
            description: "Read-only search agent for broad fan-out searches.".into(),
            system_prompt: String::new(),
            color: Some("cyan".into()),
            model_selection: explore.clone(),
            default_model_selection: None,
            model_selection_override: explore,
            tools: Some(
                [
                    "Bash",
                    "Glob",
                    "Grep",
                    "Read",
                    "WebFetch",
                    "WebSearch",
                    "TodoWrite",
                ]
                .iter()
                .map(|value| value.to_string())
                .collect(),
            ),
            disallowed_tools: None,
            inject_agents_md: Some(false),
            skills: None,
            permission_mode: None,
            max_turns: None,
            background: None,
            mcp_servers: None,
            path: "built-in:Explore".into(),
            scope: "built-in".into(),
            source: "built-in".into(),
            enabled: true,
            read_only: Some(true),
            project_path: None,
            plugin_id: None,
            plugin_name: None,
            diagnostics: None,
        },
    ]
}

pub fn collect_agent_markdown_paths(root_path: &Path) -> Vec<PathBuf> {
    if !js::exists(root_path) {
        return Vec::new();
    }
    let Ok(metadata) = std::fs::symlink_metadata(root_path) else {
        return Vec::new();
    };
    if !metadata.is_dir() {
        return Vec::new();
    }
    let mut result = Vec::new();
    collect_recursive(root_path, &mut result);
    result.sort_by(|left, right| {
        js::locale_compare(&left.to_string_lossy(), &right.to_string_lossy())
    });
    result
}

fn collect_recursive(directory: &Path, result: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_recursive(&path, result);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.ends_with(".md") || name.ends_with(".markdown") {
            result.push(path);
        }
    }
}

pub struct FileDiscovery<'a> {
    pub diagnostics: &'a mut Vec<AgentDiagnostic>,
    pub include_user_agents: bool,
    pub include_workspace_agents: bool,
    pub storage: &'a StorageOptions,
    pub workspace_path: &'a Path,
}

pub fn discover_file_agents(params: FileDiscovery<'_>) -> Vec<AgentSummary> {
    let mut roots: Vec<(&str, PathBuf)> = Vec::new();
    if params.include_user_agents {
        roots.push(("user", resolve_user_subagent_root(params.storage)));
    }
    if params.include_workspace_agents {
        roots.push((
            "workspace",
            resolve_workspace_subagent_root(params.workspace_path),
        ));
    }

    let mut agents = Vec::new();
    for (scope, root_path) in roots {
        for agent_path in collect_agent_markdown_paths(&root_path) {
            let Some(markdown) = js::read_text(&agent_path) else {
                params.diagnostics.push(AgentDiagnostic {
                    code: "agent_read_failed".into(),
                    message: format!(
                        "Failed to read agent Markdown: {}",
                        agent_path.to_string_lossy()
                    ),
                    path: agent_path.to_string_lossy().into_owned(),
                });
                continue;
            };
            let path_text = agent_path.to_string_lossy().into_owned();
            let parsed = parse_agent_markdown(&markdown, &path_text, scope);
            if let Some(diagnostic) = parsed.diagnostic {
                params.diagnostics.push(diagnostic);
                continue;
            }
            if let Some(mut agent) = parsed.agent {
                if scope == "workspace" {
                    agent.project_path = Some(params.workspace_path.to_string_lossy().into_owned());
                }
                agents.push(agent);
            }
        }
    }
    agents
}

pub struct PluginAgentDiscovery {
    pub profiles: Vec<AgentSummary>,
    pub runtime_agents: Vec<AgentSummary>,
}

struct InstalledRecord {
    id: String,
    install_path: PathBuf,
    name: String,
    scope: Option<String>,
    project_path: Option<String>,
}

pub fn discover_plugin_agents(
    diagnostics: &mut Vec<AgentDiagnostic>,
    state: &AgentsState,
    reserved_names: &[String],
    storage: &StorageOptions,
) -> PluginAgentDiscovery {
    let storage_root = resolve_mode_storage_root(storage);
    let cli_root = if storage_root
        .file_name()
        .map(|name| name == "cli")
        .unwrap_or(false)
    {
        storage_root
    } else {
        storage_root.join("cli")
    };
    let plugin_storage_root = cli_root.join("plugins");
    let config = read_plugin_config(&read_cli_config(storage));
    let records = read_enabled_plugin_records(&plugin_storage_root, &config);

    struct ParsedPluginAgent {
        agent: AgentSummary,
        bare_name: String,
        plugin_name: String,
    }
    let mut parsed_agents: Vec<ParsedPluginAgent> = Vec::new();

    for record in records {
        for agent_path in collect_plugin_agent_markdown_paths(&record.install_path) {
            let Some(markdown) = js::read_text(&agent_path) else {
                diagnostics.push(AgentDiagnostic {
                    code: "agent_read_failed".into(),
                    message: format!(
                        "Failed to read plugin agent Markdown: {}",
                        agent_path.to_string_lossy()
                    ),
                    path: agent_path.to_string_lossy().into_owned(),
                });
                continue;
            };
            let path_text = agent_path.to_string_lossy().into_owned();
            let scope = if record.scope.as_deref() == Some("workspace") {
                "workspace"
            } else {
                "user"
            };
            let parsed = parse_agent_markdown(&markdown, &path_text, scope);
            if let Some(diagnostic) = parsed.diagnostic {
                diagnostics.push(diagnostic);
                continue;
            }
            let Some(agent) = parsed.agent else {
                continue;
            };
            let id = format!("plugin:{}:{}", record.id, agent.name.trim().to_lowercase());
            let bare_name = agent.name.clone();
            let mut agent = AgentSummary {
                id: id.clone(),
                name: format!("{}:{}", record.name, agent.name),
                read_only: Some(true),
                source: "plugin".into(),
                plugin_id: Some(record.id.clone()),
                plugin_name: Some(record.name.clone()),
                project_path: if scope == "workspace" {
                    record.project_path.clone()
                } else {
                    None
                },
                ..agent
            };
            if let Some(override_selection) = state.plugin_overrides.get(&id) {
                agent.default_model_selection = agent.model_selection.clone();
                agent.model_selection = Some(override_selection.clone());
                agent.model_selection_override = Some(override_selection.clone());
            } else {
                agent.default_model_selection = agent.model_selection.clone();
            }
            parsed_agents.push(ParsedPluginAgent {
                agent,
                bare_name,
                plugin_name: record.name.clone(),
            });
        }
    }

    let reserved: BTreeSet<String> = BUILT_IN_AGENT_NAMES
        .iter()
        .map(|name| name.to_string())
        .chain(reserved_names.iter().cloned())
        .collect();
    let mut bare_name_counts: BTreeMap<String, usize> = BTreeMap::new();
    for parsed in &parsed_agents {
        *bare_name_counts
            .entry(parsed.bare_name.clone())
            .or_insert(0) += 1;
    }

    let mut profiles = Vec::new();
    let mut runtime_agents = Vec::new();
    for parsed in &parsed_agents {
        profiles.push(parsed.agent.clone());
        runtime_agents.push(parsed.agent.clone());
        let count = bare_name_counts
            .get(&parsed.bare_name)
            .copied()
            .unwrap_or(0);
        if count == 1 && !reserved.contains(&parsed.bare_name) {
            let mut alias = parsed.agent.clone();
            alias.id = format!(
                "plugin:{}:alias:{}",
                parsed.agent.id,
                parsed.bare_name.trim().to_lowercase()
            );
            alias.name = parsed.bare_name.clone();
            runtime_agents.push(alias);
        } else if reserved.contains(&parsed.bare_name) {
            diagnostics.push(AgentDiagnostic {
                code: "agent_ambiguous_name".into(),
                message: format!(
                    "Plugin agent bare name conflicts with an existing profile; use {}:{}",
                    parsed.plugin_name, parsed.bare_name
                ),
                path: parsed.agent.path.clone(),
            });
        } else if count > 1 {
            diagnostics.push(AgentDiagnostic {
                code: "agent_ambiguous_name".into(),
                message: format!(
                    "Plugin agent bare name is ambiguous; use {}:{}",
                    parsed.plugin_name, parsed.bare_name
                ),
                path: parsed.agent.path.clone(),
            });
        }
    }

    profiles.sort_by(|left, right| js::locale_compare(&left.name, &right.name));
    runtime_agents.sort_by(|left, right| js::locale_compare(&left.name, &right.name));
    PluginAgentDiscovery {
        profiles,
        runtime_agents,
    }
}

fn read_cli_config(storage: &StorageOptions) -> serde_json::Map<String, Value> {
    let path = storage
        .resolve_home()
        .join(".mode")
        .join("cli")
        .join("config.json");
    let Some(raw) = js::read_text(&path) else {
        return serde_json::Map::new();
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(object)) => object,
        _ => serde_json::Map::new(),
    }
}

fn read_enabled_plugin_records(
    plugin_storage_root: &Path,
    config: &plugin_discovery::PluginConfig,
) -> Vec<InstalledRecord> {
    let installed: Vec<InstalledRecord> = read_installed_plugin_records(plugin_storage_root)
        .into_iter()
        .filter_map(|record| {
            let name = record.name?;
            Some(InstalledRecord {
                id: canonical_plugin_id(&record.id),
                install_path: PathBuf::from(record.install_path),
                name,
                scope: record.scope,
                project_path: record.project_path,
            })
        })
        .collect();

    let suppressed = |id: &str| {
        id.ends_with(&format!("@{MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID}"))
            && config.suppressed_builtins.iter().any(|item| item == id)
    };

    let mut records: Vec<InstalledRecord> = installed
        .iter()
        .filter(|record| {
            config
                .enabled_plugins
                .get(&record.id)
                .copied()
                .unwrap_or(false)
                && !suppressed(&record.id)
        })
        .map(|record| InstalledRecord {
            id: record.id.clone(),
            install_path: record.install_path.clone(),
            name: record.name.clone(),
            scope: record.scope.clone(),
            project_path: record.project_path.clone(),
        })
        .collect();

    let mut seen_ids: BTreeSet<String> = installed.iter().map(|record| record.id.clone()).collect();
    for cache_root in scan_official_plugin_cache_roots(plugin_storage_root) {
        let id = format!("{}@{MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID}", cache_root.name);
        if seen_ids.contains(&id) || config.suppressed_builtins.iter().any(|item| item == &id) {
            continue;
        }
        let enabled = config
            .enabled_plugins
            .get(&id)
            .copied()
            .unwrap_or_else(|| is_default_enabled_official_plugin_id(&id));
        if !enabled {
            continue;
        }
        for version_root in cache_root.version_roots {
            let Some(manifest) = read_plugin_manifest_object(&version_root) else {
                continue;
            };
            let manifest_name = manifest.get("name").and_then(Value::as_str);
            if manifest_name != Some(cache_root.name.as_str()) {
                continue;
            }
            records.push(InstalledRecord {
                id: id.clone(),
                install_path: version_root,
                name: cache_root.name.clone(),
                scope: Some("user".into()),
                project_path: None,
            });
            seen_ids.insert(id.clone());
            break;
        }
    }
    records
}

fn collect_plugin_agent_markdown_paths(root_path: &Path) -> Vec<PathBuf> {
    let manifest = read_plugin_manifest_object(root_path);
    let manifest_agents = manifest.as_ref().and_then(|object| object.get("agents"));
    let mut paths = Vec::new();
    for root in collect_plugin_agent_roots(root_path, manifest_agents) {
        paths.extend(collect_agent_markdown_paths(&root));
    }
    paths.sort_by(|left, right| {
        js::locale_compare(&left.to_string_lossy(), &right.to_string_lossy())
    });
    paths
}

fn collect_plugin_agent_roots(root_path: &Path, manifest_agents: Option<&Value>) -> Vec<PathBuf> {
    let root = js::normalize_lexical(&absolute(root_path));
    let mut roots = Vec::new();
    let mut seen = BTreeSet::new();
    let mut add = |relative: &str| {
        let relative = relative.strip_prefix("./").unwrap_or(relative);
        let resolved = js::normalize_lexical(&root.join(relative));
        if !js::contains_path(&root, &resolved) {
            return;
        }
        if seen.insert(resolved.clone()) {
            roots.push(resolved);
        }
    };
    add("agents");
    match manifest_agents {
        Some(Value::String(value)) => add(value),
        Some(Value::Array(values)) => {
            for value in values {
                if let Some(value) = value.as_str() {
                    add(value);
                }
            }
        }
        _ => {}
    }
    roots
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

/// 同名后到者覆盖先到者，但保留首次出现的位置（JS `Map.set` 语义）。
pub fn apply_runtime_precedence(agents: Vec<AgentSummary>) -> Vec<AgentSummary> {
    let mut ordered: Vec<AgentSummary> = Vec::new();
    for agent in agents {
        match ordered
            .iter_mut()
            .find(|existing| existing.name == agent.name)
        {
            Some(existing) => *existing = agent,
            None => ordered.push(agent),
        }
    }
    ordered
}

pub fn attach_enabled_state(agents: Vec<AgentSummary>, state: &AgentsState) -> Vec<AgentSummary> {
    agents
        .into_iter()
        .map(|mut agent| {
            agent.enabled = if agent.scope == "user" {
                !state.disabled_ids.iter().any(|id| id == &agent.id)
            } else {
                true
            };
            agent
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_precedence_keeps_first_position_and_last_value() {
        let make = |name: &str, description: &str| AgentSummary {
            name: name.into(),
            description: description.into(),
            ..create_built_in_agents(&BTreeMap::new())[0].clone()
        };
        let merged = apply_runtime_precedence(vec![
            make("a", "first"),
            make("b", "second"),
            make("a", "replaced"),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "a");
        assert_eq!(merged[0].description, "replaced");
        assert_eq!(merged[1].name, "b");
    }

    #[test]
    fn builtin_ids_follow_source_scope_name() {
        let builtins = create_built_in_agents(&BTreeMap::new());
        assert_eq!(builtins[0].id, "built-in:built-in:general-purpose");
        assert_eq!(builtins[1].id, "built-in:built-in:explore");
        assert_eq!(builtins[1].read_only, Some(true));
    }
}
