//! marketplace source 归档：闭包解析、本地 source 镜像与导入。

use std::path::{Path, PathBuf};

use plugin_discovery::js;
use serde_json::{Map, Value};

use crate::archive::{
    extract_archive, ArchiveEntry, ArchiveMetadataPlugin, PLUGIN_SYNC_METADATA_ARCHIVE_PATH,
};
use crate::candidates::{create_short_hash, read_plugin_manifest_info};
use crate::paths::{normalize_relative_path, resolve_user_path, resolve_within, user_mode_plugin_root};
use crate::types::{PluginSyncMarketplaceImportResult, PluginSyncMarketplaceImportStatus};

pub const MARKETPLACE_SOURCE_ROOT_DIRECTORY: &str = "marketplace-sources";
const MIRRORED_MARKETPLACE_PLUGIN_ROOT: &str = "plugins";

#[derive(Debug, Clone)]
pub struct MarketplacePluginEntry {
    pub name: String,
    pub dependencies: Vec<String>,
    pub raw: Map<String, Value>,
    pub source: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct MarketplaceManifest {
    pub name: String,
    pub description: Option<String>,
    pub plugin_root: Option<String>,
    pub plugins: Vec<MarketplacePluginEntry>,
    pub raw: Map<String, Value>,
}

struct LoadedSource {
    manifest: MarketplaceManifest,
    source_root: Option<PathBuf>,
}

struct MirrorEntry {
    entry: MarketplacePluginEntry,
    mirrored_source_path: Option<PathBuf>,
    mirrored_source_relative_path: Option<String>,
}

pub struct MarketplaceExport {
    pub archive: Vec<u8>,
    pub marketplace_id: String,
    pub plugin_names: Vec<String>,
}

pub fn export_marketplace_source_archive(
    marketplace_id: &str,
    plugin_names: &[String],
    source: &Value,
    max_archive_bytes: u64,
) -> Result<MarketplaceExport, String> {
    let marketplace_id = normalize_marketplace_id(marketplace_id)?;
    let mut names: Vec<String> = Vec::new();
    for name in plugin_names {
        let name = required_string(name, "plugin name")?;
        if !names.iter().any(|existing| existing == &name) {
            names.push(name);
        }
    }
    if names.is_empty() {
        return Err("missing marketplace plugin names".to_string());
    }
    let loaded = load_marketplace_source(source)?;
    if loaded.manifest.name != marketplace_id {
        return Err(format!(
            "marketplace id mismatch: {} !== {marketplace_id}",
            loaded.manifest.name
        ));
    }
    let closure = resolve_closure(&loaded.manifest, &marketplace_id, &names)?;
    let mirror_entries: Vec<MirrorEntry> = closure
        .iter()
        .map(|entry| build_mirror_entry(&loaded, entry))
        .collect::<Result<_, _>>()?;
    let manifest = build_mirrored_manifest(&loaded.manifest, &mirror_entries);
    let manifest_content = format!(
        "{}\n",
        serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?
    );
    let mut selected_bytes = manifest_content.len() as u64;
    for entry in &mirror_entries {
        if let Some(path) = &entry.mirrored_source_path {
            selected_bytes += crate::candidates::compute_recursive_size(path);
        }
    }
    if selected_bytes > max_archive_bytes {
        return Err(format!(
            "plugin marketplace source archive exceeds limit: {selected_bytes}/{max_archive_bytes}"
        ));
    }
    let fingerprints: Vec<Option<String>> = mirror_entries
        .iter()
        .map(|entry| {
            entry
                .mirrored_source_path
                .as_deref()
                .map(compute_content_fingerprint)
                .transpose()
        })
        .collect::<Result<_, _>>()?;
    let directory_name = create_source_directory_name(&marketplace_id, &manifest_content, &fingerprints);
    let mut entries: Vec<ArchiveEntry> = vec![ArchiveEntry::Content {
        archive_path: format!("{directory_name}/marketplace.json"),
        content: manifest_content.into_bytes(),
        mode: None,
        mtime_ms: None,
    }];
    for entry in &mirror_entries {
        if let (Some(path), Some(relative)) =
            (&entry.mirrored_source_path, &entry.mirrored_source_relative_path)
        {
            entries.push(ArchiveEntry::Source {
                source_path: path.clone(),
                archive_path: format!("{directory_name}/{relative}"),
            });
        }
    }
    let metadata = ArchiveMetadataWithSources {
        plugins: Vec::<ArchiveMetadataPlugin>::new(),
        marketplace_sources: vec![MarketplaceSourceMetadata {
            marketplace_id: marketplace_id.clone(),
            directory_name: directory_name.clone(),
        }],
    };
    let archive = create_archive_with_sources(&entries, &metadata)?;
    if archive.len() as u64 > max_archive_bytes {
        return Err(format!(
            "plugin marketplace source archive exceeds limit: {}/{max_archive_bytes}",
            archive.len()
        ));
    }
    Ok(MarketplaceExport {
        archive,
        marketplace_id,
        plugin_names: closure.iter().map(|entry| entry.name.clone()).collect(),
    })
}

pub fn import_marketplace_source_archive(
    archive: &[u8],
    home: &Path,
    max_archive_bytes: u64,
) -> Result<PluginSyncMarketplaceImportResult, String> {
    let temp_root = std::env::temp_dir().join(format!(
        "mode-plugin-marketplace-source-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let outcome = (|| -> Result<PluginSyncMarketplaceImportResult, String> {
        extract_archive(archive, &temp_root, Some(max_archive_bytes))?;
        let metadata = read_marketplace_source_metadata(&temp_root)?;
        let directory_name = normalize_relative_path(&metadata.1)?;
        let extracted_path = resolve_within(&temp_root, &directory_name)?;
        let manifest = read_marketplace_manifest(&extracted_path.join("marketplace.json"))?;
        if manifest.name != metadata.0 {
            return Err(format!(
                "marketplace source archive id mismatch: {} !== {}",
                manifest.name, metadata.0
            ));
        }
        let target_root = user_mode_plugin_root(home).join(MARKETPLACE_SOURCE_ROOT_DIRECTORY);
        let target_path = resolve_within(&target_root, &directory_name)?;
        if js::exists(&target_path) {
            return Ok(PluginSyncMarketplaceImportResult {
                marketplace_id: metadata.0,
                path: target_path.to_string_lossy().into_owned(),
                status: PluginSyncMarketplaceImportStatus::Skipped,
            });
        }
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        copy_recursive(&extracted_path, &target_path)?;
        Ok(PluginSyncMarketplaceImportResult {
            marketplace_id: metadata.0,
            path: target_path.to_string_lossy().into_owned(),
            status: PluginSyncMarketplaceImportStatus::Synced,
        })
    })();
    let _ = std::fs::remove_dir_all(&temp_root);
    outcome
}

fn load_marketplace_source(source: &Value) -> Result<LoadedSource, String> {
    let object = source
        .as_object()
        .ok_or_else(|| "marketplace manifest is invalid".to_string())?;
    let kind = object.get("source").and_then(Value::as_str).unwrap_or("");
    match kind {
        "file" => {
            let file_path = required_value_string(object.get("path"), "marketplace source file path")?;
            let path = resolve_user_path(&file_path);
            Ok(LoadedSource {
                manifest: read_marketplace_manifest(&path)?,
                source_root: path.parent().map(Path::to_path_buf),
            })
        }
        "directory" => {
            let source_root = resolve_user_path(&required_value_string(
                object.get("path"),
                "marketplace source directory path",
            )?);
            let manifest_path = find_marketplace_manifest_path(&source_root)
                .ok_or_else(|| format!("marketplace manifest not found in directory: {}", source_root.display()))?;
            Ok(LoadedSource {
                manifest: read_marketplace_manifest(&manifest_path)?,
                source_root: Some(source_root),
            })
        }
        "settings" => {
            let marketplace = object
                .get("marketplace")
                .ok_or_else(|| "settings marketplace source is missing marketplace manifest".to_string())?;
            Ok(LoadedSource {
                manifest: parse_marketplace_manifest(marketplace)?,
                source_root: None,
            })
        }
        other => Err(format!(
            "unsupported marketplace source for SSH sync: {}",
            if other.is_empty() { "unknown" } else { other }
        )),
    }
}

fn read_marketplace_manifest(path: &Path) -> Result<MarketplaceManifest, String> {
    let raw = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    parse_marketplace_manifest(&parsed)
}

fn parse_marketplace_manifest(value: &Value) -> Result<MarketplaceManifest, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "marketplace manifest is invalid".to_string())?;
    let name = required_value_string(object.get("name"), "marketplace name")?;
    let plugin_values: Vec<Map<String, Value>> = match object.get("plugins") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_object)
            .cloned()
            .collect(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(plugin_name, plugin)| {
                let mut entry = plugin.as_object().cloned().unwrap_or_default();
                entry.insert("name".to_string(), Value::String(plugin_name.clone()));
                entry
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut plugins = Vec::with_capacity(plugin_values.len());
    for plugin in plugin_values {
        let plugin_name = required_value_string(plugin.get("name"), "marketplace plugin name")?;
        let dependencies = plugin
            .get("dependencies")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        plugins.push(MarketplacePluginEntry {
            name: plugin_name,
            dependencies,
            source: plugin.get("source").cloned(),
            raw: plugin,
        });
    }
    let plugin_root = read_plugin_root(object);
    Ok(MarketplaceManifest {
        name,
        description: object.get("description").and_then(Value::as_str).map(str::to_string),
        plugin_root,
        plugins,
        raw: object.clone(),
    })
}

fn read_plugin_root(object: &Map<String, Value>) -> Option<String> {
    if let Some(value) = object.get("pluginRoot").and_then(Value::as_str) {
        if !value.trim().is_empty() {
            return Some(value.trim().to_string());
        }
    }
    object
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("pluginRoot"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
}

fn find_marketplace_manifest_path(source_root: &Path) -> Option<PathBuf> {
    [
        source_root.join("marketplace.json"),
        source_root.join(".claude-plugin").join("marketplace.json"),
    ]
    .into_iter()
    .find(|candidate| js::exists(candidate))
}

fn resolve_closure(
    manifest: &MarketplaceManifest,
    marketplace_id: &str,
    plugin_names: &[String],
) -> Result<Vec<MarketplacePluginEntry>, String> {
    let mut visited: Vec<String> = Vec::new();
    let mut result: Vec<MarketplacePluginEntry> = Vec::new();
    fn visit(
        plugin_name: &str,
        required_by: &str,
        manifest: &MarketplaceManifest,
        marketplace_id: &str,
        visited: &mut Vec<String>,
        result: &mut Vec<MarketplacePluginEntry>,
    ) -> Result<(), String> {
        let normalized = required_string(plugin_name, "marketplace plugin name")?;
        if visited.iter().any(|name| name == &normalized) {
            return Ok(());
        }
        let entry = manifest
            .plugins
            .iter()
            .find(|entry| entry.name == normalized)
            .ok_or_else(|| format!("marketplace plugin not found: {normalized}"))?
            .clone();
        visited.push(normalized);
        result.push(entry.clone());
        for dependency in &entry.dependencies {
            let name = normalize_dependency(dependency, marketplace_id, required_by)?;
            visit(&name, &entry.name, manifest, marketplace_id, visited, result)?;
        }
        Ok(())
    }
    for name in plugin_names {
        visit(name, name, manifest, marketplace_id, &mut visited, &mut result)?;
    }
    Ok(result)
}

fn normalize_dependency(
    dependency: &str,
    marketplace_id: &str,
    required_by: &str,
) -> Result<String, String> {
    let trimmed = dependency.trim();
    let separator = trimmed.rfind('@');
    let Some(index) = separator else {
        return Ok(trimmed.to_string());
    };
    let name = &trimmed[..index];
    let marketplace = &trimmed[index + 1..];
    if marketplace == marketplace_id {
        Ok(name.to_string())
    } else {
        Err(format!(
            "cross-marketplace dependency is not supported for SSH source mirror: {dependency} required by {required_by}"
        ))
    }
}

fn build_mirror_entry(
    loaded: &LoadedSource,
    entry: &MarketplacePluginEntry,
) -> Result<MirrorEntry, String> {
    if is_remote_marketplace_source(entry.source.as_ref()) {
        return Ok(MirrorEntry {
            entry: entry.clone(),
            mirrored_source_path: None,
            mirrored_source_relative_path: None,
        });
    }
    let source_path = resolve_local_marketplace_plugin_source(loaded, entry)?;
    let directory_name = to_safe_directory_name(&entry.name);
    Ok(MirrorEntry {
        entry: entry.clone(),
        mirrored_source_path: Some(source_path),
        mirrored_source_relative_path: Some(format!(
            "{MIRRORED_MARKETPLACE_PLUGIN_ROOT}/{directory_name}"
        )),
    })
}

fn is_remote_marketplace_source(source: Option<&Value>) -> bool {
    let Some(object) = source.and_then(Value::as_object) else {
        return false;
    };
    matches!(
        object.get("source").and_then(Value::as_str).unwrap_or(""),
        "github" | "git" | "url" | "git-subdir" | "npm" | "pip"
    )
}

fn resolve_local_marketplace_plugin_source(
    loaded: &LoadedSource,
    entry: &MarketplacePluginEntry,
) -> Result<PathBuf, String> {
    match &entry.source {
        Some(Value::String(source)) => resolve_local_source_path(loaded, source, &format!("{} source", entry.name), false),
        Some(Value::Object(object)) => {
            let kind = object.get("source").and_then(Value::as_str).unwrap_or("");
            if kind == "directory" {
                let path = object
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("missing {} directory source", entry.name))?;
                resolve_local_source_path(
                    loaded,
                    path,
                    &format!("{} directory source", entry.name),
                    loaded.source_root.is_none(),
                )
            } else {
                resolve_local_plugin_fallback(loaded, entry)
            }
        }
        _ => resolve_local_plugin_fallback(loaded, entry),
    }
}

fn resolve_local_plugin_fallback(
    loaded: &LoadedSource,
    entry: &MarketplacePluginEntry,
) -> Result<PathBuf, String> {
    let Some(source_root) = &loaded.source_root else {
        return Err(format!(
            "cannot mirror local marketplace plugin source without source root: {}",
            entry.name
        ));
    };
    let _ = source_root;
    let base = resolve_marketplace_plugin_base_dir(loaded)?;
    let fallback = resolve_within(&base, &entry.name).map_err(|_| {
        format!("unsafe marketplace plugin source path: {}", entry.name)
    })?;
    if fallback.is_dir() {
        assert_plugin_source_root(&fallback)
    } else {
        Err(format!(
            "local marketplace plugin source does not exist: {}",
            fallback.display()
        ))
    }
}

fn resolve_local_source_path(
    loaded: &LoadedSource,
    source_path: &str,
    label: &str,
    allow_settings_absolute: bool,
) -> Result<PathBuf, String> {
    let requested = required_string(source_path, label)?;
    let relative = requested.strip_prefix("./").unwrap_or(&requested);
    if loaded.source_root.is_none() && allow_settings_absolute {
        if !Path::new(&requested).is_absolute() {
            return Err(format!(
                "cannot mirror local marketplace plugin source without source root: {source_path}"
            ));
        }
        let absolute = resolve_user_path(&requested);
        return if absolute.is_dir() {
            assert_plugin_source_root(&absolute)
        } else {
            Err(format!(
                "local marketplace plugin source does not exist: {source_path}"
            ))
        };
    }
    let Some(source_root) = &loaded.source_root else {
        return Err(format!(
            "cannot mirror local marketplace plugin source without source root: {source_path}"
        ));
    };
    let _ = source_root;
    let base = resolve_marketplace_plugin_base_dir(loaded)?;
    let relative_path = resolve_within(&base, relative)
        .map_err(|_| format!("unsafe marketplace plugin source path: {source_path}"))?;
    if relative_path.is_dir() {
        assert_plugin_source_root(&relative_path)
    } else {
        Err(format!(
            "local marketplace plugin source does not exist: {source_path}"
        ))
    }
}

fn assert_plugin_source_root(path: &Path) -> Result<PathBuf, String> {
    match read_plugin_manifest_info(path)? {
        Some(_) => Ok(path.to_path_buf()),
        None => Err(format!(
            "local marketplace plugin source is missing plugin manifest: {}",
            path.display()
        )),
    }
}

fn resolve_marketplace_plugin_base_dir(loaded: &LoadedSource) -> Result<PathBuf, String> {
    let Some(source_root) = &loaded.source_root else {
        return Err("cannot resolve marketplace plugin source root".to_string());
    };
    let Some(plugin_root) = &loaded.manifest.plugin_root else {
        return Ok(source_root.clone());
    };
    let joined = resolve_within(source_root, plugin_root)
        .map_err(|_| format!("unsafe marketplace pluginRoot: {plugin_root}"))?;
    Ok(if js::exists(&joined) {
        joined
    } else {
        source_root.clone()
    })
}

fn build_mirrored_manifest(
    manifest: &MarketplaceManifest,
    entries: &[MirrorEntry],
) -> Map<String, Value> {
    let has_mirrored = entries
        .iter()
        .any(|entry| entry.mirrored_source_relative_path.is_some());
    let raw_metadata = manifest
        .raw
        .get("metadata")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut manifest_object = manifest.raw.clone();
    manifest_object.remove("pluginRoot");
    manifest_object.insert("name".to_string(), Value::String(manifest.name.clone()));
    if let Some(description) = &manifest.description {
        manifest_object.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
    }
    if has_mirrored {
        let mut metadata = raw_metadata;
        metadata.insert(
            "pluginRoot".to_string(),
            Value::String(MIRRORED_MARKETPLACE_PLUGIN_ROOT.to_string()),
        );
        manifest_object.insert("metadata".to_string(), Value::Object(metadata));
    }
    let plugins: Vec<Value> = entries
        .iter()
        .map(|entry| {
            let mut raw = entry.entry.raw.clone();
            raw.insert("name".to_string(), Value::String(entry.entry.name.clone()));
            if let Some(relative) = &entry.mirrored_source_relative_path {
                let trimmed = relative
                    .split_once('/')
                    .map(|(_, rest)| rest)
                    .unwrap_or(relative.as_str());
                raw.insert("source".to_string(), Value::String(format!("./{trimmed}")));
            }
            Value::Object(raw)
        })
        .collect();
    manifest_object.insert("plugins".to_string(), Value::Array(plugins));
    manifest_object
}

fn create_source_directory_name(
    marketplace_id: &str,
    manifest_content: &str,
    fingerprints: &[Option<String>],
) -> String {
    let hash_input = serde_json::to_string(&serde_json::json!({
        "manifestContent": manifest_content,
        "sourceFingerprints": fingerprints
    }))
    .unwrap_or_default();
    format!(
        "{}-{}",
        to_safe_directory_name(marketplace_id),
        &create_short_hash(&hash_input)[..12]
    )
}

fn compute_content_fingerprint(path: &Path) -> Result<String, String> {
    fn visit(path: &Path, hasher: &mut sha2::Sha256) -> Result<(), String> {
        use sha2::Digest;
        let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_file() {
            hasher.update(b"file\0");
            let content = std::fs::read(path).map_err(|error| error.to_string())?;
            hasher.update(content);
            return Ok(());
        }
        if !metadata.file_type().is_dir() {
            return Err(format!(
                "unsupported plugin archive source: {}",
                path.display()
            ));
        }
        hasher.update(b"dir\0");
        let mut children: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        children.sort_by(|left, right| {
            js::locale_compare(
                &left.file_name().unwrap_or_default().to_string_lossy(),
                &right.file_name().unwrap_or_default().to_string_lossy(),
            )
        });
        for child in children {
            hasher.update(child.file_name().unwrap_or_default().to_string_lossy().as_bytes());
            hasher.update(b"\0");
            visit(&child, hasher)?;
            hasher.update(b"\0");
        }
        Ok(())
    }
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    visit(path, &mut hasher)?;
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

fn read_marketplace_source_metadata(temp_root: &Path) -> Result<(String, String), String> {
    let raw = std::fs::read_to_string(temp_root.join(PLUGIN_SYNC_METADATA_ARCHIVE_PATH))
        .map_err(|error| error.to_string())?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    let object = parsed
        .as_object()
        .ok_or_else(|| "invalid plugin marketplace source archive metadata".to_string())?;
    let sources = object
        .get("marketplaceSources")
        .and_then(Value::as_array)
        .ok_or_else(|| "invalid plugin marketplace source archive metadata".to_string())?;
    if sources.len() != 1 {
        return Err("plugin marketplace source archive must contain one marketplace".to_string());
    }
    let item = sources[0]
        .as_object()
        .ok_or_else(|| "invalid plugin marketplace source metadata".to_string())?;
    let marketplace_id = normalize_marketplace_id(&required_value_string(
        item.get("marketplaceId"),
        "marketplace source id",
    )?)?;
    let directory_name = normalize_relative_path(&required_value_string(
        item.get("directoryName"),
        "marketplace source directory",
    )?)?;
    Ok((marketplace_id, directory_name))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MarketplaceSourceMetadata {
    marketplace_id: String,
    directory_name: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveMetadataWithSources {
    plugins: Vec<ArchiveMetadataPlugin>,
    marketplace_sources: Vec<MarketplaceSourceMetadata>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MetadataEnvelope {
    plugins: Vec<ArchiveMetadataPlugin>,
    marketplace_sources: Vec<MarketplaceSourceMetadata>,
}

fn create_archive_with_sources(
    entries: &[ArchiveEntry],
    metadata: &ArchiveMetadataWithSources,
) -> Result<Vec<u8>, String> {
    let value = serde_json::to_value(MetadataEnvelope {
        plugins: metadata.plugins.clone(),
        marketplace_sources: metadata.marketplace_sources.iter().map(|source| MarketplaceSourceMetadata {
            marketplace_id: source.marketplace_id.clone(),
            directory_name: source.directory_name.clone(),
        }).collect(),
    })
    .map_err(|error| error.to_string())?;
    create_archive_with_metadata_value(entries, value)
}

fn create_archive_with_metadata_value(entries: &[ArchiveEntry], metadata: Value) -> Result<Vec<u8>, String> {
    // `create_archive` 的 metadata 类型是固定结构；marketplaceSources 通过临时写盘不可行，
    // 这里复用底层 tar 写入：把 metadata JSON 当成内容条目追加。
    crate::archive::create_archive_with_metadata_value(entries, metadata)
}

fn copy_recursive(source: &Path, target: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if metadata.file_type().is_dir() {
        std::fs::create_dir_all(target).map_err(|error| error.to_string())?;
        for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            copy_recursive(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if !metadata.file_type().is_file() {
        return Err(format!(
            "unsupported plugin archive source: {}",
            source.display()
        ));
    }
    std::fs::copy(source, target).map_err(|error| error.to_string())?;
    Ok(())
}

fn normalize_marketplace_id(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains('\\') {
        return Err(format!("invalid marketplace id: {value}"));
    }
    Ok(trimmed.to_string())
}

fn to_safe_directory_name(value: &str) -> String {
    let sanitized: String = value
        .trim()
        .chars()
        .map(|ch| if matches!(ch, '\\' | '/' | ':') { '-' } else { ch })
        .collect();
    let sanitized = if sanitized.is_empty() {
        "plugin".to_string()
    } else {
        sanitized
    };
    normalize_relative_path(&sanitized).unwrap_or_else(|_| {
        let hash = create_short_hash(value);
        format!("plugin-{}", &hash[..8])
    })
}

fn required_string(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(format!("missing {label}"))
    } else {
        Ok(trimmed.to_string())
    }
}

fn required_value_string(value: Option<&Value>, label: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .map(|value| required_string(value, label))
        .unwrap_or_else(|| Err(format!("missing {label}")))
}

fn unique_suffix() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
        .to_string()
}
