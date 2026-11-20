//! Plugins 会话面（C4 T2）：list / describe / overview / setEnabled / marketplace /
//! install / uninstall / update / referenceCatalog / resolveSuggestedReference。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §3 与 `fixtures/c4/plugins-*.json`。
//!
//! # 事实源（全部在本仓库既有/黑盒观察的布局内）
//!
//! ```text
//! <dataRoot>/cli/config.json                          plugins.enabledPlugins / suppressedBuiltins
//! <dataRoot>/cli/plugins/known_marketplaces.json      本地 marketplace 来源
//! <dataRoot>/cli/plugins/marketplaces/<name>/          marketplace 快照（含 marketplace.json）
//! <dataRoot>/cli/plugins/installed_plugins.json       已安装插件记录
//! <dataRoot>/cli/plugins/cache/<mkt>/<plugin>/<ver>/  插件缓存
//! <dataRoot>/cli/plugins/cache/mode-plugins-official/ 官方内置缓存（由宿主物化）
//! ```
//!
//! 官方 availablePlugins 依赖 CLI 打包的目录快照；本 crate 只消费本地 marketplace
//! 与官方缓存，不内置远端目录（差异登记在契约 §3.3）。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use agent_protocol::plugin::{
    AvailablePluginSummary, InstalledPluginSummary, PluginComponentGroup, PluginComponentItem,
    PluginComponentKind, PluginDescribeMetadata, PluginDiagnostic, PluginDiagnosticSeverity,
    PluginInfo, PluginMarketplaceSummary, PluginOverviewCapability, PluginReferenceAuthority,
    PluginReferenceCatalogEntry, PluginScope, PluginSuggestedReferenceStatus,
    PluginsDescribeResult, PluginsInstallResult, PluginsListResult,
    PluginsMarketplaceMutationResult, PluginsOverviewResult, PluginsReferenceCatalogResult,
    PluginsResolveSuggestedReferenceResult, PluginsSetEnabledResult, PluginsUninstallResult,
    PluginsUpdateParams, SourceTrustOfficial,
};
use agent_protocol::session_create::{
    McpEntry, McpServer, McpServerHttp, McpServerStdio, McpTransportType,
};
use plugin_discovery::{
    canonical_plugin_id, is_default_enabled_official_plugin_id, read_installed_plugin_records,
    read_plugin_manifest_object, resolve_config_path, resolve_plugin_storage_root,
    scan_official_plugin_cache_roots, InstalledPluginRecord, MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID,
};
use serde_json::{json, Map, Value};

use crate::path_safety::{is_safe_segment, is_within};

const KNOWN_MARKETPLACES_FILE: &str = "known_marketplaces.json";
const INSTALLED_PLUGINS_FILE: &str = "installed_plugins.json";
const MARKETPLACE_FILE: &str = "marketplace.json";

/// 校验调用方/清单提供的目录名，避免 `../` 把落盘路径带出插件根。
fn validate_segment(value: &str, label: &str) -> Result<(), String> {
    if is_safe_segment(value) {
        Ok(())
    } else {
        Err(format!("Invalid {label}: {value:?}"))
    }
}

/// 插件服务的路径与配置快照（每次请求重新读，避免跨进程缓存陈旧）。
pub struct PluginService {
    home_dir: PathBuf,
    config_path: PathBuf,
    storage_root: PathBuf,
}

impl std::fmt::Debug for PluginService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginService")
            .field("home_dir", &self.home_dir)
            .field("storage_root", &self.storage_root)
            .finish()
    }
}

/// 一个可列出的插件根（官方缓存 / 已安装 / 配置目录）。
struct PluginRoot {
    root_path: PathBuf,
    name: String,
    marketplace: String,
    source: String,
    default_enabled: bool,
    installed: Option<InstalledPluginRecord>,
}

struct PluginListSnapshot {
    plugins: Vec<PluginInfo>,
    installed: Vec<(InstalledPluginRecord, PluginInfo)>,
    diagnostics: Vec<PluginDiagnostic>,
}

impl PluginService {
    /// 生产构造：home 取 `HOME`/`USERPROFILE`；配置/缓存在 agent-server 数据根下
    /// （`<dataRoot>/cli/...`，与 oracle 的 CLI 布局一致）。
    pub fn from_data_root(data_root: &Path) -> Self {
        let home_dir = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
            .map(PathBuf::from)
            .unwrap_or_else(|| data_root.to_path_buf());
        Self::with_paths(home_dir, data_root.to_path_buf())
    }

    pub fn with_paths(home_dir: PathBuf, data_root: PathBuf) -> Self {
        let config_path = data_root.join("cli").join("config.json");
        let storage_root = resolve_plugin_storage_root(&home_dir, &read_storage_dir(&config_path));
        Self {
            home_dir,
            config_path,
            storage_root,
        }
    }

    fn config(&self) -> Map<String, Value> {
        read_json_object(&self.config_path).unwrap_or_default()
    }

    fn plugin_config(&self) -> plugin_discovery::PluginConfig {
        plugin_discovery::read_plugin_config(&self.config())
    }

