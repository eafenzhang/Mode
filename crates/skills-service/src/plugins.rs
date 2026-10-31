//! 插件来源的技能根解析。
//!
//! 插件配置、市场 id 归一、缓存根回退与安装记录读取由 `plugin-discovery` 共用；
//! 本模块只保留技能域特有的「插件 → 技能根目录」投影。
//!
//! 真源：`skillsService.ts` 的 `resolvePluginSkillRootDescriptors` /
//! `readPluginManifest` / `resolvePluginSkillRoots` / `resolveInside`。

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::read_cli_config;
use crate::fsx;
use crate::paths::{SkillRootDescriptor, SkillScope};
use plugin_discovery::{
    canonical_plugin_id, is_default_enabled_official_plugin_id, is_valid_plugin_name,
    read_installed_plugin_records, read_plugin_config, read_plugin_manifest_object,
    resolve_config_path, resolve_plugin_storage_root, MODE_INLINE_PLUGIN_MARKETPLACE,
    MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID,
};

#[derive(Debug, Clone)]
pub(crate) struct PluginRootCandidate {
    pub default_enabled: bool,
    pub marketplace: String,
    pub root_path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct PluginManifest {
    pub name: String,
    pub skills: Option<Value>,
}

pub(crate) fn resolve_plugin_skill_root_descriptors(home_dir: &Path) -> Vec<SkillRootDescriptor> {
    let config = read_cli_config(home_dir);
    let plugin_config = read_plugin_config(&config);
    if !plugin_config.enabled {
        return Vec::new();
    }

    let plugin_storage_root = resolve_plugin_storage_root(home_dir, &plugin_config.storage_dir);
    let official_cache_roots = plugin_discovery::official_cache_version_roots(&plugin_storage_root);
    let installed_roots = read_installed_plugin_roots(&plugin_storage_root);

    let mut candidates: Vec<PluginRootCandidate> = plugin_config
        .dirs
        .iter()
        .map(|dir| PluginRootCandidate {
            default_enabled: true,
            marketplace: MODE_INLINE_PLUGIN_MARKETPLACE.to_string(),
            root_path: resolve_config_path(home_dir, dir),
        })
        .collect();
    candidates.extend(
        official_cache_roots
            .into_iter()
            .map(|root_path| PluginRootCandidate {
                default_enabled: false,
                marketplace: MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID.to_string(),
                root_path,
            }),
    );
    candidates.extend(installed_roots);

    let mut descriptors: Vec<SkillRootDescriptor> = Vec::new();
    let mut seen_plugin_ids: Vec<String> = Vec::new();

    for candidate in candidates {
        let Some(manifest) = read_plugin_manifest(&candidate.root_path) else {
            continue;
        };
        let plugin_id = format!("{}@{}", manifest.name, candidate.marketplace);
        // 内置官方插件被「卸载」后只在 CLI config 写入 suppressedBuiltins；
        // desktop 直扫官方 cache 不经过 CLI 过滤，这里必须同样跳过，否则仍会贡献技能。
        if candidate.marketplace == MODE_OFFICIAL_PLUGIN_MARKETPLACE_ID
            && plugin_config
                .suppressed_builtins
                .iter()
                .any(|id| id == &plugin_id)
        {
            continue;
        }
        if seen_plugin_ids.iter().any(|id| id == &plugin_id) {
            continue;
        }
        seen_plugin_ids.push(plugin_id.clone());
        let default_enabled =
            candidate.default_enabled || is_default_enabled_official_plugin_id(&plugin_id);
        let enabled = plugin_config
            .enabled_plugins
            .get(&plugin_id)
            .copied()
            .unwrap_or(default_enabled);
        if !enabled {
            continue;
        }
        for root_path in resolve_plugin_skill_roots(&manifest, &candidate.root_path) {
            descriptors.push(SkillRootDescriptor {
                scope: SkillScope::Plugin,
                root_path,
                plugin_name: Some(manifest.name.clone()),
                plugin_id: Some(plugin_id.clone()),
            });
        }
    }

    descriptors
}

fn read_installed_plugin_roots(plugin_storage_root: &Path) -> Vec<PluginRootCandidate> {
    let mut roots = Vec::new();
    for record in read_installed_plugin_records(plugin_storage_root) {
        // skills 侧口径：三个字段先 trim 再判空，installPath 必须是绝对路径。
        let id = fsx::js_trim(&record.id);
        let marketplace = record
            .marketplace
            .as_deref()
            .map(fsx::js_trim)
            .unwrap_or("");
        let install_path = fsx::js_trim(&record.install_path);
        if id.is_empty() || marketplace.is_empty() || install_path.is_empty() {
            continue;
        }
        if !fsx::is_absolute(install_path) {
            continue;
        }
        let canonical_id = canonical_plugin_id(id);
        let canonical_marketplace = match canonical_id.rfind('@') {
            Some(index) => &canonical_id[index + 1..],
            None => canonical_id.as_str(),
        };
        roots.push(PluginRootCandidate {
            default_enabled: false,
            marketplace: if canonical_marketplace.is_empty() {
                marketplace.to_string()
            } else {
                canonical_marketplace.to_string()
            },
            root_path: PathBuf::from(install_path),
        });
    }
    roots
}

fn read_plugin_manifest(root_path: &Path) -> Option<PluginManifest> {
    let object = read_plugin_manifest_object(root_path)?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(fsx::js_trim)
        .unwrap_or("")
        .to_string();
    if !is_valid_plugin_name(&name) {
        return None;
    }
    Some(PluginManifest {
        name,
        skills: object.get("skills").cloned(),
    })
}

fn parse_path_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(text)) => vec![text.clone()],
        Some(other) => match other.as_array() {
            Some(items) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            None => Vec::new(),
        },
        None => Vec::new(),
    }
}

pub(crate) fn resolve_plugin_skill_roots(
    manifest: &PluginManifest,
    root_path: &Path,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for raw in parse_path_list(manifest.skills.as_ref()) {
        if let Some(resolved) = resolve_inside(root_path, &raw) {
            roots.push(resolved);
        }
    }
    if roots.is_empty() && manifest.skills.is_none() {
        let default_root = root_path.join("skills");
        if fsx::exists(&default_root) {
            roots.push(default_root);
        }
    }
    roots
}

fn resolve_inside(root_path: &Path, raw_path: &str) -> Option<PathBuf> {
    if fsx::is_absolute(raw_path) {
        return None;
    }
    let resolved = fsx::resolve_join(root_path, Path::new(raw_path));
    if fsx::contains_path(root_path, &resolved) {
        Some(resolved)
    } else {
        None
    }
}
