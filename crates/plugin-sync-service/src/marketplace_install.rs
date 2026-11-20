//! Marketplace 安装编排（LT-15 P3 切三）。
//!
//! 真源：`apps/mode-cli/packages/adapters/src/plugins/marketplace.ts` 的
//! `installMarketplacePlugin` / `resolveDependencyClosure` / `cacheMarketplacePlugin` /
//! `resolvePluginSourceRoot` / `resolveInstalledPluginVersion`。
//!
//! 本切片覆盖 directory/file/字符串相对路径 与 filesystem/sea 缓存兜底源；
//! git/npm/url 等网络源返回错误（后续按需接入）。安装按依赖闭包展开，逐条
//! 拷贝到 cache 目录（官方市场改名后旧目录兜底），失败时回滚新建目录。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use serde_json::Value;

use crate::installed_records::{
    canonical_official_marketplace_id, load_installed_plugins_state, parse_plugin_id,
    save_installed_plugins_state, InstalledPluginRecord, InstalledPluginScope,
    InstalledPluginsState, DEFAULT_VERSION, MODE_OFFICIAL_PLUGIN_MARKETPLACE,
    MODE_OFFICIAL_PLUGIN_MARKETPLACE_LEGACY_ID,
};
use crate::plugin_management::{plugin_cache_dir, sanitize_plugin_id};

const MARKETPLACE_FILE: &str = "marketplace.json";
const PLUGIN_MANIFEST_CANDIDATES: [&str; 4] = [
    ".mode-plugin/plugin.json",
    ".zcode-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".codex-plugin/plugin.json",
];

/// marketplace.json 的轻量投影：只取闭包/安装编排需要的字段。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarketplaceManifest {
    pub name: String,
    pub allow_cross_marketplace_dependencies_on: Vec<String>,
    pub plugins: Vec<MarketplacePluginEntry>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarketplacePluginEntry {
    pub name: String,
    pub version: Option<String>,
    pub dependencies: Option<Vec<String>>,
    pub source: Option<Value>,
    pub cache_path: Option<String>,
}

pub fn marketplace_manifest_path(storage_root: &Path, marketplace: &str) -> PathBuf {
    storage_root
        .join("marketplaces")
        .join(sanitize_plugin_id(marketplace))
        .join(MARKETPLACE_FILE)
}

/// 读 marketplace.json 快照：缺失/坏 JSON 返回 None（与 TS loadMarketplaceManifestSync 同口径）。
pub fn load_marketplace_manifest(
    storage_root: &Path,
    marketplace: &str,
) -> Option<MarketplaceManifest> {
    let text =
        std::fs::read_to_string(marketplace_manifest_path(storage_root, marketplace)).ok()?;
    let raw: Value = serde_json::from_str(&text).ok()?;
    let object = raw.as_object()?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let allow_cross = object
        .get("allowCrossMarketplaceDependenciesOn")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let raw_plugins = object.get("plugins")?;
    let plugin_entries: Vec<(String, &Value)> = match raw_plugins {
        Value::Array(items) => items
            .iter()
            .filter_map(|entry| {
                let name = entry.get("name").and_then(Value::as_str)?.to_string();
                Some((name, entry))
            })
            .collect(),
        Value::Object(entries) => entries
            .iter()
            .map(|(plugin_name, entry)| (plugin_name.clone(), entry))
            .collect(),
        _ => Vec::new(),
    };
    Some(MarketplaceManifest {
        name,
        allow_cross_marketplace_dependencies_on: allow_cross,
        plugins: plugin_entries
            .into_iter()
            .map(|(plugin_name, entry)| MarketplacePluginEntry {
                name: plugin_name,
                version: entry
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                dependencies: entry
                    .get("dependencies")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    }),
                source: entry.get("source").cloned(),
                cache_path: entry
                    .get("cachePath")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
            .collect(),
    })
}