    fn write_config(&self, config: &Map<String, Value>) -> Result<(), String> {
        write_json_object(&self.config_path, config)
    }

    fn installed_records(&self) -> Vec<InstalledPluginRecord> {
        read_installed_plugin_records(&self.storage_root)
    }

    fn known_marketplaces(&self) -> Vec<Value> {
        read_json_object(&self.storage_root.join(KNOWN_MARKETPLACES_FILE))
            .and_then(|object| {
                object
                    .get("marketplaces")
                    .and_then(Value::as_array)
                    .cloned()
            })
            .unwrap_or_default()
    }

    fn roots(&self) -> Vec<PluginRoot> {
        let config = self.plugin_config();
        let mut roots = Vec::new();
        for cached in scan_official_plugin_cache_roots(&self.storage_root) {
            if let Some(version_root) = cached.version_roots.first() {
                roots.push(PluginRoot {
                    root_path: version_root.clone(),
                    name: cached.name.clone(),
                    marketplace: MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID.to_string(),
                    source: "official".to_string(),
                    default_enabled: is_default_enabled_official_plugin_id(&format!(
                        "{}@{}",
                        cached.name, MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID
                    )),
                    installed: None,
                });
            }
        }
        for record in self.installed_records() {
            let root_path = PathBuf::from(&record.install_path);
            let name = record
                .name
                .clone()
                .or_else(|| record.id.split('@').next().map(str::to_string))
                .unwrap_or_else(|| record.id.clone());
            roots.push(PluginRoot {
                root_path,
                name,
                marketplace: record
                    .marketplace
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                source: "cache".to_string(),
                default_enabled: true,
                installed: Some(record),
            });
        }
        for dir in &config.dirs {
            let root_path = resolve_config_path(&self.home_dir, dir);
            if let Some(manifest) = read_plugin_manifest_object(&root_path) {
                let name = manifest
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                roots.push(PluginRoot {
                    root_path,
                    name,
                    marketplace: "inline".to_string(),
                    source: "inline".to_string(),
                    default_enabled: true,
                    installed: None,
                });
            }
        }
        roots
    }

    fn snapshot(&self) -> PluginListSnapshot {
        let config = self.plugin_config();
        let suppressed: BTreeSet<String> = config.suppressed_builtins.iter().cloned().collect();
        let mut plugins = Vec::new();
        let mut installed = Vec::new();
        let mut diagnostics = Vec::new();
        for root in self.roots() {
            let Some(manifest) = read_plugin_manifest_object(&root.root_path) else {
                diagnostics.push(PluginDiagnostic {
                    code: "plugin_manifest_invalid".to_string(),
                    message: format!(
                        "Plugin manifest is missing or invalid: {}",
                        root.root_path.display()
                    ),
                    severity: Some(PluginDiagnosticSeverity::Warning),
                    plugin_id: None,
                });
                continue;
            };
            let name = manifest
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or(&root.name)
                .to_string();
            let id = format!("{name}@{}", root.marketplace);
            let enabled = if suppressed.contains(&canonical_plugin_id(&id)) {
                false
            } else {
                config
                    .enabled_plugins
                    .get(&canonical_plugin_id(&id))
                    .copied()
                    .unwrap_or(root.default_enabled)
            };
            let components = scan_components(&root.root_path, &name);
            let info = build_plugin_info(&root, &manifest, &id, &name, enabled, &components);
            if let Some(record) = &root.installed {
                installed.push((record.clone(), info.clone()));
            }
            plugins.push(info);
        }
        PluginListSnapshot {
            plugins,
            installed,
            diagnostics,
        }
    }

    pub fn list(&self) -> PluginsListResult {
        let snapshot = self.snapshot();
        PluginsListResult {
            plugins: snapshot.plugins,
            diagnostics: snapshot.diagnostics,
        }
    }

