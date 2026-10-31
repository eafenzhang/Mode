//! 选中项的导入执行与文件落地（复制 / 软链 / 硬链）。
//!
//! 真源：`settingsSyncService.ts` 的 `import*ForAgent`、`importSkillDirectory`、
//! `importCommandFile`、`importPluginDirectory` 与 `addPluginDirToConfig`。

use std::path::Path;

use serde::Serialize;

use crate::mcp_config::add_mcp_server_to_mode_config;
use crate::scan::{
    collect_command_candidates, collect_mcp_candidates, collect_plugin_candidates,
    collect_skill_candidates, command_skip_reason, mcp_skip_reason, plugin_skip_reason,
    skill_skip_reason, CommandCandidate, McpCandidate, PluginCandidate, SkillCandidate,
};
use crate::sources::{config_path_for_scope, target_root_for_scope};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillImportItem {
    pub name: String,
    pub path: String,
    pub source_scope: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandImportItem {
    pub name: String,
    pub path: String,
    pub source_scope: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginImportItem {
    pub name: String,
    pub path: String,
    pub source_scope: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerImportItem {
    pub name: String,
    pub path: String,
    pub source_scope: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskImportResult {
    pub agent: String,
    pub category: String,
    pub status: String,
    pub imported_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_results: Option<Vec<SkillImportItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_results: Option<Vec<CommandImportItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_results: Option<Vec<PluginImportItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_server_results: Option<Vec<McpServerImportItem>>,
}

fn task_status(imported: usize, failed: usize) -> String {
    if failed > 0 {
        "failed".to_string()
    } else if imported > 0 {
        "success".to_string()
    } else {
        "skipped".to_string()
    }
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn import_mode_copy(mode: Option<&str>) -> bool {
    mode == Some("copy")
}

// ---------------------------------------------------------------------------
// 文件落地
// ---------------------------------------------------------------------------

fn create_directory_symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = crate::sources::absolute_path(source);
    #[cfg(windows)]
    {
        junction::create(&source, target)
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(&source, target)
    }
}

fn import_directory(source: &Path, target: &Path, mode: Option<&str>) -> std::io::Result<()> {
    if !import_mode_copy(mode) {
        return create_directory_symlink(source, target);
    }
    copy_directory_recursive(source, target)
}

fn copy_directory_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(target).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "target exists",
        ));
    }
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return copy_symlink(source, target);
    }
    if !metadata.is_dir() {
        std::fs::copy(source, target)?;
        return Ok(());
    }
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        copy_directory_recursive(&entry.path(), &target.join(entry.file_name()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(source)?, target)
}

#[cfg(windows)]
fn copy_symlink(source: &Path, target: &Path) -> std::io::Result<()> {
    let resolved = std::fs::metadata(source)?;
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(std::fs::read_link(source)?, target)
    } else {
        std::os::windows::fs::symlink_file(std::fs::read_link(source)?, target)
    }
}

fn import_command_file(source: &Path, target: &Path, mode: Option<&str>) -> std::io::Result<()> {
    if import_mode_copy(mode) {
        if std::fs::symlink_metadata(target).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "target exists",
            ));
        }
        std::fs::copy(source, target)?;
        return Ok(());
    }
    let source = crate::sources::absolute_path(source);
    #[cfg(windows)]
    {
        if std::fs::hard_link(&source, target).is_ok() {
            return Ok(());
        }
        std::fs::copy(&source, target)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(&source, target)
    }
}

// ---------------------------------------------------------------------------
// 各分类导入
// ---------------------------------------------------------------------------

pub struct ImportRequest<'a> {
    pub home: &'a Path,
    pub workspace: Option<&'a Path>,
    pub agent: &'a str,
    pub source_scope: Option<&'a str>,
    pub target_scope: Option<&'a str>,
    pub import_mode: Option<&'a str>,
    pub selected_paths: Option<&'a [String]>,
}

fn path_selected(selected: Option<&[String]>, path: &Path) -> bool {
    match selected {
        None => true,
        Some(paths) => {
            let text = path.to_string_lossy();
            paths.iter().any(|candidate| candidate == text.as_ref())
        }
    }
}

fn source_scope_matches(scope: Option<&str>, candidate_scope: &str) -> bool {
    match scope {
        None => true,
        Some(scope) => scope == candidate_scope,
    }
}

