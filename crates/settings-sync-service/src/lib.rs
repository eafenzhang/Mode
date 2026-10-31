//! 外部 Agent 配置同步服务（Rust 实现）。
//!
//! 真源：`packages/services/src/settings-sync/settingsSyncService.ts`
//! （clean-room：只读本仓库 TS）。行为契约与已知差异见
//! `docs/specs/settings-sync-service.md`。
//!
//! 覆盖六个方法：Claude AGENTS.md 迁移状态/复制、`detect`、`importSelected`、
//! 首启提示状态读取与标记。

mod import;
mod mcp_config;
mod scan;
mod sources;

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use import::{
    CommandImportItem, McpServerImportItem, PluginImportItem, SkillImportItem, TaskImportResult,
};
pub use scan::{AgentSummary, CategorySummary, DiscoveryResult, SourceRootSummary};

use plugin_discovery::js;
use settings_store::SettingsService;

// ---------------------------------------------------------------------------
// 依赖
// ---------------------------------------------------------------------------

/// 首启提示状态的读写面；壳内用 `settings_store::SettingsService` 实现。
pub trait SettingsBridge: Send + Sync {
    fn get_settings(&self) -> Result<Value, String>;
    fn update_settings(&self, patch: &Value) -> Result<(), String>;
}

impl SettingsBridge for SettingsService {
    fn get_settings(&self) -> Result<Value, String> {
        Ok(self.get())
    }