    pub fn describe(&self, marketplace: &str, plugin_name: &str) -> PluginsDescribeResult {
        let Some(root) = self
            .roots()
            .into_iter()
            .find(|root| root.marketplace == marketplace && root.name == plugin_name)
        else {
            return PluginsDescribeResult {
                components: Vec::new(),
                diagnostics: Some(vec![PluginDiagnostic {
                    code: "plugin_marketplace_invalid".to_string(),
                    message: format!("Marketplace not found: {marketplace}"),
                    severity: Some(PluginDiagnosticSeverity::Error),
                    plugin_id: None,
                }]),
                metadata: None,
            };
        };
        let Some(manifest) = read_plugin_manifest_object(&root.root_path) else {
            return PluginsDescribeResult {
                components: Vec::new(),
                diagnostics: Some(vec![PluginDiagnostic {
                    code: "plugin_manifest_invalid".to_string(),
                    message: format!("Plugin manifest is invalid: {}", root.root_path.display()),
                    severity: Some(PluginDiagnosticSeverity::Error),
                    plugin_id: Some(format!("{plugin_name}@{marketplace}")),
                }]),
                metadata: None,
            };
        };
        let components = scan_components(&root.root_path, plugin_name);
        PluginsDescribeResult {
            components,
            // 观察：命中的 describe 不带 diagnostics 键（不是空数组）。
            diagnostics: None,
            metadata: Some(PluginDescribeMetadata {
                author: manifest
                    .get("author")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                author_url: None,
                homepage: None,
                version: manifest
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
        }
    }

    pub fn overview(&self) -> PluginsOverviewResult {
        let snapshot = self.snapshot();
        let marketplaces: Vec<PluginMarketplaceSummary> = self
            .known_marketplaces()
            .iter()
            .filter_map(marketplace_summary)
            .filter(|marketplace| is_safe_segment(&marketplace.id))
            .collect();
        let installed_ids: BTreeSet<String> = snapshot
            .installed
            .iter()
            .map(|(record, _)| record.id.clone())
            .collect();
        let mut available = Vec::new();
        for marketplace in &marketplaces {
            for entry in read_marketplace_plugins(&self.storage_root, &marketplace.id) {
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let id = format!("{name}@{}", marketplace.id);
                available.push(AvailablePluginSummary {
                    id: id.clone(),
                    name,
                    marketplace: marketplace.id.clone(),
                    description: entry
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    version: entry
                        .get("version")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    installed: installed_ids.contains(&id),
                    component_types: None,
                    listing: None,
                    bundled_unavailable: None,
                    source_unavailable: Some(false),
                });
            }
        }
        let installed_plugins = snapshot
            .installed
            .iter()
            .map(|(record, info)| installed_summary(record, info))
            .collect();
        PluginsOverviewResult {
            marketplaces,
            available_plugins: available,
            installed_plugins,
            restorable_builtins: Vec::new(),
            diagnostics: snapshot.diagnostics,
            official_marketplace_enabled: None,
            capability: PluginOverviewCapability {
                supported: true,
                reason: None,
            },
        }
    }

    pub fn set_enabled(
        &self,
        plugin_id: &str,
        enabled: bool,
    ) -> Result<PluginsSetEnabledResult, String> {
        let mut config = self.config();
        let plugins = config
            .entry("plugins".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(plugins) = plugins.as_object_mut() else {
            return Err("plugins config is not an object".to_string());
        };
        let enabled_plugins = plugins
            .entry("enabledPlugins".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(enabled_plugins) = enabled_plugins.as_object_mut() else {
            return Err("plugins.enabledPlugins is not an object".to_string());
        };
        enabled_plugins.insert(canonical_plugin_id(plugin_id), Value::Bool(enabled));
        self.write_config(&config)?;
        let snapshot = self.snapshot();
        let plugin = snapshot
            .plugins
            .into_iter()
            .find(|plugin| plugin.id == plugin_id)
            .ok_or_else(|| format!("Plugin not found: {plugin_id}"))?;
        Ok(PluginsSetEnabledResult { plugin, enabled })
    }

    pub fn reference_catalog(
        &self,
        authority: PluginReferenceAuthority,
    ) -> PluginsReferenceCatalogResult {
        let snapshot = self.snapshot();
        let plugins = snapshot
            .plugins
            .into_iter()
            .map(|plugin| {
                let marketplace = plugin.marketplace.clone();
                let name = plugin.name.clone();
                let skill_qualified_names =
                    scan_components(&PathBuf::from(&plugin.root_path), &name)
                        .into_iter()
                        .filter(|group| group.kind == PluginComponentKind::Skill)
                        .flat_map(|group| group.items.into_iter().map(|item| item.name))
                        .map(|skill| format!("{name}:{skill}"))
                        .collect();
                PluginReferenceCatalogEntry {
                    category: None,
                    plugin_id: plugin.id.clone(),
                    name,
                    marketplace,
                    icon: None,
                    display_name: None,
                    display_name_i18n: None,
                    description: plugin.description.clone(),
                    description_i18n: None,
                    enabled: plugin.enabled,
                    conflicting_plugin_ids: Vec::new(),
                    skill_qualified_names,
                    mcp_server_names: plugin.mcp_server_names.clone(),
                    subagent_names: Vec::new(),
                }
            })
            .collect();
        PluginsReferenceCatalogResult { authority, plugins }
    }

    pub fn resolve_suggested_reference(
        &self,
        stable_id: &str,
    ) -> PluginsResolveSuggestedReferenceResult {
        let Some((name, marketplace)) = stable_id.split_once('@') else {
            return PluginsResolveSuggestedReferenceResult {
                stable_id: stable_id.to_string(),
                status: PluginSuggestedReferenceStatus::Unavailable,
                marketplace: None,
                plugin_name: None,
                source_trust: None,
                icon: None,
                listing: None,
                diagnostics: vec![PluginDiagnostic {
                    code: "plugin_suggested_reference_untrusted_source".to_string(),
                    message: "推荐插件不是受信任的官方 mode-plugins-official 来源".to_string(),
                    severity: Some(PluginDiagnosticSeverity::Error),
                    plugin_id: Some(stable_id.to_string()),
                }],
            };
        };
        if marketplace != MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID {
            return PluginsResolveSuggestedReferenceResult {
                stable_id: stable_id.to_string(),
                status: PluginSuggestedReferenceStatus::Unavailable,
                marketplace: Some(marketplace.to_string()),
                plugin_name: Some(name.to_string()),
                source_trust: None,
                icon: None,
                listing: None,
                diagnostics: vec![PluginDiagnostic {
                    code: "plugin_suggested_reference_untrusted_source".to_string(),
                    message: "推荐插件不是受信任的官方 mode-plugins-official 来源".to_string(),
                    severity: Some(PluginDiagnosticSeverity::Error),
                    plugin_id: Some(stable_id.to_string()),
                }],
            };
        }
        let found = self
            .roots()
            .into_iter()
            .any(|root| root.marketplace == marketplace && root.name == name);
        let result = if found {
            PluginsResolveSuggestedReferenceResult {
                stable_id: stable_id.to_string(),
                status: PluginSuggestedReferenceStatus::Ready,
                marketplace: Some(marketplace.to_string()),
                plugin_name: Some(name.to_string()),
                source_trust: Some(SourceTrustOfficial::Official),
                icon: None,
                listing: None,
                diagnostics: Vec::new(),
            }
        } else {
            PluginsResolveSuggestedReferenceResult {
                stable_id: stable_id.to_string(),
                status: PluginSuggestedReferenceStatus::Unavailable,
                marketplace: Some(marketplace.to_string()),
                plugin_name: Some(name.to_string()),
                source_trust: None,
                icon: None,
                listing: None,
                diagnostics: vec![PluginDiagnostic {
                    code: "marketplace_refresh_failed".to_string(),
                    message: format!("Marketplace not found: {marketplace}"),
                    severity: Some(PluginDiagnosticSeverity::Error),
                    plugin_id: Some(stable_id.to_string()),
                }],
            }
        };
        result.tap_validate()
    }

    /// 已启用插件的 MCP server（供会话工具目录合并）。
    ///
    /// server 名用 `plugin:<pluginName>:<serverName>`（观察 `plugin:fixture:fixture`）；
    /// 工具名由 `McpToolDescriptor` 投影为 `mcp__plugin_<plugin>_<server>__<tool>`。
    pub fn session_mcp_servers(&self) -> Vec<PluginMcpConfig> {
        let snapshot = self.snapshot();
        let mut servers = Vec::new();
        for plugin in snapshot.plugins.iter().filter(|plugin| plugin.enabled) {
            let root = PathBuf::from(&plugin.root_path);
            let Some(document) = read_json_object(&root.join(".mcp.json")) else {
                continue;
            };
            let Some(entries) = document.get("mcpServers").and_then(Value::as_object) else {
                continue;
            };
            for (server_name, config) in entries {
                let name = format!("plugin:{}:{server_name}", plugin.name);
                if let Some(command) = config.get("command").and_then(Value::as_str) {
                    servers.push(PluginMcpConfig {
                        plugin_name: plugin.name.clone(),
                        server: McpServer::Stdio(McpServerStdio {
                            name,
                            command: command.to_string(),
                            args: string_list(config.get("args")),
                            env: entry_list(config.get("env")),
                            isolation: None,
                            protocol_version: None,
                            timeout_ms: None,
                        }),
                    });
                } else if let Some(url) = config.get("url").and_then(Value::as_str) {
                    let transport = match config.get("type").and_then(Value::as_str) {
                        Some("sse") => McpTransportType::Sse,
                        _ => McpTransportType::Http,
                    };
                    servers.push(PluginMcpConfig {
                        plugin_name: plugin.name.clone(),
                        server: McpServer::Http(McpServerHttp {
                            name,
                            transport,
                            url: url.to_string(),
                            headers: entry_list(config.get("headers")),
                            oauth: None,
                            isolation: None,
                            protocol_version: None,
                            timeout_ms: None,
                        }),
                    });
                }
            }
        }
        servers
    }

    // ── marketplace 写侧 ──
    pub fn marketplace_add(
        &self,
        source: &str,
    ) -> Result<PluginsMarketplaceMutationResult, String> {
        let source_path = resolve_config_path(&self.home_dir, source);
        let marketplace_file = source_path.join(MARKETPLACE_FILE);
        let manifest = read_json_object(&marketplace_file).ok_or_else(|| {
            format!(
                "Marketplace manifest not found: {}",
                marketplace_file.display()
            )
        })?;
        let name = manifest
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Marketplace manifest has no name".to_string())?
            .to_string();
        validate_segment(&name, "marketplace name")?;
        let marketplaces_root = self.storage_root.join("marketplaces");
        let target = marketplaces_root.join(&name);
        if !is_within(&marketplaces_root, &target) {
            return Err(format!("Marketplace path escapes storage root: {name:?}"));
        }
        copy_directory(&source_path, &target)?;
        let plugin_count = manifest
            .get("plugins")
            .and_then(Value::as_array)
            .map(|plugins| plugins.len() as u64)
            .unwrap_or(0);
        let now = iso8601_now();
        let entry = json!({
            "id": name,
            "source": { "source": "directory", "path": source_path.display().to_string() },
            "name": name,
            "addedAt": now,
            "lastUpdated": now,
            "pluginCount": plugin_count,
            "cacheTransactionId": uuid::Uuid::new_v4().to_string(),
        });
        let mut known = self.known_marketplace_document();
        {
            let entries = known
                .entry("marketplaces".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Some(entries) = entries.as_array_mut() else {
                return Err("known_marketplaces.json marketplaces is not an array".to_string());
            };
            entries.retain(|entry| entry.get("id").and_then(Value::as_str) != Some(name.as_str()));
            entries.push(entry.clone());
        }
        write_json_object(&self.storage_root.join(KNOWN_MARKETPLACES_FILE), &known)?;
        let summary = marketplace_summary(&entry)
            .ok_or_else(|| "failed to build marketplace summary".to_string())?;
        Ok(PluginsMarketplaceMutationResult {
            marketplace: Some(summary),
            marketplaces: None,
            diagnostics: Some(Vec::new()),
        })
    }

    pub fn marketplace_remove(
        &self,
        marketplace: &str,
    ) -> Result<PluginsMarketplaceMutationResult, String> {
        validate_segment(marketplace, "marketplace")?;
        let marketplaces_root = self.storage_root.join("marketplaces");
        let target = marketplaces_root.join(marketplace);
        if !is_within(&marketplaces_root, &target) {
            return Err(format!(
                "Marketplace path escapes storage root: {marketplace:?}"
            ));
        }
        let _ = fs::remove_dir_all(&target);
        let mut known = self.known_marketplace_document();
        if let Some(entries) = known.get_mut("marketplaces").and_then(Value::as_array_mut) {
            entries.retain(|entry| entry.get("id").and_then(Value::as_str) != Some(marketplace));
        }
        write_json_object(&self.storage_root.join(KNOWN_MARKETPLACES_FILE), &known)?;
        Ok(PluginsMarketplaceMutationResult {
            marketplace: None,
            marketplaces: None,
            diagnostics: Some(Vec::new()),
        })
    }

    pub fn marketplace_update(
        &self,
        marketplace: Option<&str>,
    ) -> Result<PluginsMarketplaceMutationResult, String> {
        let mut known = self.known_marketplace_document();
        let now = iso8601_now();
        {
            let entries = known
                .get_mut("marketplaces")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| {
                    "known_marketplaces.json marketplaces is not an array".to_string()
                })?;
            for entry in entries.iter_mut() {
                let id = entry.get("id").and_then(Value::as_str).unwrap_or_default();
                if marketplace.is_some_and(|wanted| wanted != id) {
                    continue;
                }
                // id 来自磁盘上的 known_marketplaces.json：非法段直接跳过，不拼路径。
                if !is_safe_segment(id) {
                    continue;
                }
                let Some(source_path) = entry
                    .get("source")
                    .and_then(|source| source.get("path"))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let source_path = PathBuf::from(source_path);
                let marketplaces_root = self.storage_root.join("marketplaces");
                let target = marketplaces_root.join(id);
                if !is_within(&marketplaces_root, &target) {
                    continue;
                }
                copy_directory(&source_path, &target)?;
                let plugin_count = read_json_object(&target.join(MARKETPLACE_FILE))
                    .and_then(|manifest| {
                        manifest
                            .get("plugins")
                            .and_then(Value::as_array)
                            .map(|plugins| plugins.len() as u64)
                    })
                    .unwrap_or(0);
                if let Some(object) = entry.as_object_mut() {
                    object.insert("lastUpdated".to_string(), Value::String(now.clone()));
                    object.insert(
                        "pluginCount".to_string(),
                        Value::Number(plugin_count.into()),
                    );
                }
            }
        }
        write_json_object(&self.storage_root.join(KNOWN_MARKETPLACES_FILE), &known)?;
        let marketplaces = known
            .get("marketplaces")
            .and_then(Value::as_array)
            .map(|entries| entries.iter().filter_map(marketplace_summary).collect())
            .unwrap_or_default();
        Ok(PluginsMarketplaceMutationResult {
            marketplace: None,
            marketplaces: Some(marketplaces),
            diagnostics: Some(Vec::new()),
        })
    }

    // ── 安装写侧 ──

    pub fn install(
        &self,
        plugin_name: &str,
        marketplace: &str,
        scope: Option<PluginScope>,
    ) -> Result<PluginsInstallResult, String> {
        validate_segment(plugin_name, "plugin name")?;
        validate_segment(marketplace, "marketplace")?;
        let marketplaces_root = self.storage_root.join("marketplaces");
        let marketplace_root = marketplaces_root.join(marketplace);
        if !is_within(&marketplaces_root, &marketplace_root) {
            return Err(format!(
                "Marketplace path escapes storage root: {marketplace:?}"
            ));
        }
        let entry = read_marketplace_plugins(&self.storage_root, marketplace)
            .into_iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(plugin_name))
            .ok_or_else(|| {
                format!("Plugin not found in marketplace: {plugin_name}@{marketplace}")
            })?;
        let relative = entry.get("source").and_then(Value::as_str).unwrap_or(".");
        let source = resolve_marketplace_source(&marketplace_root, relative);
        // marketplace.json 的 source 可能写成绝对路径或 `../../`：只允许快照目录内。
        if !is_within(&marketplace_root, &source) {
            return Err(format!(
                "Plugin source escapes marketplace: {relative:?} in {marketplace}"
            ));
        }
        let manifest = read_plugin_manifest_object(&source)
            .ok_or_else(|| format!("Plugin manifest not found: {}", source.display()))?;
        let version = manifest
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("0.0.0")
            .to_string();
        validate_segment(&version, "plugin version")?;
        let cache_root = self.storage_root.join("cache");
        let install_path = cache_root
            .join(marketplace)
            .join(plugin_name)
            .join(&version);
        if !is_within(&cache_root, &install_path) {
            return Err(format!(
                "Plugin cache path escapes storage root: {version:?}"
            ));
        }
        copy_directory(&source, &install_path)?;
        let now = iso8601_now();
        let id = format!("{plugin_name}@{marketplace}");
        let scope = scope.unwrap_or(PluginScope::User);
        let mut document = self.installed_document();
        let entries = document
            .entry("plugins".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(entries) = entries.as_array_mut() else {
            return Err("installed_plugins.json plugins is not an array".to_string());
        };
        entries.retain(|entry| entry.get("id").and_then(Value::as_str) != Some(id.as_str()));
        entries.push(json!({
            "id": id,
            "name": plugin_name,
            "marketplace": marketplace,
            "version": version,
            "installPath": install_path.display().to_string(),
            "installedAt": now,
            "updatedAt": now,
            "scope": plugin_scope_str(scope),
            "source": relative,
            "cacheTransactionId": uuid::Uuid::new_v4().to_string(),
        }));
        write_json_object(&self.storage_root.join(INSTALLED_PLUGINS_FILE), &document)?;
        self.set_enabled(&id, true)?;
        let summary = installed_summary_from(
            &id,
            plugin_name,
            marketplace,
            &version,
            &install_path,
            &now,
            scope,
            true,
        );
        Ok(PluginsInstallResult {
            dependency_closure: vec![id],
            installed_plugins: vec![summary],
            diagnostics: Vec::new(),
        })
    }

    pub fn uninstall(
        &self,
        plugin_id: Option<&str>,
        plugin_name: Option<&str>,
        marketplace: Option<&str>,
        remove_cache: Option<bool>,
    ) -> Result<PluginsUninstallResult, String> {
        let target = plugin_id
            .map(str::to_string)
            .or_else(|| match (plugin_name, marketplace) {
                (Some(name), Some(marketplace)) => Some(format!("{name}@{marketplace}")),
                _ => None,
            })
            .ok_or_else(|| "uninstall requires pluginId or pluginName+marketplace".to_string())?;
        let mut document = self.installed_document();
        let entries = document
            .get_mut("plugins")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "installed_plugins.json plugins is not an array".to_string())?;
        let Some(position) = entries
            .iter()
            .position(|entry| entry.get("id").and_then(Value::as_str) == Some(target.as_str()))
        else {
            return Err(format!("Plugin not installed: {target}"));
        };
        let record = entries.remove(position);
        write_json_object(&self.storage_root.join(INSTALLED_PLUGINS_FILE), &document)?;
        let _ = self.set_enabled(&target, false);
        let recorded_install_path = record
            .get("installPath")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // 记录文件可被篡改：只处理 cache 根内的路径，避免任意目录删除/扫描。
        let cache_root = self.storage_root.join("cache");
        let install_path = (!recorded_install_path.is_empty()
            && is_within(&cache_root, Path::new(&recorded_install_path)))
        .then_some(recorded_install_path);
        if remove_cache.unwrap_or(true) {
            if let Some(path) = &install_path {
                let _ = fs::remove_dir_all(path);
            }
        }
        let name = record
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| target.split('@').next().unwrap_or_default())
            .to_string();
        let marketplace = record
            .get("marketplace")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let version = record
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string);
        let installed_at = record
            .get("installedAt")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut summary = InstalledPluginSummary {
            id: target,
            name,
            marketplace,
            description: None,
            version,
            enabled: false,
            scope: PluginScope::User,
            install_path: install_path.clone(),
            installed_at,
            component_types: None,
            hook_details: None,
            update_status: None,
            latest_version: None,
            listing: None,
        };
        if let Some(path) = &summary.install_path {
            let components = scan_components(Path::new(path), &summary.name);
            summary.component_types = Some(component_types(&components));
        }
        Ok(PluginsUninstallResult {
            removed_plugin: Some(summary),
            diagnostics: Vec::new(),
        })
    }

    pub fn update(&self, params: &PluginsUpdateParams) -> Result<PluginsInstallResult, String> {
        let target = params
            .plugin_id
            .as_deref()
            .ok_or_else(|| "plugins/update requires pluginId".to_string())?;
        let (name, marketplace) = target
            .split_once('@')
            .ok_or_else(|| format!("Invalid plugin id: {target}"))?;
        self.install(
            name,
            marketplace,
            params.marketplace.as_ref().map(|_| PluginScope::User),
        )
    }

    fn known_marketplace_document(&self) -> Map<String, Value> {
        read_json_object(&self.storage_root.join(KNOWN_MARKETPLACES_FILE)).unwrap_or_else(|| {
            let mut document = Map::new();
            document.insert("version".to_string(), json!(1));
            document.insert("marketplaces".to_string(), Value::Array(Vec::new()));
            document
        })
    }

    fn installed_document(&self) -> Map<String, Value> {
        read_json_object(&self.storage_root.join(INSTALLED_PLUGINS_FILE)).unwrap_or_else(|| {
            let mut document = Map::new();
            document.insert("version".to_string(), json!(1));
            document.insert("plugins".to_string(), Value::Array(Vec::new()));
            document
        })
    }
}

trait ValidateTap {
    fn tap_validate(self) -> Self;
}

impl ValidateTap for PluginsResolveSuggestedReferenceResult {
    fn tap_validate(self) -> Self {
        if let Err(error) = self.validate() {
            return PluginsResolveSuggestedReferenceResult {
                status: PluginSuggestedReferenceStatus::Unavailable,
                source_trust: None,
                diagnostics: vec![PluginDiagnostic {
                    code: "plugin_suggested_reference_untrusted_source".to_string(),
                    message: error,
                    severity: Some(PluginDiagnosticSeverity::Error),
                    plugin_id: Some(self.stable_id.clone()),
                }],
                ..self
            };
        }
        self
    }
}

/// 已启用插件贡献的一个 MCP server 声明（`plugin_name` 用于 `process/childProcesses`）。
pub struct PluginMcpConfig {
    pub plugin_name: String,
    pub server: McpServer,
}

fn read_marketplace_plugins(storage_root: &Path, marketplace: &str) -> Vec<Value> {
    let path = storage_root
        .join("marketplaces")
        .join(marketplace)
        .join(MARKETPLACE_FILE);
    read_json_object(&path)
        .and_then(|manifest| manifest.get("plugins").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

fn marketplace_summary(value: &Value) -> Option<PluginMarketplaceSummary> {
    let object = value.as_object()?;
    let id = object.get("id")?.as_str()?.to_string();
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();
    let source = object
        .get("source")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    Some(PluginMarketplaceSummary {
        id,
        name,
        source,
        description: object
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        last_updated: object
            .get("lastUpdated")
            .and_then(Value::as_str)
            .map(str::to_string),
        plugin_count: object
            .get("pluginCount")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        is_official: Some(false),
        featured: None,
        refresh_failure: None,
    })
}

fn build_plugin_info(
    root: &PluginRoot,
    manifest: &Map<String, Value>,
    id: &str,
    name: &str,
    enabled: bool,
    components: &[PluginComponentGroup],
) -> PluginInfo {
    let declared_mcp_server_names: Vec<String> = read_mcp_server_names(&root.root_path);
    PluginInfo {
        id: id.to_string(),
        name: name.to_string(),
        description: manifest
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        version: manifest
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string),
        enabled,
        source: root.source.clone(),
        marketplace: root.marketplace.clone(),
        author: manifest
            .get("author")
            .and_then(Value::as_str)
            .map(str::to_string),
        author_url: None,
        homepage: None,
        skill_count: components
            .iter()
            .find(|group| group.kind == PluginComponentKind::Skill)
            .map(|group| group.items.len() as u64),
        skill_root_count: u64::from(root.root_path.join("skills").is_dir()),
        command_root_count: u64::from(root.root_path.join("commands").is_dir()),
        components: Some(components.to_vec()),
        declared_mcp_server_names: Some(declared_mcp_server_names.clone()),
        host_mcp_server_names: None,
        mcp_server_names: declared_mcp_server_names
            .iter()
            .map(|server| format!("plugin:{name}:{server}"))
            .collect(),
        hook_details: Some(Vec::new()),
        root_path: root.root_path.display().to_string(),
        user_config: None,
        configured_options: None,
        package_status: None,
        root_source: None,
        enabled_source: None,
        option_sources: None,
    }
}

fn scan_components(root: &Path, plugin_name: &str) -> Vec<PluginComponentGroup> {
    let mut groups = Vec::new();
    let commands = scan_markdown_dir(&root.join("commands"));
    if !commands.is_empty() {
        groups.push(PluginComponentGroup {
            kind: PluginComponentKind::Command,
            items: commands,
        });
    }
    let skills = scan_skill_dirs(&root.join("skills"));
    if !skills.is_empty() {
        groups.push(PluginComponentGroup {
            kind: PluginComponentKind::Skill,
            items: skills,
        });
    }
    let mcp = read_mcp_server_names(root);
    if !mcp.is_empty() {
        groups.push(PluginComponentGroup {
            kind: PluginComponentKind::Mcp,
            items: mcp
                .into_iter()
                .map(|name| PluginComponentItem {
                    name,
                    description: None,
                })
                .collect(),
        });
    }
    let _ = plugin_name;
    groups
}

fn scan_markdown_dir(dir: &Path) -> Vec<PluginComponentItem> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        let description = fs::read_to_string(&path)
            .ok()
            .map(|raw| skills_service::read_frontmatter(&raw).description)
            .filter(|description| !description.is_empty());
        items.push(PluginComponentItem { name, description });
    }
    items.sort_by(|left, right| plugin_discovery::js::locale_compare(&left.name, &right.name));
    items
}

