//! Marketplace 安装编排（LT-15 P3 切三）。
//!
//! 真源：`apps/mode-cli/packages/adapters/src/plugins/marketplace.ts` 的
//! `installMarketplacePlugin` / `resolveDependencyClosure` / `cacheMarketplacePlugin` /
//! `resolvePluginSourceRoot` / `resolveInstalledPluginVersion`。
//!
//! 本切片覆盖 directory/file/字符串相对路径 与 filesystem/sea 缓存兜底源；
//! git/npm/url 等网络源返回错误（后续按需接入）。安装按依赖闭包展开，逐条
//! 拷贝到 cache 目录（官方市场改名后旧目录兜底），失败时回滚新建目录。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config_write_back::enable_plugins_in_config;
use crate::installed_records::{
    load_installed_plugins_state, parse_plugin_id, save_installed_plugins_state,
    InstalledPluginRecord, InstalledPluginScope, DEFAULT_VERSION,
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
    /// TS `manifest.pluginRoot`：插件目录相对 marketplace 目录的基准（顶层或 metadata.pluginRoot）。
    pub plugin_root: Option<String>,
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
    let plugin_root = object
        .get("pluginRoot")
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get("metadata")
                .and_then(|metadata| metadata.get("pluginRoot"))
                .and_then(Value::as_str)
        })
        .map(str::to_string);
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
        plugin_root,
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

const GIT_COMMAND_TIMEOUT_MS: u64 = 90_000;
const GIT_CLONE_MAX_ATTEMPTS: u32 = 3;
const GIT_CLONE_RETRY_DELAY_MS: u64 = 1_000;

/// TS `resolveMarketplacePluginBaseDir`：manifest.pluginRoot 相对 marketplace 目录，
/// 解析失败或目录缺失回落 marketplace 目录本身。
fn resolve_marketplace_plugin_base_dir(
    marketplace_dir: &Path,
    manifest: Option<&MarketplaceManifest>,
) -> PathBuf {
    let Some(plugin_root) = manifest.and_then(|manifest| manifest.plugin_root.as_deref()) else {
        return marketplace_dir.to_path_buf();
    };
    let stripped = plugin_root.strip_prefix("./").unwrap_or(plugin_root);
    let Ok(resolved) = crate::paths::resolve_within(marketplace_dir, stripped) else {
        return marketplace_dir.to_path_buf();
    };
    if resolved.is_dir() {
        resolved
    } else {
        marketplace_dir.to_path_buf()
    }
}

/// TS `readRequiredPluginSourceString`：字段必须是非空字符串。
fn read_required_plugin_source_string(
    source: &serde_json::Map<String, Value>,
    field: &str,
    label: &str,
) -> Result<String, String> {
    match source.get(field).and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => Ok(value.to_string()),
        _ => Err(format!(
            "Plugin {label} source requires a non-empty {field}"
        )),
    }
}

/// TS `readPluginSourceIdentityPin`：zip sha256 → sha → commit（旧写法兼容）。
fn read_plugin_source_identity_pin(source: &serde_json::Map<String, Value>) -> Option<String> {
    for field in ["sha256", "sha", "commit"] {
        if let Some(value) = source
            .get(field)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            return Some(value.to_string());
        }
    }
    None
}

/// TS `normalizeGitUrl`：`owner/repo` 简写（无 `:`）补成 GitHub HTTPS URL。
fn normalize_git_url(value: &str) -> String {
    if value.matches('/').count() == 1
        && !value.contains(':')
        && value.split('/').all(|part| !part.is_empty())
    {
        let (owner, repo) = (
            value.split('/').next().unwrap_or_default(),
            value.split('/').nth(1).unwrap_or_default(),
        );
        return format!("https://github.com/{owner}/{repo}.git");
    }
    value.to_string()
}

/// TS `isRetryableGitCloneError`：仅网络型错误短重试，权限/路径/仓库不存在等
/// 确定性错误不重试。
fn is_retryable_git_clone_error(output: &str) -> bool {
    const PATTERNS: [&str; 11] = [
        "rpc failed",
        "operation timed out",
        "recv failure",
        "expected flush",
        "early eof",
        "remote end hung up",
        "http/2 stream",
        "connection reset",
        "etimedout",
        "econnreset",
        "network timeout",
    ];
    let lower = output.to_lowercase();
    PATTERNS.iter().any(|pattern| lower.contains(pattern))
}