    fn update_settings(&self, patch: &Value) -> Result<(), String> {
        self.update(patch)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

// ---------------------------------------------------------------------------
// 参数与结果
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SettingsSyncServiceOptions {
    pub home_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DetectParams {
    pub workspace_path: Option<String>,
    pub workspace_identity: Option<String>,
    pub categories: Option<Vec<String>>,
    pub intent: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Selection {
    pub agent: String,
    pub category: String,
    pub source_scope: Option<String>,
    pub target_scope: Option<String>,
    pub import_mode: Option<String>,
    pub skill_paths: Option<Vec<String>>,
    pub command_paths: Option<Vec<String>>,
    pub plugin_paths: Option<Vec<String>>,
    pub mcp_server_paths: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportParams {
    pub workspace_path: Option<String>,
    pub workspace_identity: Option<String>,
    pub selections: Vec<Selection>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub success_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
    pub task_results: Vec<TaskImportResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAgentsFileMigrationStatus {
    pub source_path: String,
    pub target_path: String,
    pub source_exists: bool,
    pub target_exists: bool,
    pub supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAgentsFileCopyResult {
    pub source_path: String,
    pub target_path: String,
    pub status: String,
    pub overwritten: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunPromptState {
    pub handled: bool,
}

// ---------------------------------------------------------------------------
// 服务
// ---------------------------------------------------------------------------

pub struct SettingsSyncService {
    options: SettingsSyncServiceOptions,
    settings: Arc<dyn SettingsBridge>,
}

impl SettingsSyncService {
    pub fn new(options: SettingsSyncServiceOptions, settings: Arc<dyn SettingsBridge>) -> Self {
        Self { options, settings }
    }

    fn home(&self) -> PathBuf {
        if let Some(home) = &self.options.home_dir {
            if !js::js_trim(&home.to_string_lossy()).is_empty() {
                return home.clone();
            }
        }
        std::env::var("HOME")
            .ok()
            .map(|value| js::js_trim(&value).to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                std::env::var("USERPROFILE")
                    .ok()
                    .map(|value| js::js_trim(&value).to_string())
                    .filter(|value| !value.is_empty())
            })
            .map(PathBuf::from)
            .unwrap_or_else(js::node_homedir)
    }

    fn workspace(&self, workspace_path: Option<&str>) -> Option<PathBuf> {
        workspace_path
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    pub fn get_claude_agents_file_migration_status(&self) -> ClaudeAgentsFileMigrationStatus {
        let home = self.home();
        let source_path = sources::claude_user_agents_file(&home);
        let target_path = sources::user_mode_agents_file(&home);
        let source_exists = scan::path_exists(&source_path);
        let target_exists = scan::path_exists(&target_path);
        ClaudeAgentsFileMigrationStatus {
            source_path: source_path.to_string_lossy().into_owned(),
            target_path: target_path.to_string_lossy().into_owned(),
            source_exists,
            target_exists,
            supported: source_exists,
            unavailable_reason: if source_exists {
                None
            } else {
                Some("missingSource".to_string())
            },
        }
    }

    pub fn copy_claude_agents_file_to_mode_agents_file(
        &self,
        overwrite: bool,
    ) -> ClaudeAgentsFileCopyResult {
        let home = self.home();
        let source_path = sources::claude_user_agents_file(&home);
        let target_path = sources::user_mode_agents_file(&home);
        let source_exists = scan::path_exists(&source_path);
        let target_exists = scan::path_exists(&target_path);
        let source_text = source_path.to_string_lossy().into_owned();
        let target_text = target_path.to_string_lossy().into_owned();

        if !source_exists {
            return ClaudeAgentsFileCopyResult {
                source_path: source_text,
                target_path: target_text,
                status: "skipped".to_string(),
                overwritten: false,
                skipped_reason: Some("missingSource".to_string()),
            };
        }
        if target_exists && !overwrite {
            return ClaudeAgentsFileCopyResult {
                source_path: source_text,
                target_path: target_text,
                status: "skipped".to_string(),
                overwritten: false,
                skipped_reason: Some("targetExists".to_string()),
            };
        }
        if let Some(parent) = target_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::copy(&source_path, &target_path).is_err() {
            return ClaudeAgentsFileCopyResult {
                source_path: source_text,
                target_path: target_text,
                status: "failed".to_string(),
                overwritten: target_exists,
                skipped_reason: None,
            };
        }
        ClaudeAgentsFileCopyResult {
            source_path: source_text,
            target_path: target_text,
            status: "copied".to_string(),
            overwritten: target_exists,
            skipped_reason: None,
        }
    }

    pub fn detect(&self, params: &DetectParams) -> DiscoveryResult {
        let home = self.home();
        let workspace = self.workspace(params.workspace_path.as_deref());
        let is_manual = params.intent.as_deref() == Some("manualImport");
        let categories = params.categories.as_deref().unwrap_or(&[]);
        if !is_manual {
            return DiscoveryResult { agents: Vec::new() };
        }
        let workspace = workspace.as_deref();
        if categories.iter().any(|category| category == "skills") {
            return scan::build_skills_discovery(&home, workspace);
        }
        if categories.iter().any(|category| category == "commands") {
            return scan::build_commands_discovery(&home, workspace);
        }
        if categories.iter().any(|category| category == "plugins") {
            return scan::build_plugins_discovery(&home, workspace);
        }
        if categories.iter().any(|category| category == "mcpServers") {
            return scan::build_mcp_discovery(&home, workspace);
        }
        DiscoveryResult { agents: Vec::new() }
    }

    pub fn import_selected(&self, params: &ImportParams) -> ImportResult {
        let home = self.home();
        let workspace = self.workspace(params.workspace_path.as_deref());
        let workspace = workspace.as_deref();
        let mut task_results = Vec::new();

        for selection in &params.selections {
            let request = import::ImportRequest {
                home: &home,
                workspace,
                agent: &selection.agent,
                source_scope: selection.source_scope.as_deref(),
                target_scope: selection.target_scope.as_deref(),
                import_mode: selection.import_mode.as_deref(),
                selected_paths: None,
            };
            match selection.category.as_str() {
                "skills" => task_results.push(import::import_skills(&with_selected(
                    &request,
                    selection.skill_paths.as_deref(),
                ))),
                "commands" => task_results.push(import::import_commands(&with_selected(
                    &request,
                    selection.command_paths.as_deref(),
                ))),
                "plugins" => task_results.push(import::import_plugins(&with_selected(
                    &request,
                    selection.plugin_paths.as_deref(),
                ))),
                "mcpServers" => task_results.push(import::import_mcp_servers(&with_selected(
                    &request,
                    selection.mcp_server_paths.as_deref(),
                ))),
                category => task_results.push(import::skipped_task(&selection.agent, category)),
            }
        }

        ImportResult {
            success_count: task_results.iter().map(|task| task.imported_count).sum(),
            skipped_count: task_results.iter().map(|task| task.skipped_count).sum(),
            failed_count: task_results.iter().map(|task| task.failed_count).sum(),
            task_results,
        }
    }

    pub fn get_first_run_prompt_state(&self) -> Result<FirstRunPromptState, String> {
        let settings = self.settings.get_settings()?;
        Ok(FirstRunPromptState {
            handled: settings
                .get("settingsSyncFirstRunPromptHandled")
                .and_then(Value::as_bool)
                == Some(true),
        })
    }

    pub fn mark_first_run_prompt_handled(&self) -> Result<(), String> {
        self.settings.update_settings(&serde_json::json!({
            "settingsSyncFirstRunPromptHandled": true
        }))
    }
}

fn with_selected<'a>(
    request: &import::ImportRequest<'a>,
    paths: Option<&'a [String]>,
) -> import::ImportRequest<'a> {
    import::ImportRequest {
        home: request.home,
        workspace: request.workspace,
        agent: request.agent,
        source_scope: request.source_scope,
        target_scope: request.target_scope,
        import_mode: request.import_mode,
        selected_paths: paths,
    }
}

/// 供测试与调用方构造选择项。
pub fn selection_for(
    agent: &str,
    category: &str,
    source_scope: Option<&str>,
    target_scope: Option<&str>,
    import_mode: Option<&str>,
) -> Selection {
    Selection {
        agent: agent.to_string(),
        category: category.to_string(),
        source_scope: source_scope.map(str::to_string),
        target_scope: target_scope.map(str::to_string),
        import_mode: import_mode.map(str::to_string),
        ..Selection::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct MemorySettings(std::sync::Mutex<Value>);

    impl SettingsBridge for MemorySettings {
        fn get_settings(&self) -> Result<Value, String> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn update_settings(&self, patch: &Value) -> Result<(), String> {
            let mut current = self.0.lock().unwrap();
            if let (Value::Object(target), Value::Object(patch)) = (&mut *current, patch) {
                for (key, value) in patch {
                    target.insert(key.clone(), value.clone());
                }
            }
            Ok(())
        }
    }

    #[test]
    fn first_run_state_round_trips() {
        let settings = Arc::new(MemorySettings(std::sync::Mutex::new(serde_json::json!({}))));
        let service = SettingsSyncService::new(
            SettingsSyncServiceOptions {
                home_dir: Some(PathBuf::from("/nonexistent-home")),
            },
            settings,
        );
        assert!(!service.get_first_run_prompt_state().unwrap().handled);
        service.mark_first_run_prompt_handled().unwrap();
        assert!(service.get_first_run_prompt_state().unwrap().handled);
    }

    #[test]
    fn first_run_intent_never_scans() {
        let settings = Arc::new(MemorySettings(std::sync::Mutex::new(serde_json::json!({}))));
        let service = SettingsSyncService::new(
            SettingsSyncServiceOptions {
                home_dir: Some(PathBuf::from("/nonexistent-home")),
            },
            settings,
        );
        let result = service.detect(&DetectParams {
            categories: Some(vec!["skills".to_string()]),
            intent: Some("firstRun".to_string()),
            ..DetectParams::default()
        });
        assert!(result.agents.is_empty());
    }

    #[test]
    fn skill_metadata_uses_loose_fallback() {
        let (name, version) = crate::scan::read_skill_metadata_from_markdown(
            "---\nname: 'quoted name'\nversion: 2.1\n---\nbody\n",
            "fallback",
        );
        assert_eq!(name, "quoted name");
        assert_eq!(version.as_deref(), Some("2.1"));
        let (name, version) =
            crate::scan::read_skill_metadata_from_markdown("no frontmatter", "fallback");
        assert_eq!(name, "fallback");
        assert!(version.is_none());
    }

    #[test]
    fn selection_defaults_are_empty() {
        let selection = Selection::default();
        assert!(selection.agent.is_empty());
        let map: BTreeMap<String, Value> = BTreeMap::new();
        assert!(map.is_empty());
    }
}