pub fn import_skills(request: &ImportRequest<'_>) -> TaskImportResult {
    let candidates: Vec<SkillCandidate> = collect_skill_candidates(request.home, request.workspace)
        .into_iter()
        .filter(|candidate| {
            candidate.agent == request.agent
                && source_scope_matches(request.source_scope, candidate.source_root_scope)
                && path_selected(request.selected_paths, &candidate.source_path)
        })
        .collect();
    let mut imported = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut results = Vec::new();
    let mut existing = std::collections::HashMap::new();

    for candidate in candidates {
        let selected_target_root = match request.target_scope {
            Some(scope) => target_root_for_scope(request.home, request.workspace, scope, "skills"),
            None => Some(candidate.target_root.clone()),
        };
        let Some(target_root) = selected_target_root else {
            skipped += 1;
            results.push(SkillImportItem {
                name: candidate.name.clone(),
                path: candidate.source_path.to_string_lossy().into_owned(),
                source_scope: candidate.source_root_scope.to_string(),
                status: "skipped".to_string(),
                skip_reason: Some("targetExists".to_string()),
                version: candidate.version.clone(),
            });
            continue;
        };
        let target_candidate = SkillCandidate {
            target_root: target_root.clone(),
            target_path: target_root.join(basename(&candidate.source_path)),
            ..candidate.clone()
        };
        if let Some(skip_reason) = skill_skip_reason(&target_candidate, &mut existing) {
            skipped += 1;
            results.push(SkillImportItem {
                name: candidate.name.clone(),
                path: candidate.source_path.to_string_lossy().into_owned(),
                source_scope: candidate.source_root_scope.to_string(),
                status: "skipped".to_string(),
                skip_reason: Some(skip_reason),
                version: candidate.version.clone(),
            });
            continue;
        }
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = target_candidate.target_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            import_directory(
                &candidate.source_path,
                &target_candidate.target_path,
                request.import_mode,
            )
        })();
        match result {
            Ok(()) => {
                imported += 1;
                if let Some(keys) = existing.get_mut(&target_candidate.target_root) {
                    keys.insert(candidate.name_key.clone());
                }
                results.push(SkillImportItem {
                    name: candidate.name.clone(),
                    path: candidate.source_path.to_string_lossy().into_owned(),
                    source_scope: candidate.source_root_scope.to_string(),
                    status: "imported".to_string(),
                    skip_reason: None,
                    version: candidate.version.clone(),
                });
            }
            Err(_) => {
                failed += 1;
                results.push(SkillImportItem {
                    name: candidate.name.clone(),
                    path: candidate.source_path.to_string_lossy().into_owned(),
                    source_scope: candidate.source_root_scope.to_string(),
                    status: "failed".to_string(),
                    skip_reason: None,
                    version: candidate.version.clone(),
                });
            }
        }
    }

    TaskImportResult {
        agent: request.agent.to_string(),
        category: "skills".to_string(),
        status: task_status(imported, failed),
        imported_count: imported,
        skipped_count: skipped,
        failed_count: failed,
        skill_results: Some(results),
        command_results: None,
        plugin_results: None,
        mcp_server_results: None,
    }
}