struct GitCommandOutput {
    success: bool,
    combined: String,
}

/// TS `execGitCommand`：显式二进制覆盖（`MODE_GIT_BINARY`）+ 90s 超时（超时 kill）。
fn exec_git(args: &[String]) -> Result<(), String> {
    let output = exec_git_with_output(args)?;
    if output.success {
        Ok(())
    } else {
        Err(output.combined)
    }
}

fn exec_git_with_output(args: &[String]) -> Result<GitCommandOutput, String> {
    let binary = std::env::var("MODE_GIT_BINARY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "git".to_string());
    let mut child = std::process::Command::new(&binary)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                format!("Git binary not available: {binary}")
            } else {
                format!("Failed to spawn git: {error}")
            }
        })?;
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(GIT_COMMAND_TIMEOUT_MS);
    let mut failure = String::new();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = child.stdout.take().map(|mut stream| {
                    let mut buffer = Vec::new();
                    std::io::Read::read_to_end(&mut stream, &mut buffer).ok();
                    String::from_utf8_lossy(&buffer).to_string()
                });
                let stderr = child.stderr.take().map(|mut stream| {
                    let mut buffer = Vec::new();
                    std::io::Read::read_to_end(&mut stream, &mut buffer).ok();
                    String::from_utf8_lossy(&buffer).to_string()
                });
                let combined = [stdout, stderr]
                    .into_iter()
                    .flatten()
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !status.success() {
                    failure = combined;
                }
                return Ok(GitCommandOutput {
                    success: status.success(),
                    combined: failure,
                });
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // 循环唯一出口是超时：kill 后直接返回超时错误。
                    return Ok(GitCommandOutput {
                        success: false,
                        combined: format!("git command timed out after {GIT_COMMAND_TIMEOUT_MS}ms"),
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => return Err(format!("Failed to wait for git: {error}")),
        }
    }
}

/// TS `execGitCloneWithRetry`：仅网络型错误重试，重试间重建目标目录。
fn exec_git_clone_with_retry(args: &[String], target_dir: &Path) -> Result<(), String> {
    let mut last_error = String::new();
    for attempt in 1..=GIT_CLONE_MAX_ATTEMPTS {
        if attempt > 1 {
            let _ = std::fs::remove_dir_all(target_dir);
            std::fs::create_dir_all(target_dir).map_err(|error| error.to_string())?;
        }
        match exec_git(args) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = error;
                if attempt >= GIT_CLONE_MAX_ATTEMPTS || !is_retryable_git_clone_error(&last_error) {
                    return Err(last_error);
                }
                std::thread::sleep(std::time::Duration::from_millis(
                    GIT_CLONE_RETRY_DELAY_MS * attempt as u64,
                ));
            }
        }
    }
    Err(last_error)
}

/// TS `clonePluginSource`：临时目录 clone（无 sha 用 --depth 1，有 ref 用 --branch，
/// 有 sha 补 checkout），失败清理临时目录。
fn clone_plugin_source(
    url: &str,
    git_ref: Option<&str>,
    sha: Option<&str>,
) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("mode-plugin-src-{}-{nanos}", std::process::id()));
    let mut args: Vec<String> = vec!["clone".to_string()];
    if sha.is_none() {
        args.push("--depth".to_string());
        args.push("1".to_string());
    }
    if let Some(git_ref) = git_ref {
        args.push("--branch".to_string());
        args.push(git_ref.to_string());
    }
    args.push(url.to_string());
    args.push(dir.display().to_string());
    let result = exec_git_clone_with_retry(&args, &dir).and_then(|()| {
        if let Some(sha) = sha {
            exec_git(&[
                "-C".to_string(),
                dir.display().to_string(),
                "checkout".to_string(),
                sha.to_string(),
            ])
        } else {
            Ok(())
        }
    });
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
        return result.map(|()| dir);
    }
    Ok(dir)
}