fn scan_skill_dirs(dir: &Path) -> Vec<PluginComponentItem> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("SKILL.md");
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let frontmatter = skills_service::read_frontmatter(&raw);
        let name = if frontmatter.name.is_empty() {
            entry.file_name().to_string_lossy().into_owned()
        } else {
            frontmatter.name
        };
        items.push(PluginComponentItem {
            name,
            description: Some(frontmatter.description).filter(|value| !value.is_empty()),
        });
    }
    items.sort_by(|left, right| plugin_discovery::js::locale_compare(&left.name, &right.name));
    items
}

fn read_mcp_server_names(root: &Path) -> Vec<String> {
    let Some(document) = read_json_object(&root.join(".mcp.json")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = document
        .get("mcpServers")
        .and_then(Value::as_object)
        .map(|servers| servers.keys().cloned().collect())
        .unwrap_or_default();
    names.sort();
    names
}

fn component_types(components: &[PluginComponentGroup]) -> Vec<String> {
    components
        .iter()
        .map(|group| match group.kind {
            PluginComponentKind::Agent => "agent".to_string(),
            PluginComponentKind::Command => "command".to_string(),
            PluginComponentKind::Skill => "skill".to_string(),
            PluginComponentKind::Hook => "hook".to_string(),
            PluginComponentKind::Mcp => "mcp".to_string(),
        })
        .collect()
}

fn installed_summary(record: &InstalledPluginRecord, info: &PluginInfo) -> InstalledPluginSummary {
    InstalledPluginSummary {
        id: record.id.clone(),
        name: info.name.clone(),
        marketplace: info.marketplace.clone(),
        description: info.description.clone(),
        version: info.version.clone(),
        enabled: info.enabled,
        scope: plugin_scope_from_str(record.scope.as_deref()),
        install_path: Some(record.install_path.clone()),
        installed_at: None,
        component_types: info
            .components
            .as_ref()
            .map(|groups| component_types(groups)),
        hook_details: Some(Vec::new()),
        update_status: None,
        latest_version: None,
        listing: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn installed_summary_from(
    id: &str,
    name: &str,
    marketplace: &str,
    version: &str,
    install_path: &Path,
    installed_at: &str,
    scope: PluginScope,
    enabled: bool,
) -> InstalledPluginSummary {
    InstalledPluginSummary {
        id: id.to_string(),
        name: name.to_string(),
        marketplace: marketplace.to_string(),
        description: None,
        version: Some(version.to_string()),
        enabled,
        scope,
        install_path: Some(install_path.display().to_string()),
        installed_at: Some(installed_at.to_string()),
        component_types: Some(component_types(&scan_components(install_path, name))),
        hook_details: Some(Vec::new()),
        update_status: None,
        latest_version: None,
        listing: None,
    }
}

fn plugin_scope_str(scope: PluginScope) -> &'static str {
    match scope {
        PluginScope::User => "user",
        PluginScope::Workspace => "workspace",
    }
}

fn plugin_scope_from_str(value: Option<&str>) -> PluginScope {
    match value {
        Some("workspace") => PluginScope::Workspace,
        _ => PluginScope::User,
    }
}

fn resolve_marketplace_source(marketplace_root: &Path, source: &str) -> PathBuf {
    let candidate = marketplace_root.join(source);
    plugin_discovery::js::normalize_lexical(&candidate)
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `.mcp.json` 的 `env`/`headers`：对象字面量（观察形态）或 `[{name,value}]`。
fn entry_list(value: Option<&Value>) -> Vec<McpEntry> {
    match value {
        Some(Value::Object(object)) => {
            let mut entries: Vec<McpEntry> = object
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|value| McpEntry {
                        name: name.clone(),
                        value: value.to_string(),
                    })
                })
                .collect();
            entries.sort_by(|left, right| left.name.cmp(&right.name));
            entries
        }
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                Some(McpEntry {
                    name: item.get("name")?.as_str()?.to_string(),
                    value: item.get("value")?.as_str()?.to_string(),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn read_storage_dir(config_path: &Path) -> String {
    read_json_object(config_path)
        .and_then(|config| {
            config
                .get("storage")
                .and_then(|storage| storage.get("dir"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "~/.mode".to_string())
}

fn read_json_object(path: &Path) -> Option<Map<String, Value>> {
    let raw = fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&raw).ok()? {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

fn write_json_object(path: &Path, object: &Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(object).map_err(|error| error.to_string())?;
    fs::write(path, format!("{text}\n")).map_err(|error| error.to_string())
}

fn copy_directory(source: &Path, target: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!("Not a directory: {}", source.display()));
    }
    let _ = fs::remove_dir_all(target);
    fs::create_dir_all(target).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            copy_directory(&from, &to)?;
        } else if entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            fs::copy(&from, &to)
                .map_err(|error| format!("copy {} failed: {error}", from.display()))?;
        }
    }
    Ok(())
}

fn iso8601_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    data_root::iso8601_utc(now)
}