pub fn import_commands(request: &ImportRequest<'_>) -> TaskImportResult {
    let candidates: Vec<CommandCandidate> =
        collect_command_candidates(request.home, request.workspace)
            .into_iter()
            .filter(|candidate| {
                candidate.agent == request.agent
                    && source_scope_matches(request.source_scope, candidate.source_root_scope)
                    && path_selected(request.selected_paths, &candidate.source_path)
            })
            .collect();
    let mut imported = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut results = Vec::new();
    let mut existing = std::collections::HashMap::new();

    for candidate in candidates {
        let selected_target_root = match request.target_scope {
            Some(scope) => {
                target_root_for_scope(request.home, request.workspace, scope, "commands")
            }
            None => Some(candidate.target_root.clone()),
        };
        let Some(target_root) = selected_target_root else {
            skipped += 1;
            results.push(CommandImportItem {
                name: candidate.name.clone(),
                path: candidate.source_path.to_string_lossy().into_owned(),
                source_scope: candidate.source_root_scope.to_string(),
                status: "skipped".to_string(),
                skip_reason: Some("targetExists".to_string()),
            });
            continue;
        };
        let target_candidate = CommandCandidate {
            target_root: target_root.clone(),
            target_path: target_root.join(&candidate.source_relative_path),
            ..candidate.clone()
        };
        if let Some(skip_reason) = command_skip_reason(&target_candidate, &mut existing) {
            skipped += 1;
            results.push(CommandImportItem {
                name: candidate.name.clone(),
                path: candidate.source_path.to_string_lossy().into_owned(),
                source_scope: candidate.source_root_scope.to_string(),
                status: "skipped".to_string(),
                skip_reason: Some(skip_reason),
            });
            continue;
        }
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = target_candidate.target_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            import_command_file(
                &candidate.source_path,
                &target_candidate.target_path,
                request.import_mode,
            )
        })();
        match result {
            Ok(()) => {
                imported += 1;
                if let Some(keys) = existing.get_mut(&target_candidate.target_root) {
                    keys.insert(candidate.name_key.clone());
                }
                results.push(CommandImportItem {
                    name: candidate.name.clone(),
                    path: candidate.source_path.to_string_lossy().into_owned(),
                    source_scope: candidate.source_root_scope.to_string(),
                    status: "imported".to_string(),
                    skip_reason: None,
                });
            }
            Err(_) => {
                failed += 1;
                results.push(CommandImportItem {
                    name: candidate.name.clone(),
                    path: candidate.source_path.to_string_lossy().into_owned(),
                    source_scope: candidate.source_root_scope.to_string(),
                    status: "failed".to_string(),
                    skip_reason: None,
                });
            }
        }
    }

    TaskImportResult {
        agent: request.agent.to_string(),
        category: "commands".to_string(),
        status: task_status(imported, failed),
        imported_count: imported,
        skipped_count: skipped,
        failed_count: failed,
        skill_results: None,
        command_results: Some(results),
        plugin_results: None,
        mcp_server_results: None,
    }
}

pub fn import_plugins(request: &ImportRequest<'_>) -> TaskImportResult {
    let candidates: Vec<PluginCandidate> =
        collect_plugin_candidates(request.home, request.workspace)
            .into_iter()
            .filter(|candidate| {
                candidate.agent == request.agent
                    && source_scope_matches(request.source_scope, candidate.source_root_scope)
                    && path_selected(request.selected_paths, &candidate.source_path)
            })
            .collect();
    let mut imported = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut results = Vec::new();
    let mut existing = std::collections::HashMap::new();

    for candidate in candidates {
        let selected_target_scope = request.target_scope.unwrap_or(candidate.source_root_scope);
        let selected_target_root = target_root_for_scope(
            request.home,
            request.workspace,
            selected_target_scope,
            "plugins",
        );
        let selected_config_path =
            config_path_for_scope(request.home, request.workspace, selected_target_scope);
        let (Some(target_root), Some(config_path)) = (selected_target_root, selected_config_path)
        else {
            skipped += 1;
            results.push(PluginImportItem {
                name: candidate.name.clone(),
                path: candidate.source_path.to_string_lossy().into_owned(),
                source_scope: candidate.source_root_scope.to_string(),
                status: "skipped".to_string(),
                skip_reason: Some("targetExists".to_string()),
                version: candidate.version.clone(),
            });
            continue;
        };
        let target_candidate = PluginCandidate {
            target_path: target_root.join(basename(&candidate.source_path)),
            ..candidate.clone()
        };
        if let Some(skip_reason) = plugin_skip_reason(
            &target_candidate,
            &mut existing,
            request.home,
            request.workspace,
            selected_target_scope,
        ) {
            skipped += 1;
            results.push(PluginImportItem {
                name: candidate.name.clone(),
                path: candidate.source_path.to_string_lossy().into_owned(),
                source_scope: candidate.source_root_scope.to_string(),
                status: "skipped".to_string(),
                skip_reason: Some(skip_reason),
                version: candidate.version.clone(),
            });
            continue;
        }
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = target_candidate.target_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            import_directory(
                &candidate.source_path,
                &target_candidate.target_path,
                request.import_mode,
            )?;
            crate::mcp_config::add_plugin_dir_to_config(&config_path, &target_candidate.target_path)
                .map_err(std::io::Error::other)
        })();
        match result {
            Ok(()) => {
                imported += 1;
                if let Some(ids) = existing.get_mut(&config_path) {
                    ids.insert(candidate.id.to_lowercase());
                }
                results.push(PluginImportItem {
                    name: candidate.name.clone(),
                    path: candidate.source_path.to_string_lossy().into_owned(),
                    source_scope: candidate.source_root_scope.to_string(),
                    status: "imported".to_string(),
                    skip_reason: None,
                    version: candidate.version.clone(),
                });
            }
            Err(_) => {
                failed += 1;
                results.push(PluginImportItem {
                    name: candidate.name.clone(),
                    path: candidate.source_path.to_string_lossy().into_owned(),
                    source_scope: candidate.source_root_scope.to_string(),
                    status: "failed".to_string(),
                    skip_reason: None,
                    version: candidate.version.clone(),
                });
            }
        }
    }

    TaskImportResult {
        agent: request.agent.to_string(),
        category: "plugins".to_string(),
        status: task_status(imported, failed),
        imported_count: imported,
        skipped_count: skipped,
        failed_count: failed,
        skill_results: None,
        command_results: None,
        plugin_results: Some(results),
        mcp_server_results: None,
    }
}

