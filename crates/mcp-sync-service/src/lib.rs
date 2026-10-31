//! MCP 用户目录同步服务（本地读写、候选、导入导出）。
//!
//! 真源：`packages/services/src/mcp-sync/mcpSyncService.ts`（clean-room：只读本仓库 TS）。
//! 行为契约与范围见 `docs/specs/mcp-sync-service.md`。
//!
//! 覆盖七个本地方法；`listWorkspaceMcpServerStatuses` 是 Agent 运行态的投影，
//! 需要 W4 的 Agent 控制面就位后再接，不在本 crate 内。

mod config;
mod paths;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use config::{
    normalize_mcp_name_key, McpSyncCandidate, NativeMcpFileReference, NativeMcpServerRecord,
    UserMcpRecord,
};
pub use paths::{SettingsDirectoryLocation, MODE_DESCRIPTOR};

use crate::config::{
    check_directory_write_access, collect_effective_user_records,
    collect_effective_user_records_by_name, read_directory_servers_from_file,
    read_directory_servers_from_preferred_sources, read_json_object, read_server_map,
    record_to_candidate, server_enabled, set_server_enabled, write_document_private,
    write_server_map, McpDocument, ServerMap,
};
use crate::paths::{
    build_directory_config_path, build_directory_mcp_location, find_descriptor_by_location,
    rewrite_filesystem_mcp_config, user_mode_config_path, PathRewritePaths,
};

#[derive(Debug, Clone, Default)]
pub struct McpSyncServiceOptions {
    /// 测试注入；默认按 `HOME.trim() || USERPROFILE.trim() || homedir()` 解析。
    pub home_dir: Option<PathBuf>,
}

impl McpSyncServiceOptions {
    pub fn with_home(home_dir: PathBuf) -> Self {
        Self {
            home_dir: Some(home_dir),
        }
    }
}