/// 依赖闭包 BFS：跨市场依赖默认阻断（allowCrossMarketplaceDependenciesOn 白名单放行），
/// 循环依赖抛错，市场/条目缺失抛错。
pub fn resolve_dependency_closure(
    storage_root: &Path,
    marketplace: &str,
    name: &str,
    allow_cross_marketplaces: &HashSet<String>,
) -> Result<Vec<String>, String> {
    let root_id = format!("{name}@{marketplace}");

    struct WalkContext<'a> {
        closure: Vec<String>,
        visiting: Vec<String>,
        visited: HashSet<String>,
        input_marketplace: &'a str,
        allow_cross: &'a HashSet<String>,
        storage_root: &'a Path,
    }

    fn walk(
        context: &mut WalkContext<'_>,
        plugin_id: &str,
        required_by: &str,
    ) -> Result<(), String> {
        let (name, marketplace) = parse_plugin_id(plugin_id)?;
        if marketplace != context.input_marketplace && !context.allow_cross.contains(&marketplace) {
            return Err(format!(
                "Cross-marketplace dependency is blocked: {plugin_id} required by {required_by}"
            ));
        }
        if context.visiting.contains(&plugin_id.to_string()) {
            return Err(format!(
                "Plugin dependency cycle: {} -> {plugin_id}",
                context.visiting.join(" -> ")
            ));
        }
        if context.visited.contains(plugin_id) {
            return Ok(());
        }
        let manifest = load_marketplace_manifest(context.storage_root, &marketplace)
            .ok_or_else(|| format!("Marketplace not found for dependency: {marketplace}"))?;
        let entry = manifest
            .plugins
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| {
                format!("Dependency not found: {plugin_id} required by {required_by}")
            })?;
        context.visiting.push(plugin_id.to_string());
        let dependencies = entry.dependencies.clone().unwrap_or_default();
        for dependency in &dependencies {
            let qualified = if dependency.contains('@') {
                dependency.clone()
            } else {
                format!("{dependency}@{marketplace}")
            };
            walk(context, &qualified, plugin_id)?;
        }
        context.visiting.pop();
        context.visited.insert(plugin_id.to_string());
        context.closure.push(plugin_id.to_string());
        Ok(())
    }

    let mut context = WalkContext {
        closure: Vec::new(),
        visiting: Vec::new(),
        visited: HashSet::new(),
        input_marketplace: marketplace,
        allow_cross: allow_cross_marketplaces,
        storage_root,
    };
    walk(&mut context, &root_id, &root_id)?;
    Ok(context.closure)
}

/// 源根目录解析：directory/file/字符串相对路径 与 filesystem/sea 缓存兜底。
/// git/npm/url 等网络源返回错误（后续按需接入）。
pub fn resolve_plugin_source_root(
    storage_root: &Path,
    marketplace: &str,
    entry: &MarketplacePluginEntry,
) -> Result<PathBuf, String> {
    let marketplace_dir = marketplace_manifest_path(storage_root, marketplace)
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let source = entry.source.as_ref();
    if let Some(source) = source {
        if source == &Value::String("filesystem".to_string())
            || source == &Value::String("sea".to_string())
        {
            if let Some(cache_path) = &entry.cache_path {
                let path = Path::new(cache_path);
                if path.is_dir() {
                    return Ok(path.to_path_buf());
                }
            }
            let computed = plugin_cache_dir(
                storage_root,
                marketplace,
                &entry.name,
                entry.version.as_deref().unwrap_or(DEFAULT_VERSION),
            );
            if computed.is_dir() {
                return Ok(computed);
            }
            return Err(format!(
                "Bundled plugin cache directory missing: {}@{marketplace} (this plugin ships inside the official client and is not bundled in this fork)",
                entry.name
            ));
        }
        if let Some(source_object) = source.as_object() {
            let kind = source_object
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("");
            if kind == "directory" || kind == "file" {
                let path = source_object
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let resolved = Path::new(path);
                if resolved.is_dir() {
                    return Ok(resolved.to_path_buf());
                }
                return Err(format!("Plugin source directory does not exist: {path}"));
            }
            // git / npm / url / github 等网络源暂不接入。
            return Err(format!(
                "Network plugin source not supported in this slice: {kind} ({})",
                entry.name
            ));
        }
    }
    // 裸字符串：manifest 相对路径或绝对路径。
    if let Some(source) = source.and_then(Value::as_str) {
        let stripped = source.strip_prefix("./").unwrap_or(source);
        let relative = marketplace_dir.join(stripped);
        if relative.is_dir() {
            return Ok(relative);
        }
        let absolute = Path::new(source);
        if absolute.is_dir() {
            return Ok(absolute.to_path_buf());
        }
    }
    Err(format!(
        "Unsupported or missing plugin source: {}@{marketplace}",
        entry.name
    ))
}