pub fn import_mcp_servers(request: &ImportRequest<'_>) -> TaskImportResult {
    let candidates: Vec<McpCandidate> = collect_mcp_candidates(request.home, request.workspace)
        .into_iter()
        .filter(|candidate| {
            candidate.agent == request.agent
                && source_scope_matches(request.source_scope, candidate.source_root_scope)
                && path_selected(request.selected_paths, Path::new(&candidate.source_path))
        })
        .collect();
    let mut imported = 0;
    let mut skipped = 0;
    let mut failed = 0;
    let mut results = Vec::new();
    let mut existing = std::collections::HashMap::new();

    for candidate in candidates {
        let selected_target_scope = request.target_scope.unwrap_or(candidate.source_root_scope);
        let Some(config_path) =
            config_path_for_scope(request.home, request.workspace, selected_target_scope)
        else {
            skipped += 1;
            results.push(McpServerImportItem {
                name: candidate.name.clone(),
                path: candidate.source_path.clone(),
                source_scope: candidate.source_root_scope.to_string(),
                status: "skipped".to_string(),
                skip_reason: Some("sameNameExists".to_string()),
            });
            continue;
        };
        if let Some(skip_reason) = mcp_skip_reason(
            &candidate,
            &mut existing,
            request.home,
            request.workspace,
            selected_target_scope,
        ) {
            skipped += 1;
            results.push(McpServerImportItem {
                name: candidate.name.clone(),
                path: candidate.source_path.clone(),
                source_scope: candidate.source_root_scope.to_string(),
                status: "skipped".to_string(),
                skip_reason: Some(skip_reason),
            });
            continue;
        }
        match add_mcp_server_to_mode_config(&config_path, &candidate.name, &candidate.config) {
            Ok(()) => {
                imported += 1;
                if let Some(keys) = existing.get_mut(selected_target_scope) {
                    keys.insert(candidate.name_key.clone());
                }
                results.push(McpServerImportItem {
                    name: candidate.name.clone(),
                    path: candidate.source_path.clone(),
                    source_scope: candidate.source_root_scope.to_string(),
                    status: "imported".to_string(),
                    skip_reason: None,
                });
            }
            Err(_) => {
                failed += 1;
                results.push(McpServerImportItem {
                    name: candidate.name.clone(),
                    path: candidate.source_path.clone(),
                    source_scope: candidate.source_root_scope.to_string(),
                    status: "failed".to_string(),
                    skip_reason: None,
                });
            }
        }
    }

    TaskImportResult {
        agent: request.agent.to_string(),
        category: "mcpServers".to_string(),
        status: task_status(imported, failed),
        imported_count: imported,
        skipped_count: skipped,
        failed_count: failed,
        skill_results: None,
        command_results: None,
        plugin_results: None,
        mcp_server_results: Some(results),
    }
}

pub fn skipped_task(agent: &str, category: &str) -> TaskImportResult {
    TaskImportResult {
        agent: agent.to_string(),
        category: category.to_string(),
        status: "skipped".to_string(),
        imported_count: 0,
        skipped_count: 1,
        failed_count: 0,
        skill_results: None,
        command_results: None,
        plugin_results: None,
        mcp_server_results: None,
    }
}
