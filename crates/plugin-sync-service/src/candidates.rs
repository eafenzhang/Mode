//! 用户级插件候选扫描（`collectLocalUserPluginCandidates` 与配置读取）。

use std::path::Path;

use plugin_discovery::js;
use sha2::{Digest, Sha256};

use crate::paths::{resolve_user_path, user_mode_config_path};
use crate::types::{PluginSyncCandidate, PluginSyncComponentType};

const INLINE_PLUGIN_MARKETPLACE: &str = "inline";
const PLUGIN_MANIFEST_RELATIVE_PATHS: [[&str; 2]; 4] = [
    [".mode-plugin", "plugin.json"],
    [".zcode-plugin", "plugin.json"],
    [".claude-plugin", "plugin.json"],
    [".codex-plugin", "plugin.json"],
];

#[derive(Debug, Clone)]
pub struct PluginManifestInfo {
    pub name: String,
    pub plugin_id: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub component_types: Vec<PluginSyncComponentType>,
}

#[derive(Debug, Clone, Default)]
pub struct UserPluginConfigState {
    pub dirs: Vec<String>,
    pub enabled_overrides: Vec<(String, bool)>,
}

impl UserPluginConfigState {
    pub fn enabled_override(&self, plugin_id_key: &str) -> Option<bool> {
        self.enabled_overrides
            .iter()
            .find(|(key, _)| key == plugin_id_key)
            .map(|(_, value)| *value)
    }
}

pub fn read_user_plugin_config_state(home: &Path) -> Result<UserPluginConfigState, String> {
    let config_path = user_mode_config_path(home);
    let parsed = read_json_file_or_empty(&config_path)?;
    let plugins = parsed
        .get("plugins")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut enabled_overrides = Vec::new();
    if let Some(enabled) = plugins
        .get("enabledPlugins")
        .and_then(serde_json::Value::as_object)
    {
        for (plugin_id, value) in enabled {
            if let Some(enabled) = value.as_bool() {
                enabled_overrides.push((normalize_plugin_id_key(plugin_id)?, enabled));
            }
        }
    }
    let dirs = plugins
        .get("dirs")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(UserPluginConfigState {
        dirs,
        enabled_overrides,
    })
}

pub fn collect_local_user_plugin_candidates(
    home: &Path,
) -> Result<Vec<PluginSyncCandidate>, String> {
    let config = read_user_plugin_config_state(home)?;
    let mut candidates = Vec::new();
    let mut seen_realpaths: Vec<String> = Vec::new();
    let mut used_directory_names: Vec<String> = Vec::new();

    for raw_dir in &config.dirs {
        let plugin_root = resolve_user_path(raw_dir);
        let canonical_root = js::realpath(&plugin_root).unwrap_or_else(|| plugin_root.clone());
        let canonical_key = canonical_root.to_string_lossy().into_owned();
        if seen_realpaths.iter().any(|seen| seen == &canonical_key) {
            continue;
        }
        seen_realpaths.push(canonical_key.clone());
        let Some(manifest) = read_plugin_manifest_info(&canonical_root)? else {
            continue;
        };
        let enabled_override =
            config.enabled_override(&normalize_plugin_id_key(&manifest.plugin_id)?);
        let directory_name = create_unique_directory_name(
            &canonical_root,
            &manifest.name,
            &canonical_root,
            &mut used_directory_names,
        );
        candidates.push(PluginSyncCandidate {
            id: create_short_hash(&format!("{}:{canonical_key}", manifest.plugin_id)),
            name: manifest.name.clone(),
            plugin_id: manifest.plugin_id.clone(),
            directory_name,
            description: manifest.description.clone(),
            version: manifest.version.clone(),
            path: canonical_key,
            size_bytes: compute_recursive_size(&canonical_root),
            enabled: enabled_override.unwrap_or(true),
            enabled_override,
            component_types: manifest.component_types,
        });
    }

    candidates.sort_by(|left, right| js::locale_compare(&left.name, &right.name));
    Ok(candidates)
}