/// 解析结果：源根目录 + 需要调用方用完后清理的临时目录（TS `{path, cleanup}` 的
/// 所有权对应物；网络源 clone 到系统临时目录，复制进 cache 后必须清理）。
#[derive(Debug)]
pub struct ResolvedPluginSourceRoot {
    pub path: PathBuf,
    temp_dir: Option<PathBuf>,
}

impl ResolvedPluginSourceRoot {
    /// 网络 clone 的临时目录（本地目录源为 None；仅测试与诊断用）。
    pub fn temp_dir(&self) -> Option<&Path> {
        self.temp_dir.as_deref()
    }

    /// TS cleanup 语义：删除网络源 clone 的临时目录（幂等）。
    pub fn cleanup(&self) {
        if let Some(temp_dir) = &self.temp_dir {
            let _ = std::fs::remove_dir_all(temp_dir);
        }
    }
}

pub(crate) fn local_source_root(path: PathBuf) -> ResolvedPluginSourceRoot {
    ResolvedPluginSourceRoot {
        path,
        temp_dir: None,
    }
}

pub(crate) fn cloned_source_root(path: PathBuf, temp_dir: PathBuf) -> ResolvedPluginSourceRoot {
    ResolvedPluginSourceRoot {
        path,
        temp_dir: Some(temp_dir),
    }
}

/// TS `resolveGitPluginSource`：clone 后按 `path` 取子目录（须在克隆目录内）。
fn resolve_git_plugin_source(
    url: &str,
    git_ref: Option<&str>,
    sha: Option<&str>,
    path: Option<&str>,
) -> Result<ResolvedPluginSourceRoot, String> {
    let dir = clone_plugin_source(url, git_ref, sha)?;
    let Some(path) = path else {
        return Ok(cloned_source_root(dir.clone(), dir));
    };
    let stripped = path.strip_prefix("./").unwrap_or(path);
    let subdir = crate::paths::resolve_within(&dir, stripped)
        .ok()
        .filter(|resolved| resolved.is_dir());
    match subdir {
        Some(subdir) => Ok(cloned_source_root(subdir, dir)),
        None => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(format!("Plugin source subdirectory does not exist: {path}"))
        }
    }
}

/// TS zip 分支的入参读取（headers/path/sha256/stripRoot；sha256 必填）。
fn read_zip_source_input(
    source_object: &serde_json::Map<String, Value>,
    url: &str,
) -> crate::zip_source::ZipSourceInput {
    fn read_headers(
        source_object: &serde_json::Map<String, Value>,
    ) -> Option<serde_json::Map<String, Value>> {
        let headers = source_object.get("headers")?;
        let Some(headers) = headers.as_object() else {
            // 非对象 headers 由 zip 源校验报错；这里先透传原样。
            let mut map = serde_json::Map::new();
            map.insert("invalid".to_string(), headers.clone());
            return Some(map);
        };
        Some(headers.clone())
    }
    crate::zip_source::ZipSourceInput {
        url: url.to_string(),
        sha256: source_object
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        headers: read_headers(source_object),
        path: source_object
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string),
        strip_root: source_object
            .get("stripRoot")
            .and_then(Value::as_bool),
    }
}

/// TS `resolveRepositoryPluginSource`：先试 GitHub archive 快路径（codeload tar.gz），
/// 网络型失败回退 git clone；本切片直接 git clone（结果一致，传输差异记录 spec 已知差异）。
fn resolve_repository_plugin_source(
    url: &str,
    git_ref: Option<&str>,
    sha: Option<&str>,
    path: Option<&str>,
) -> Result<ResolvedPluginSourceRoot, String> {
    resolve_git_plugin_source(url, git_ref, sha, path)
}

