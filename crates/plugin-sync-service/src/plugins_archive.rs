//! 插件归档导出/导入（`exportPluginsArchive` / `importPluginsArchive`）。

use std::path::{Path, PathBuf};

use crate::archive::{
    extract_archive, ArchiveMetadata, ArchiveMetadataPlugin, PLUGIN_SYNC_METADATA_ARCHIVE_PATH,
};
use crate::candidates::{
    collect_configured_inline_plugin_path_by_id, normalize_plugin_id, normalize_plugin_id_key,
    read_plugin_manifest_info,
};
use crate::paths::{
    resolve_user_path, resolve_within, user_mode_config_path, user_mode_plugin_root,
};
use crate::types::{PluginSyncImportResult, PluginSyncImportResultItem, PluginSyncImportStatus};

pub fn read_archive_metadata(temp_root: &Path) -> Result<ArchiveMetadata, String> {
    let raw = std::fs::read_to_string(temp_root.join(PLUGIN_SYNC_METADATA_ARCHIVE_PATH))
        .map_err(|error| error.to_string())?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    let Some(object) = parsed.as_object() else {
        return Err("invalid plugin sync archive metadata".to_string());
    };
    let Some(items) = object.get("plugins").and_then(serde_json::Value::as_array) else {
        return Err("invalid plugin sync archive metadata".to_string());
    };
    let mut plugins = Vec::with_capacity(items.len());
    for item in items {
        let Some(item) = item.as_object() else {
            return Err("invalid plugin sync archive plugin metadata".to_string());
        };
        let name = required_string(item.get("name"), "plugin metadata name")?;
        let plugin_id = normalize_plugin_id(&required_string(
            item.get("pluginId"),
            "plugin metadata id",
        )?)?;
        let directory_name = crate::paths::normalize_relative_path(&required_string(
            item.get("directoryName"),
            "plugin metadata directory",
        )?)?;
        plugins.push(ArchiveMetadataPlugin {
            name,
            plugin_id,
            directory_name,
            enabled: item.get("enabled").and_then(serde_json::Value::as_bool),
        });
    }
    Ok(ArchiveMetadata { plugins })
}

pub fn import_plugins_archive(
    archive: &[u8],
    home: &Path,
    max_archive_bytes: u64,
) -> Result<PluginSyncImportResult, String> {
    let temp_root: PathBuf = std::env::temp_dir().join(format!(
        "mode-plugin-sync-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let outcome = (|| -> Result<PluginSyncImportResult, String> {
        extract_archive(archive, &temp_root, Some(max_archive_bytes))?;
        let metadata = read_archive_metadata(&temp_root)?;
        let target_root = user_mode_plugin_root(home);
        let mut existing = collect_configured_inline_plugin_path_by_id(home)?;
        let mut results = Vec::with_capacity(metadata.plugins.len());
        for plugin in &metadata.plugins {
            results.push(import_one(
                plugin,
                &temp_root,
                &target_root,
                home,
                &mut existing,
            ));
        }
        Ok(PluginSyncImportResult { results })
    })();
    let _ = std::fs::remove_dir_all(&temp_root);
    outcome
}