pub struct McpSyncService {
    options: McpSyncServiceOptions,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoadRequest {
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SavePayload {
    pub action: String,
    pub source: Option<String>,
    pub name: String,
    pub config: Option<Value>,
    pub enabled: Option<bool>,
    pub project_path: Option<String>,
    pub location: Option<SettingsDirectoryLocation>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedServer {
    pub id: String,
    pub name: String,
    pub config: Value,
    pub enabled: bool,
    pub source: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportParams {
    pub servers: Vec<ExportedServer>,
    pub local_home_dir: String,
    pub local_workspace_path: Option<String>,
    pub remote_workspace_path: Option<String>,
    pub overwrite: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateListResult {
    pub candidates: Vec<McpSyncCandidate>,
    pub local_home_dir: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub name: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatusResult {
    pub remote_home_dir: String,
    pub statuses: Vec<RemoteStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub local_home_dir: String,
    pub servers: Vec<ExportedServer>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResultItem {
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub results: Vec<ImportResultItem>,
}

impl McpSyncService {
    pub fn new(options: McpSyncServiceOptions) -> Self {
        Self { options }
    }

    fn home(&self) -> PathBuf {
        config::resolve_home(self.options.home_dir.as_deref())
    }

    pub fn load_mcp_from_user_directory(
        &self,
        request: Option<&LoadRequest>,
    ) -> Result<Vec<NativeMcpServerRecord>, String> {
        let home = self.home();
        let mut servers = Vec::new();
        if let Some(workspace_path) = request.and_then(|request| request.workspace_path.as_deref())
        {
            servers.extend(read_directory_servers_from_preferred_sources(
                &home,
                "workspace",
                Some(workspace_path),
            )?);
        }
        servers.extend(read_directory_servers_from_preferred_sources(
            &home,
            "user",
            request.and_then(|request| request.workspace_path.as_deref()),
        )?);
        Ok(servers)
    }

    pub fn save_mcp_to_user_directory(&self, payload: &SavePayload) -> Result<(), String> {
        let home = self.home();
        if payload.action == "set-enabled" {
            let Some(enabled) = payload.enabled else {
                return Err("Missing enabled value for MCP set-enabled action".to_string());
            };
            let scope = if payload.project_path.is_some() {
                "workspace"
            } else {
                "user"
            };
            let location = match &payload.location {
                Some(location) => location.clone(),
                None => build_directory_mcp_location(
                    &home,
                    MODE_DESCRIPTOR,
                    scope,
                    payload.project_path.as_deref(),
                )?,
            };
            let descriptor = find_descriptor_by_location(&location)?;
            write_server_enabled_to_file(&home, descriptor, &location, &payload.name, enabled)?;
            cleanup_legacy_enabled_override(&home, &location, &payload.name)?;
            return Ok(());
        }

        let scope = if payload.project_path.is_some() {
            "workspace"
        } else {
            "user"
        };
        let existing = read_directory_servers_from_file(
            &home,
            MODE_DESCRIPTOR,
            scope,
            payload.project_path.as_deref(),
        )?;
        let mut next_servers = ServerMap::new();
        for server in existing {
            next_servers.insert(server.name, server.config);
        }
        if payload.action == "upsert" {
            let Some(config) = &payload.config else {
                return Err("Missing MCP config for upsert action".to_string());
            };
            next_servers.insert(payload.name.clone(), config.clone());
        } else {
            next_servers.shift_remove(&payload.name);
        }
        write_mode_servers_to_file(&home, scope, next_servers, payload.project_path.as_deref())
    }

    pub fn list_local_user_mcp_candidates(&self) -> Result<CandidateListResult, String> {
        let home = self.home();
        Ok(CandidateListResult {
            candidates: collect_effective_user_records(&home)?
                .iter()
                .map(record_to_candidate)
                .collect(),
            local_home_dir: home.to_string_lossy().into_owned(),
        })
    }

    pub fn list_remote_user_mcp_statuses(
        &self,
        names: &[String],
    ) -> Result<RemoteStatusResult, String> {
        let home = self.home();
        let existing = collect_effective_user_records_by_name(&home)?;
        let statuses = names
            .iter()
            .map(|name| {
                let key = normalize_mcp_name_key(name);
                match existing
                    .iter()
                    .find(|(existing_key, _)| existing_key == &key)
                {
                    Some((_, record)) => RemoteStatus {
                        name: name.clone(),
                        exists: true,
                        path: Some(record.path.clone()),
                    },
                    None => RemoteStatus {
                        name: name.clone(),
                        exists: false,
                        path: None,
                    },
                }
            })
            .collect();
        Ok(RemoteStatusResult {
            remote_home_dir: home.to_string_lossy().into_owned(),
            statuses,
        })
    }

    pub fn export_mcp_servers(&self, server_ids: &[String]) -> Result<ExportResult, String> {
        let home = self.home();
        let candidates: Vec<McpSyncCandidate> = collect_effective_user_records(&home)?
            .iter()
            .map(record_to_candidate)
            .collect();
        let mut servers = Vec::new();
        for id in server_ids {
            let Some(candidate) = candidates.iter().find(|candidate| &candidate.id == id) else {
                return Err(format!("mcp sync candidate not found: {id}"));
            };
            servers.push(ExportedServer {
                id: candidate.id.clone(),
                name: candidate.name.clone(),
                config: candidate.config.clone(),
                enabled: candidate.enabled,
                source: candidate.source.clone(),
                path: candidate.path.clone(),
            });
        }
        Ok(ExportResult {
            local_home_dir: home.to_string_lossy().into_owned(),
            servers,
        })
    }

    pub fn check_remote_user_mcp_write_access(&self) -> Result<Value, String> {
        let home = self.home();
        let directory = user_mode_config_path(&home)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.clone());
        Ok(check_directory_write_access(&directory))
    }

    pub fn import_mcp_servers(&self, params: &ImportParams) -> Result<ImportResult, String> {
        if params.overwrite == Some(true) {
            return Err("mcp sync overwrite is not supported".to_string());
        }
        let home = self.home();
        let target_path = user_mode_config_path(&home);
        let current = read_json_object(&target_path)?.unwrap_or_default();
        let mut target_servers = read_server_map(&current, MODE_DESCRIPTOR.config_key);
        let mut existing_by_name = collect_effective_user_records_by_name(&home)?;
        let mut results = Vec::new();
        let mut changed = false;

        for server in &params.servers {
            let name_key = normalize_mcp_name_key(&server.name);
            if target_servers.contains_key(&server.name) {
                results.push(ImportResultItem {
                    name: server.name.clone(),
                    status: "skipped".to_string(),
                    path: Some(target_path.to_string_lossy().into_owned()),
                    error: None,
                });
                continue;
            }
            if let Some((_, existing)) = existing_by_name
                .iter()
                .find(|(existing_key, _)| existing_key == &name_key)
            {
                results.push(ImportResultItem {
                    name: server.name.clone(),
                    status: "skipped".to_string(),
                    path: Some(existing.path.clone()),
                    error: None,
                });
                continue;
            }

            let rewritten = rewrite_filesystem_mcp_config(
                &server.name,
                &set_server_enabled(&server.config, server.enabled),
                &PathRewritePaths {
                    local_home_dir: params.local_home_dir.clone(),
                    local_workspace_path: params.local_workspace_path.clone(),
                    remote_home_dir: home.to_string_lossy().into_owned(),
                    remote_workspace_path: params.remote_workspace_path.clone(),
                },
            );
            target_servers.insert(server.name.clone(), rewritten.clone());
            existing_by_name.push((
                name_key,
                UserMcpRecord {
                    name: server.name.clone(),
                    config: rewritten,
                    enabled: server.enabled,
                    source: "mode".to_string(),
                    path: target_path.to_string_lossy().into_owned(),
                },
            ));
            results.push(ImportResultItem {
                name: server.name.clone(),
                status: "synced".to_string(),
                path: Some(target_path.to_string_lossy().into_owned()),
                error: None,
            });
            changed = true;
        }

        if changed {
            let next = write_server_map(&current, MODE_DESCRIPTOR.config_key, target_servers);
            write_document_private(&target_path, &next)?;
        }
        Ok(ImportResult { results })
    }
}

fn write_server_enabled_to_file(
    home_dir: &Path,
    descriptor: paths::DirectoryDescriptor,
    location: &SettingsDirectoryLocation,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let scope = if location.scope == "project" {
        "workspace"
    } else {
        "user"
    };
    let workspace_path = if scope == "workspace" {
        location.project_path.as_deref()
    } else {
        None
    };
    let file_path = build_directory_config_path(home_dir, descriptor, scope, workspace_path)?;
    let document = read_json_object(&file_path)?.unwrap_or_default();
    let mut server_map = read_server_map(&document, descriptor.config_key);
    let Some(current_server) = server_map.get(name) else {
        return Ok(());
    };
    if !current_server.is_object() {
        return Ok(());
    }
    server_map.insert(
        name.to_string(),
        set_server_enabled(current_server, enabled),
    );
    let next = write_server_map(&document, descriptor.config_key, server_map);
    let (cleaned, _changed) = remove_legacy_mcp_enabled_override(&next, location, name);
    write_document_private(&file_path, &cleaned)
}

fn cleanup_legacy_enabled_override(
    home_dir: &Path,
    location: &SettingsDirectoryLocation,
    name: &str,
) -> Result<(), String> {
    let config_path = user_mode_config_path(home_dir);
    let document = read_json_object(&config_path)?.unwrap_or_default();
    let (next, changed) = remove_legacy_mcp_enabled_override(&document, location, name);
    if changed {
        write_document_private(&config_path, &next)?;
    }
    Ok(())
}

fn remove_legacy_mcp_enabled_override(
    document: &McpDocument,
    location: &SettingsDirectoryLocation,
    name: &str,
) -> (McpDocument, bool) {
    let Some(mcp) = document.mcp.as_ref() else {
        return (document.clone(), false);
    };
    let Some(path_value) = mcp.extra.get(&location.directory_path) else {
        return (document.clone(), false);
    };
    let Some(path_config) = path_value.as_object() else {
        return (document.clone(), false);
    };
    if !path_config.contains_key(name) {
        return (document.clone(), false);
    }

    let mut next = document.clone();
    let section = next.mcp.as_mut().expect("mcp exists");
    let mut path_config = path_config.clone();
    path_config.remove(name);
    if path_config.is_empty() {
        section.extra.shift_remove(&location.directory_path);
    } else {
        section
            .extra
            .insert(location.directory_path.clone(), Value::Object(path_config));
    }
    (next, true)
}

fn write_mode_servers_to_file(
    home_dir: &Path,
    scope: &str,
    servers: ServerMap,
    workspace_path: Option<&str>,
) -> Result<(), String> {
    let file_path = build_directory_config_path(home_dir, MODE_DESCRIPTOR, scope, workspace_path)?;
    let document = read_json_object(&file_path)?.unwrap_or_default();
    let next = write_server_map(&document, MODE_DESCRIPTOR.config_key, servers);
    write_document_private(&file_path, &next)
}

/// 兼容旧调用方的 `enabled` 读取；与 config 模块同一实现。
pub fn read_server_enabled(config: &Value) -> bool {
    server_enabled(config)
}