/// 源根目录解析：directory/local/字符串相对路径、filesystem/sea 缓存兜底，
/// 以及 github/git/url/git-subdir 网络源（git clone 子进程）；npm/pip 显式不支持。
pub fn resolve_plugin_source_root(
    storage_root: &Path,
    marketplace: &str,
    entry: &MarketplacePluginEntry,
    manifest: Option<&MarketplaceManifest>,
) -> Result<ResolvedPluginSourceRoot, String> {
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
                    return Ok(local_source_root(path.to_path_buf()));
                }
            }
            let computed = plugin_cache_dir(
                storage_root,
                marketplace,
                &entry.name,
                entry.version.as_deref().unwrap_or(DEFAULT_VERSION),
            );
            if computed.is_dir() {
                return Ok(local_source_root(computed));
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
            match kind {
                "directory" => {
                    let path = read_required_plugin_source_string(
                        source_object,
                        "path",
                        "directory path",
                    )?;
                    let resolved = crate::paths::resolve_user_path(&path);
                    if resolved.is_dir() {
                        return Ok(local_source_root(resolved));
                    }
                    return Err(format!(
                        "Plugin source directory does not exist: {}",
                        resolved.display()
                    ));
                }
                "local" => {
                    // Codex 格式市场条目：{ source: "local", path: "./plugins/<owner>/<name>" }。
                    let source_path =
                        read_required_plugin_source_string(source_object, "path", "local path")?;
                    let stripped = source_path.strip_prefix("./").unwrap_or(&source_path);
                    let base_dir = resolve_marketplace_plugin_base_dir(&marketplace_dir, manifest);
                    if let Ok(local) = crate::paths::resolve_within(&base_dir, stripped) {
                        if local.is_dir() {
                            return Ok(local_source_root(local));
                        }
                    }
                    // TS 本地缺失还会按需从 marketplace 仓库稀疏检出（manifest-only 大
                    // catalog）；该路径依赖 marketplace repo 身份，留待后续切片。
                    return Err(format!(
                        "Plugin source directory does not exist: {stripped}"
                    ));
                }
                "github" => {
                    let repo =
                        read_required_plugin_source_string(source_object, "repo", "GitHub repo")?;
                    let url = format!("https://github.com/{repo}.git");
                    return resolve_repository_plugin_source(
                        &url,
                        source_object.get("ref").and_then(Value::as_str),
                        read_plugin_source_identity_pin(source_object).as_deref(),
                        source_object.get("path").and_then(Value::as_str),
                    );
                }
                "git" => {
                    let url = read_required_plugin_source_string(source_object, "url", "Git URL")?;
                    return resolve_repository_plugin_source(
                        &url,
                        source_object.get("ref").and_then(Value::as_str),
                        read_plugin_source_identity_pin(source_object).as_deref(),
                        source_object.get("path").and_then(Value::as_str),
                    );
                }
                "url" => {
                    let url = read_required_plugin_source_string(source_object, "url", "URL")?;
                    let source_type = source_object
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if source_type == "zip" {
                        return crate::zip_source::resolve_zip_plugin_source(&read_zip_source_input(
                            source_object,
                            &url,
                        ));
                    }
                    if !source_type.is_empty() && source_type != "git" {
                        return Err(format!(
                            "Plugin source is recognized but not supported in this runtime: url:{source_type}"
                        ));
                    }
                    return resolve_repository_plugin_source(
                        &url,
                        source_object.get("ref").and_then(Value::as_str),
                        read_plugin_source_identity_pin(source_object).as_deref(),
                        source_object.get("path").and_then(Value::as_str),
                    );
                }
                "git-subdir" => {
                    let path = read_required_plugin_source_string(
                        source_object,
                        "path",
                        "git-subdir path",
                    )?;
                    let raw_url =
                        read_required_plugin_source_string(source_object, "url", "git-subdir URL")?;
                    let url = normalize_git_url(&raw_url);
                    return resolve_repository_plugin_source(
                        &url,
                        source_object.get("ref").and_then(Value::as_str),
                        read_plugin_source_identity_pin(source_object).as_deref(),
                        Some(&path),
                    );
                }
                "npm" | "pip" => {
                    return Err(format!(
                        "Plugin source is recognized but not supported in this runtime: {kind}"
                    ));
                }
                _ => {
                    // 显式 object source 配置错误时不能降级到 marketplace 内同名目录。
                    return Err(format!(
                        "Plugin source is invalid or unsupported for {}@{marketplace}: {}",
                        entry.name,
                        if kind.is_empty() {
                            "missing kind"
                        } else {
                            kind
                        }
                    ));
                }
            }
        }
    }
    // 裸字符串：manifest 相对路径（含 pluginRoot 基准）或绝对路径。
    if let Some(source) = source.and_then(Value::as_str) {
        let stripped = source.strip_prefix("./").unwrap_or(source);
        let base_dir = resolve_marketplace_plugin_base_dir(&marketplace_dir, manifest);
        let relative = base_dir.join(stripped);
        if relative.is_dir() {
            return Ok(local_source_root(relative));
        }
        let absolute = crate::paths::resolve_user_path(source);
        if absolute.is_dir() {
            return Ok(local_source_root(absolute));
        }
        // TS 本地缺失还会按需从 marketplace 仓库稀疏检出；留待后续切片。
        return Err(format!("Unsupported or missing plugin source: {source}"));
    }
    Err(format!(
        "Plugin source is not supported for {}@{marketplace}",
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

/// 安装编排结果：依赖闭包、实际写入的安装记录，以及本次默认启用的插件 id
/// （TS bootstrap 用它区分「新置默认启用」与「沿用既有显式配置」）。
pub struct MarketplaceInstallResult {
    pub dependency_closure: Vec<String>,
    pub installed_plugins: Vec<InstalledPluginRecord>,
    pub enabled_ids: Vec<String>,
}

/// 安装编排（directory/file 源）：闭包展开 → 逐条拷贝到 cache → 记录 upsert → 原子保存
/// → config 写回（安装即默认启用，只动未显式声明的 id）。
/// 任一步失败回滚本轮新建的 cache 目录，并返回错误；已有记录不动。
/// `config_path=None` 跳过写回（对拍测试/纯编排复用）。
pub fn install_marketplace_plugin(
    storage_root: &Path,
    marketplace: &str,
    name: &str,
    config_path: Option<&Path>,
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
            let source_root = resolve_plugin_source_root(
                storage_root,
                &plugin_marketplace,
                entry,
                Some(&manifest),
            )?;
            let version = resolve_installed_plugin_version(&source_root.path, entry);
            let target =
                plugin_cache_dir(storage_root, &plugin_marketplace, &plugin_name, &version);
            if source_root.path != target {
                if target.exists() {
                    std::fs::remove_dir_all(&target).map_err(|error| error.to_string())?;
                }
                copy_dir_recursive(&source_root.path, &target)?;
                created_dirs.push(target.clone());
            }
            // 网络源 clone 的临时目录复制完成即清理（TS cacheMarketplacePlugin 的
            // cleanup 语义）；本地目录不动。
            source_root.cleanup();
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
        Ok(()) => {
            // 安装即默认启用：写入 User config（TS bootstrap 在安装成功后同步写回，
            // 失败向上抛，与安装同成败）。
            let enabled_ids = match config_path {
                Some(path) => {
                    let ids: Vec<String> = installed.iter().map(|r| r.id.clone()).collect();
                    enable_plugins_in_config(path, &ids)?
                }
                None => Vec::new(),
            };
            Ok(MarketplaceInstallResult {
                dependency_closure: closure,
                installed_plugins: installed,
                enabled_ids,
            })
        }
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
        let result = install_marketplace_plugin(&root, "market-a", "demo", None).expect("install");
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

    fn temp_clone_dirs() -> Vec<PathBuf> {
        std::fs::read_dir(std::env::temp_dir())
            .expect("tmp")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("mode-plugin-src-")
            })
            .map(|entry| entry.path())
            .collect()
    }

    fn entry_with_source(source: Value) -> MarketplacePluginEntry {
        MarketplacePluginEntry {
            name: "demo".to_string(),
            version: None,
            dependencies: None,
            source: Some(source),
            cache_path: None,
        }
    }

    #[test]
    fn normalize_git_url_expands_owner_repo_shorthand() {
        assert_eq!(
            normalize_git_url("owner/repo"),
            "https://github.com/owner/repo.git"
        );
        assert_eq!(
            normalize_git_url("https://github.com/owner/repo"),
            "https://github.com/owner/repo"
        );
        assert_eq!(
            normalize_git_url("git@host:owner/repo"),
            "git@host:owner/repo"
        );
        assert_eq!(normalize_git_url("a/b/c"), "a/b/c");
    }

    #[test]
    fn identity_pin_prefers_sha_then_commit() {
        let mut source = serde_json::Map::new();
        assert_eq!(read_plugin_source_identity_pin(&source), None);
        source.insert("commit".to_string(), Value::String("abc".to_string()));
        assert_eq!(
            read_plugin_source_identity_pin(&source).as_deref(),
            Some("abc")
        );
        source.insert("sha".to_string(), Value::String("def".to_string()));
        assert_eq!(
            read_plugin_source_identity_pin(&source).as_deref(),
            Some("def")
        );
        source.insert("sha256".to_string(), Value::String("789".to_string()));
        assert_eq!(
            read_plugin_source_identity_pin(&source).as_deref(),
            Some("789")
        );
    }

    #[test]
    fn retryable_git_errors_match_network_patterns() {
        assert!(is_retryable_git_clone_error(
            "fatal: the remote end hung up unexpectedly"
        ));
        assert!(is_retryable_git_clone_error(
            "error: RPC failed; curl 56 Recv failure"
        ));
        assert!(is_retryable_git_clone_error("Connection reset by peer"));
        assert!(!is_retryable_git_clone_error(
            "fatal: repository 'x' not found"
        ));
        assert!(!is_retryable_git_clone_error("permission denied"));
    }

    #[test]
    fn unsupported_source_kinds_report_runtime_error() {
        let root = tmp_root("kinds");
        std::fs::create_dir_all(&root).expect("root");
        let npm = entry_with_source(json!({ "source": "npm" }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &npm, None).expect_err("npm"),
            "Plugin source is recognized but not supported in this runtime: npm"
        );
        let pip = entry_with_source(json!({ "source": "pip" }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &pip, None).expect_err("pip"),
            "Plugin source is recognized but not supported in this runtime: pip"
        );
        // url:zip 已接入（真实实现）；缺 sha256/短 sha256 走必填校验。
        let zip = entry_with_source(json!({
            "source": "url", "type": "zip", "url": "https://example.com/x.zip",
            "sha256": "deadbeef",
        }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &zip, None).expect_err("zip"),
            "Plugin zip source sha256 must be a 64 character hex string"
        );
        let tar = entry_with_source(json!({
            "source": "url", "type": "tar", "url": "https://example.com/x.tar",
        }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &tar, None).expect_err("tar"),
            "Plugin source is recognized but not supported in this runtime: url:tar"
        );
        let file_kind = entry_with_source(json!({ "source": "file", "path": "x" }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &file_kind, None).expect_err("file"),
            "Plugin source is invalid or unsupported for demo@m: file"
        );
        let missing_kind = entry_with_source(json!({}));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &missing_kind, None).expect_err("missing"),
            "Plugin source is invalid or unsupported for demo@m: missing kind"
        );
    }

    #[test]
    fn required_source_fields_report_non_empty_errors() {
        let root = tmp_root("required");
        std::fs::create_dir_all(&root).expect("root");
        let git = entry_with_source(json!({ "source": "git" }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &git, None).expect_err("git"),
            "Plugin Git URL source requires a non-empty url"
        );
        let github = entry_with_source(json!({ "source": "github" }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &github, None).expect_err("github"),
            "Plugin GitHub repo source requires a non-empty repo"
        );
        let url = entry_with_source(json!({ "source": "url" }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &url, None).expect_err("url"),
            "Plugin URL source requires a non-empty url"
        );
        let subdir = entry_with_source(json!({ "source": "git-subdir", "url": "owner/repo" }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &subdir, None).expect_err("subdir"),
            "Plugin git-subdir path source requires a non-empty path"
        );
        let directory = entry_with_source(json!({ "source": "directory" }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &directory, None).expect_err("directory"),
            "Plugin directory path source requires a non-empty path"
        );
    }

    #[test]
    fn local_source_resolves_inside_plugin_root_base_dir() {
        let root = tmp_root("local-root");
        // pluginRoot: "plugins" → base 目录是 marketplace 目录下的 plugins/。
        let marketplace_dir = root.join("marketplaces").join("m");
        std::fs::create_dir_all(marketplace_dir.join("plugins").join("demo")).expect("plugin");
        let manifest = json!({
            "name": "m",
            "pluginRoot": "plugins",
            "plugins": [ { "name": "demo", "source": "local", "path": "./demo" } ],
        });
        std::fs::write(
            marketplace_dir.join(MARKETPLACE_FILE),
            serde_json::to_string_pretty(&manifest).expect("serialize"),
        )
        .expect("write");
        let loaded = load_marketplace_manifest(&root, "m").expect("manifest");
        assert_eq!(loaded.plugin_root.as_deref(), Some("plugins"));
        let entry = entry_with_source(json!({ "source": "local", "path": "./demo" }));
        let resolved =
            resolve_plugin_source_root(&root, "m", &entry, Some(&loaded)).expect("local");
        assert_eq!(resolved.path, marketplace_dir.join("plugins").join("demo"));

        // pluginRoot 指向不存在目录时回落 marketplace 目录。
        let entry2 = entry_with_source(json!({ "source": "local", "path": "./top-level" }));
        let resolved2 = resolve_plugin_source_root(&root, "m", &entry2, Some(&loaded)).is_err();
        assert!(resolved2, "plugins/demo 下没有 top-level，应报错");
        let missing_path = entry_with_source(json!({ "source": "local", "path": "./absent" }));
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &missing_path, Some(&loaded))
                .expect_err("missing"),
            "Plugin source directory does not exist: absent"
        );
    }

    #[test]
    fn string_source_error_reports_raw_source_text() {
        let root = tmp_root("string-err");
        write_marketplace(
            &root,
            "m",
            json!([ { "name": "demo", "source": "./absent" } ]),
        );
        let loaded = load_marketplace_manifest(&root, "m").expect("manifest");
        let entry = loaded
            .plugins
            .iter()
            .find(|entry| entry.name == "demo")
            .cloned()
            .expect("entry");
        assert_eq!(
            resolve_plugin_source_root(&root, "m", &entry, Some(&loaded)).expect_err("err"),
            "Unsupported or missing plugin source: ./absent"
        );
    }

    #[test]
    fn git_source_clones_local_repo_subdirectory_with_pin() {
        // git 缺失时跳过（与 TS e2e 同思路：环境无 git 则不测该链路）。
        let probe = std::process::Command::new(
            std::env::var("MODE_GIT_BINARY").unwrap_or_else(|_| "git".to_string()),
        )
        .arg("--version")
        .output();
        let Ok(probe) = probe else {
            eprintln!("git unavailable; skipping clone e2e");
            return;
        };
        if !probe.status.success() {
            eprintln!("git unavailable; skipping clone e2e");
            return;
        }

        let root = tmp_root("git-clone");
        let repo_dir = root.join("repo");
        std::fs::create_dir_all(repo_dir.join("plugins/demo/.mode-plugin")).expect("repo");
        std::fs::create_dir_all(repo_dir.join("plugins/helper/.mode-plugin")).expect("helper");
        std::fs::write(
            repo_dir.join("plugins/demo/.mode-plugin/plugin.json"),
            r#"{"name":"demo","version":"1.0.0"}"#,
        )
        .expect("demo manifest");
        std::fs::write(
            repo_dir.join("plugins/helper/.mode-plugin/plugin.json"),
            r#"{"name":"helper","version":"0.1.0"}"#,
        )
        .expect("helper manifest");
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&repo_dir)
                .status()
                .expect("git run");
            assert!(status.success(), "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["-c", "user.email=t@local", "-c", "user.name=t", "add", "."]);
        run(&[
            "-c",
            "user.email=t@local",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ]);
        let sha_output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo_dir)
            .output()
            .expect("rev-parse");
        let sha = String::from_utf8_lossy(&sha_output.stdout)
            .trim()
            .to_string();

        // url + path：clone 后取子目录。
        let entry = entry_with_source(json!({
            "source": "git", "url": repo_dir.display().to_string(), "path": "plugins/demo",
        }));
        let resolved = resolve_plugin_source_root(&root, "m", &entry, None).expect("clone");
        assert_eq!(
            std::fs::read_to_string(resolved.path.join(".mode-plugin/plugin.json")).expect("read"),
            r#"{"name":"demo","version":"1.0.0"}"#
        );
        // 网络源 clone 落在临时目录，用完必须清理（TS cleanup 所有权）。
        let temp_root = resolved.temp_dir().expect("temp dir");
        assert!(temp_root.is_dir());
        resolved.cleanup();
        assert!(!temp_root.exists(), "cleanup 应删除临时 clone 目录");

        // 带 sha pin：clone 后 checkout 指定提交。
        let pinned = entry_with_source(json!({
            "source": "git", "url": repo_dir.display().to_string(), "sha": sha, "path": "plugins/helper",
        }));
        let resolved_pinned =
            resolve_plugin_source_root(&root, "m", &pinned, None).expect("clone pin");
        assert!(resolved_pinned
            .path
            .join(".mode-plugin/plugin.json")
            .exists());

        // 子目录不存在：报错并清理临时目录（前后快照对比，不依赖共享 temp 的历史残留）。
        let before = temp_clone_dirs();
        let bad_path = entry_with_source(json!({
            "source": "git", "url": repo_dir.display().to_string(), "path": "plugins/absent",
        }));
        let error = resolve_plugin_source_root(&root, "m", &bad_path, None).expect_err("absent");
        assert_eq!(
            error,
            "Plugin source subdirectory does not exist: plugins/absent"
        );
        let after = temp_clone_dirs();
        assert_eq!(after.len(), before.len(), "失败 clone 应清理自己的临时目录");
        assert!(
            after.iter().all(|dir| before.contains(dir)),
            "不应新增临时目录: {after:?}"
        );

        // git-subdir + owner/repo 简写不适用于本地路径，直接用绝对 URL 验证透传。
        let subdir = entry_with_source(json!({
            "source": "git-subdir", "url": repo_dir.display().to_string(), "path": "plugins/demo",
        }));
        let resolved_subdir =
            resolve_plugin_source_root(&root, "m", &subdir, None).expect("git-subdir");
        assert!(resolved_subdir
            .path
            .to_string_lossy()
            .contains("mode-plugin-src-"));
        assert_eq!(
            std::fs::read_to_string(resolved_subdir.path.join(".mode-plugin/plugin.json"))
                .expect("read"),
            r#"{"name":"demo","version":"1.0.0"}"#
        );
        resolved_subdir.cleanup();
    }

    #[test]
    fn install_writes_enabled_ids_to_config() {
        let root = tmp_root("install-config");
        let source = write_plugin_source(&root, "sources/demo", "1.0.0");
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
        let config_path = root.join("config.json");
        // 预置显式停用项，验证不被覆盖（只动未显式声明的 id）。
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&json!({
                "plugins": { "enabledPlugins": { "demo@market-a": false } },
            }))
            .expect("serialize"),
        )
        .expect("write");
        let result = install_marketplace_plugin(&root, "market-a", "demo", Some(&config_path))
            .expect("install");
        // 已显式声明（false）的 id 不重复写：enabled_ids 为空，config 保持 false。
        assert!(result.enabled_ids.is_empty());
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read"))
                .expect("json");
        assert_eq!(config["plugins"]["enabledPlugins"]["demo@market-a"], false);

        // 未显式声明的 id 安装后写 true。
        let source2 = write_plugin_source(&root, "sources/helper", "1.0.0");
        write_marketplace(
            &root,
            "market-a",
            json!([
                { "name": "demo" },
                {
                    "name": "helper",
                    "source": { "source": "directory", "path": source2.display().to_string() },
                },
            ]),
        );
        let result = install_marketplace_plugin(&root, "market-a", "helper", Some(&config_path))
            .expect("install helper");
        assert_eq!(result.enabled_ids, vec!["helper@market-a".to_string()]);
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read"))
                .expect("json");
        assert_eq!(config["plugins"]["enabledPlugins"]["helper@market-a"], true);
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
        assert!(install_marketplace_plugin(&root, "m", "demo", None).is_err());
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
        install_marketplace_plugin(&root, "m", "demo", None).expect("install v1");
        // 换源重装：manifest 更新到 v2。
        write_marketplace(
            &root,
            "m",
            json!([
                { "name": "demo", "source": { "source": "directory", "path": source_v2.display().to_string() } },
            ]),
        );
        let result = install_marketplace_plugin(&root, "m", "demo", None).expect("install v2");
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
        let result =
            install_marketplace_plugin(&root, "official", "builtin", None).expect("install");
        assert_eq!(result.installed_plugins.len(), 1);
    }
}