fn import_one(
    plugin: &ArchiveMetadataPlugin,
    temp_root: &Path,
    target_root: &Path,
    home: &Path,
    existing: &mut Vec<(String, String)>,
) -> PluginSyncImportResultItem {
    let fallback = PluginSyncImportResultItem {
        name: plugin.name.clone(),
        plugin_id: plugin.plugin_id.clone(),
        directory_name: plugin.directory_name.clone(),
        status: PluginSyncImportStatus::Failed,
        path: None,
        error: None,
    };
    let outcome = (|| -> Result<PluginSyncImportResultItem, String> {
        let directory_name = crate::paths::normalize_relative_path(&plugin.directory_name)?;
        let plugin_id = normalize_plugin_id(&plugin.plugin_id)?;
        let extracted_path = resolve_within(temp_root, &directory_name)?;
        let target_path = resolve_within(target_root, &directory_name)?;
        let manifest = read_plugin_manifest_info(&extracted_path)?
            .ok_or_else(|| format!("plugin manifest not found: {directory_name}"))?;
        if normalize_plugin_id_key(&manifest.plugin_id)? != normalize_plugin_id_key(&plugin_id)? {
            return Err(format!(
                "plugin manifest id mismatch: {} !== {plugin_id}",
                manifest.plugin_id
            ));
        }
        if std::fs::symlink_metadata(&target_path).is_ok() {
            return Ok(PluginSyncImportResultItem {
                name: manifest.name,
                plugin_id,
                directory_name,
                status: PluginSyncImportStatus::Skipped,
                path: Some(target_path.to_string_lossy().into_owned()),
                error: None,
            });
        }
        let key = normalize_plugin_id_key(&plugin_id)?;
        if let Some((_, existing_path)) = existing.iter().find(|(id, _)| id == &key) {
            return Ok(PluginSyncImportResultItem {
                name: manifest.name,
                plugin_id,
                directory_name,
                status: PluginSyncImportStatus::Skipped,
                path: Some(existing_path.clone()),
                error: None,
            });
        }
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let staging_path = resolve_within(
            target_root,
            &format!(
                ".importing-{}-{}",
                key.replace(['@', '.'], "-"),
                unique_suffix()
            ),
        )?;
        copy_recursive(&extracted_path, &staging_path)?;
        if std::fs::symlink_metadata(&target_path).is_ok() {
            let _ = std::fs::remove_dir_all(&staging_path);
            return Err(format!(
                "plugin target already exists: {}",
                target_path.display()
            ));
        }
        if let Err(error) = std::fs::rename(&staging_path, &target_path) {
            let _ = std::fs::remove_dir_all(&staging_path);
            return Err(error.to_string());
        }
        if let Err(error) =
            add_plugin_dir_to_user_config(home, &target_path, &plugin_id, plugin.enabled)
        {
            let _ = std::fs::remove_dir_all(&target_path);
            return Err(error);
        }
        existing.push((key, target_path.to_string_lossy().into_owned()));
        Ok(PluginSyncImportResultItem {
            name: manifest.name,
            plugin_id,
            directory_name,
            status: PluginSyncImportStatus::Synced,
            path: Some(target_path.to_string_lossy().into_owned()),
            error: None,
        })
    })();
    outcome.unwrap_or_else(|error| PluginSyncImportResultItem {
        error: Some(error),
        ..fallback
    })
}

fn add_plugin_dir_to_user_config(
    home: &Path,
    plugin_path: &Path,
    plugin_id: &str,
    enabled_override: Option<bool>,
) -> Result<(), String> {
    let file_path = user_mode_config_path(home);
    let mut parsed = read_json_object_or_empty(&file_path)?;
    let mut plugins = parsed
        .get("plugins")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let resolved_plugin_path = resolve_user_path(&plugin_path.to_string_lossy());
    let mut dirs: Vec<serde_json::Value> = plugins
        .get("dirs")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let already_present = dirs.iter().any(|item| {
        item.as_str()
            .map(|value| resolve_user_path(value) == resolved_plugin_path)
            .unwrap_or(false)
    });
    if !already_present {
        dirs.push(serde_json::Value::String(
            resolved_plugin_path.to_string_lossy().into_owned(),
        ));
    }
    plugins.insert("dirs".to_string(), serde_json::Value::Array(dirs));

    let mut enabled_plugins = plugins
        .get("enabledPlugins")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    match enabled_override {
        Some(enabled) => {
            enabled_plugins.insert(plugin_id.to_string(), serde_json::Value::Bool(enabled));
            plugins.insert(
                "enabledPlugins".to_string(),
                serde_json::Value::Object(enabled_plugins),
            );
        }
        None if enabled_plugins.is_empty() => {
            plugins.remove("enabledPlugins");
        }
        None => {
            plugins.insert(
                "enabledPlugins".to_string(),
                serde_json::Value::Object(enabled_plugins),
            );
        }
    }
    parsed.insert("plugins".to_string(), serde_json::Value::Object(plugins));
    write_json_file_atomic(&file_path, &serde_json::Value::Object(parsed))
}

fn read_json_object_or_empty(
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

fn write_json_file_atomic(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp_path = path.with_extension(format!(
        "json.{}.{}.tmp",
        std::process::id(),
        unique_suffix()
    ));
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    std::fs::write(&temp_path, text).map_err(|error| error.to_string())?;
    std::fs::rename(&temp_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        error.to_string()
    })
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

fn required_string(value: Option<&serde_json::Value>, label: &str) -> Result<String, String> {
    let text = value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing {label}"))?;
    Ok(text.to_string())
}

fn unique_suffix() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
        .to_string()
}