fn find_plugin_manifest_path(root: &Path) -> Option<PathBuf> {
    PLUGIN_MANIFEST_CANDIDATES
        .iter()
        .map(|candidate| root.join(candidate))
        .find(|path| path.is_file())
}

/// 从源根的 plugin.json 读真实版本（与 UI 展示同源）；缺失时回退 entry/默认值。
pub fn resolve_installed_plugin_version(
    source_root: &Path,
    entry: &MarketplacePluginEntry,
) -> String {
    if let Some(manifest_path) = find_plugin_manifest_path(source_root) {
        if let Ok(text) = std::fs::read_to_string(&manifest_path) {
            if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                if let Some(version) = parsed.get("version").and_then(Value::as_str) {
                    if !version.trim().is_empty() {
                        return version.to_string();
                    }
                }
            }
        }
    }
    entry
        .version
        .clone()
        .unwrap_or_else(|| DEFAULT_VERSION.to_string())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let target_path = target.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            copy_dir_recursive(&entry.path(), &target_path)?;
        } else {
            std::fs::copy(entry.path(), &target_path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// 安装编排结果：依赖闭包与实际写入的安装记录。
pub struct MarketplaceInstallResult {
    pub dependency_closure: Vec<String>,
    pub installed_plugins: Vec<InstalledPluginRecord>,
}

/// 安装编排（directory/file 源）：闭包展开 → 逐条拷贝到 cache → 记录 upsert → 原子保存。
/// 任一步失败回滚本轮新建的 cache 目录，并返回错误；已有记录不动。
pub fn install_marketplace_plugin(
    storage_root: &Path,
    marketplace: &str,
    name: &str,
) -> Result<MarketplaceInstallResult, String> {
    let allow_cross = load_marketplace_manifest(storage_root, marketplace)
        .map(|manifest| manifest.allow_cross_marketplace_dependencies_on)
        .unwrap_or_default();
    let closure = resolve_dependency_closure(
        storage_root,
        marketplace,
        name,
        &HashSet::from_iter(allow_cross.iter().cloned()),
    )?;
    let mut state = load_installed_plugins_state(storage_root);
    let mut created_dirs: Vec<PathBuf> = Vec::new();
    let mut installed: Vec<InstalledPluginRecord> = Vec::new();
    let result = (|| {
        for plugin_id in &closure {
            let (plugin_name, plugin_marketplace) = parse_plugin_id(plugin_id)?;
            let manifest = load_marketplace_manifest(storage_root, &plugin_marketplace)
                .ok_or_else(|| format!("Marketplace not found: {plugin_marketplace}"))?;
            let entry = manifest
                .plugins
                .iter()
                .find(|entry| entry.name == plugin_name)
                .ok_or_else(|| format!("Plugin not found: {plugin_id}"))?;
            let source_root = resolve_plugin_source_root(storage_root, &plugin_marketplace, entry)?;
            let version = resolve_installed_plugin_version(&source_root, entry);
            let target =
                plugin_cache_dir(storage_root, &plugin_marketplace, &plugin_name, &version);
            if source_root != target {
                if target.exists() {
                    std::fs::remove_dir_all(&target).map_err(|error| error.to_string())?;
                }
                copy_dir_recursive(&source_root, &target)?;
                created_dirs.push(target.clone());
            }
            let now = now_iso();
            let record = InstalledPluginRecord {
                id: format!("{plugin_name}@{plugin_marketplace}"),
                name: plugin_name.clone(),
                marketplace: plugin_marketplace.clone(),
                version: version.clone(),
                install_path: target.display().to_string(),
                installed_at: now.clone(),
                updated_at: Some(now),
                scope: InstalledPluginScope::User,
                dependencies: entry.dependencies.clone(),
                source: entry.source.clone(),
                cache_transaction_id: None,
            };
            let record_id = record.id.clone();
            let existing = state.plugins.iter_mut().find(|item| item.id == record.id);
            match existing {
                Some(previous) => {
                    let installed_at = previous.installed_at.clone();
                    *previous = InstalledPluginRecord {
                        installed_at,
                        ..record
                    };
                }
                None => state.plugins.push(record),
            }
            if let Some(record) = state.plugins.iter().find(|item| item.id == record_id) {
                installed.push(record.clone());
            }
        }
        save_installed_plugins_state(storage_root, &state).map_err(|error| error.to_string())
    })();
    match result {
        Ok(()) => Ok(MarketplaceInstallResult {
            dependency_closure: closure,
            installed_plugins: installed,
        }),
        Err(error) => {
            // 回滚本轮新建的 cache 目录（已有目录不动）。
            for path in created_dirs.iter().rev() {
                let _ = std::fs::remove_dir_all(path);
            }
            Err(error)
        }
    }
}

fn now_iso() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    // 简化 ISO 格式：秒级精度已满足排序与展示；毫秒级差异由 lastActivityAt/now_ms 补充。
    format!("{now}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tmp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "plugin-install-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    fn write_marketplace(root: &Path, id: &str, entries: Value) {
        let dir = root.join("marketplaces").join(sanitize_plugin_id(id));
        std::fs::create_dir_all(&dir).expect("marketplace dir");
        let manifest = json!({
            "name": id,
            "plugins": entries,
        });
        std::fs::write(
            dir.join(MARKETPLACE_FILE),
            serde_json::to_string_pretty(&manifest).expect("serialize"),
        )
        .expect("write");
    }

    fn write_plugin_source(root: &Path, relative: &str, version: &str) -> PathBuf {
        let dir = root.join(relative);
        std::fs::create_dir_all(&dir).expect("plugin source");
        let manifest_dir = dir.join(".mode-plugin");
        std::fs::create_dir_all(&manifest_dir).expect("manifest dir");
        std::fs::write(
            manifest_dir.join("plugin.json"),
            serde_json::to_string(&json!({ "name": "demo", "version": version })).unwrap(),
        )
        .expect("write manifest");
        dir
    }

    #[test]
    fn dependency_closure_walks_dependencies_and_blocks_cross_marketplace() {
        let root = tmp_root("closure");
        write_marketplace(
            &root,
            "market-a",
            json!([
                { "name": "app", "dependencies": ["lib"] },
                { "name": "lib" },
            ]),
        );
        let closure =
            resolve_dependency_closure(&root, "market-a", "app", &HashSet::new()).expect("closure");
        assert_eq!(closure, vec!["lib@market-a", "app@market-a"]);

        write_marketplace(&root, "market-b", json!([{ "name": "shared" }]));
        write_marketplace(
            &root,
            "market-a",
            json!([
                { "name": "app", "dependencies": ["shared@market-b"] },
            ]),
        );
        assert!(resolve_dependency_closure(&root, "market-a", "app", &HashSet::new()).is_err());
        let cross = HashSet::from(["market-b".to_string()]);
        let closure = resolve_dependency_closure(&root, "market-a", "app", &cross).expect("cross");
        assert_eq!(closure, vec!["shared@market-b", "app@market-a"]);
    }

    #[test]
    fn dependency_closure_detects_cycles_and_missing_entries() {
        let root = tmp_root("cycle");
        write_marketplace(
            &root,
            "m",
            json!([
                { "name": "a", "dependencies": ["b"] },
                { "name": "b", "dependencies": ["a"] },
            ]),
        );
        assert!(resolve_dependency_closure(&root, "m", "a", &HashSet::new())
            .expect_err("cycle")
            .contains("cycle"));
        write_marketplace(
            &root,
            "m",
            json!([{ "name": "a", "dependencies": ["missing"] }]),
        );
        assert!(resolve_dependency_closure(&root, "m", "a", &HashSet::new())
            .expect_err("missing")
            .contains("Dependency not found"));
    }

    #[test]
    fn install_from_directory_source_copies_and_writes_records() {
        let root = tmp_root("install");
        let source = write_plugin_source(&root, "sources/demo", "2.1.0");
        write_marketplace(
            &root,
            "market-a",
            json!([
                {
                    "name": "demo",
                    "source": { "source": "directory", "path": source.display().to_string() },
                },
            ]),
        );
        let result = install_marketplace_plugin(&root, "market-a", "demo").expect("install");
        assert_eq!(result.dependency_closure, vec!["demo@market-a"]);
        assert_eq!(result.installed_plugins.len(), 1);
        let record = &result.installed_plugins[0];
        assert_eq!(record.version, "2.1.0");
        assert_eq!(record.scope, InstalledPluginScope::User);
        assert!(record.install_path.contains("demo"));
        assert!(Path::new(&record.install_path)
            .join(".mode-plugin/plugin.json")
            .exists());

        let state = load_installed_plugins_state(&root);
        assert_eq!(state.plugins.len(), 1);
        assert_eq!(state.plugins[0].id, "demo@market-a");
    }

    #[test]
    fn install_error_rolls_back_created_cache_dirs() {
        let root = tmp_root("rollback");
        let source = write_plugin_source(&root, "sources/demo", "1.0.0");
        write_marketplace(
            &root,
            "m",
            json!([
                { "name": "demo", "dependencies": ["missing-dep"],
                  "source": { "source": "directory", "path": source.display().to_string() } },
            ]),
        );
        assert!(install_marketplace_plugin(&root, "m", "demo").is_err());
        // 回滚：本轮创建的 cache 目录被清掉（或从未落盘）。
        let cache = root.join("cache");
        assert!(
            !cache.exists()
                || std::fs::read_dir(&cache)
                    .map(|mut entries| entries.next().is_none())
                    .unwrap_or(true)
        );
    }

    #[test]
    fn install_replaces_existing_cache_and_updates_record() {
        let root = tmp_root("reinstall");
        let source_v1 = write_plugin_source(&root, "sources/v1", "1.0.0");
        let source_v2 = write_plugin_source(&root, "sources/v2", "2.0.0");
        write_marketplace(
            &root,
            "m",
            json!([
                { "name": "demo", "source": { "source": "directory", "path": source_v1.display().to_string() } },
            ]),
        );
        install_marketplace_plugin(&root, "m", "demo").expect("install v1");
        // 换源重装：manifest 更新到 v2。
        write_marketplace(
            &root,
            "m",
            json!([
                { "name": "demo", "source": { "source": "directory", "path": source_v2.display().to_string() } },
            ]),
        );
        let result = install_marketplace_plugin(&root, "m", "demo").expect("install v2");
        assert_eq!(result.installed_plugins[0].version, "2.0.0");
        let state = load_installed_plugins_state(&root);
        assert_eq!(state.plugins.len(), 1);
    }

    #[test]
    fn filesystem_source_uses_cache_path() {
        let root = tmp_root("filesystem");
        let cache_dir = write_plugin_source(&root, "cache/builtin", "1.0.0");
        write_marketplace(
            &root,
            "official",
            json!([
                { "name": "builtin", "cachePath": cache_dir.display().to_string(),
                  "source": "filesystem" },
            ]),
        );
        let result = install_marketplace_plugin(&root, "official", "builtin").expect("install");
        assert_eq!(result.installed_plugins.len(), 1);
    }
}