/// 配置里已登记插件的 pluginId → 路径（首个命中，key 小写）。
pub fn collect_configured_inline_plugin_path_by_id(
    home: &Path,
) -> Result<Vec<(String, String)>, String> {
    let config = read_user_plugin_config_state(home)?;
    let mut result: Vec<(String, String)> = Vec::new();
    for raw_dir in &config.dirs {
        let plugin_root = resolve_user_path(raw_dir);
        let canonical_root = js::realpath(&plugin_root).unwrap_or_else(|| plugin_root.clone());
        let Some(manifest) = read_plugin_manifest_info(&canonical_root)? else {
            continue;
        };
        let key = normalize_plugin_id_key(&manifest.plugin_id)?;
        if !result.iter().any(|(existing, _)| existing == &key) {
            result.push((key, canonical_root.to_string_lossy().into_owned()));
        }
    }
    Ok(result)
}

pub fn read_plugin_manifest_info(plugin_root: &Path) -> Result<Option<PluginManifestInfo>, String> {
    for segments in PLUGIN_MANIFEST_RELATIVE_PATHS {
        let manifest_path = plugin_root.join(segments[0]).join(segments[1]);
        let Some(raw) = js::read_text(&manifest_path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Ok(None);
        };
        let Some(object) = parsed.as_object() else {
            return Ok(None);
        };
        let name = object
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if name.is_empty() {
            return Ok(None);
        }
        let description = object
            .get("description")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let version = object
            .get("version")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        return Ok(Some(PluginManifestInfo {
            name: name.to_string(),
            plugin_id: format!("{name}@{INLINE_PLUGIN_MARKETPLACE}"),
            description,
            version,
            component_types: resolve_component_types(plugin_root, object),
        }));
    }
    Ok(None)
}

fn resolve_component_types(
    plugin_root: &Path,
    manifest: &serde_json::Map<String, serde_json::Value>,
) -> Vec<PluginSyncComponentType> {
    let mut types = Vec::new();
    if manifest.contains_key("skills") || js::exists(&plugin_root.join("skills")) {
        types.push(PluginSyncComponentType::Skills);
    }
    if manifest.contains_key("commands") || js::exists(&plugin_root.join("commands")) {
        types.push(PluginSyncComponentType::Commands);
    }
    if manifest.contains_key("hooks") || js::exists(&plugin_root.join("hooks").join("hooks.json")) {
        types.push(PluginSyncComponentType::Hooks);
    }
    if manifest.contains_key("mcpServers") || js::exists(&plugin_root.join(".mcp.json")) {
        types.push(PluginSyncComponentType::Mcp);
    }
    types
}

pub fn compute_recursive_size(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.file_type().is_file() {
        return metadata.len();
    }
    if !metadata.file_type().is_dir() {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| compute_recursive_size(&entry.path()))
        .sum()
}

fn create_unique_directory_name(
    directory_basename: &Path,
    plugin_name: &str,
    plugin_root: &Path,
    used: &mut Vec<String>,
) -> String {
    let base = directory_basename
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(plugin_name);
    let base_name = to_safe_directory_name(base);
    if !used.iter().any(|value| value == &base_name) {
        used.push(base_name.clone());
        return base_name;
    }
    let suffix = create_short_hash(&plugin_root.to_string_lossy());
    let deduped =
        to_safe_directory_name(&format!("{base_name}-{}", &suffix[..8.min(suffix.len())]));
    used.push(deduped.clone());
    deduped
}

fn to_safe_directory_name(value: &str) -> String {
    let sanitized: String = value
        .trim()
        .chars()
        .map(|ch| {
            if matches!(ch, '\\' | '/' | ':') {
                '-'
            } else {
                ch
            }
        })
        .collect();
    let sanitized = if sanitized.is_empty() {
        "plugin".to_string()
    } else {
        sanitized
    };
    match crate::paths::normalize_relative_path(&sanitized) {
        Ok(value) => value,
        Err(_) => {
            let hash = create_short_hash(value);
            format!("plugin-{}", &hash[..8.min(hash.len())])
        }
    }
}

pub fn normalize_plugin_id(plugin_id: &str) -> Result<String, String> {
    let normalized = plugin_id.trim();
    if normalized.is_empty() || normalized.contains('/') || normalized.contains('\\') {
        return Err(format!("invalid plugin id: {plugin_id}"));
    }
    Ok(normalized.to_string())
}

pub fn normalize_plugin_id_key(plugin_id: &str) -> Result<String, String> {
    Ok(normalize_plugin_id(plugin_id)?.to_lowercase())
}

pub fn create_short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn read_json_file_or_empty(
    path: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(serde_json::Value::Object(object)) => Ok(object),
            _ => Err(format!("invalid json object: {}", path.display())),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(error) => Err(error.to_string()),
    }
}
