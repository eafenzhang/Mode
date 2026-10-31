//! 外部配置扫描、候选构建与发现结果汇总。
//!
//! 真源：`settingsSyncService.ts` 的 `collect*ImportCandidates` /
//! `get*ImportCandidateSkipReason` / `build*SourceRootSummaries` / `build*Discovery`。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::mcp_config::{
    read_mcp_servers_from_source_file, read_mode_mcp_servers, strip_external_mcp_timeout_fields,
};
use crate::sources::{
    candidate_target_root, external_mcp_roots, external_roots, SUPPORTED_COMMAND_AGENT_SOURCES,
    SUPPORTED_MCP_AGENT_SOURCES, SUPPORTED_PLUGIN_AGENT_SOURCES, SUPPORTED_SKILL_AGENT_SOURCES,
};
use plugin_discovery::js;

// ---------------------------------------------------------------------------
// 结果类型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSkillSummary {
    pub name: String,
    pub path: String,
    pub importable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCommandSummary {
    pub name: String,
    pub path: String,
    pub importable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePluginSummary {
    pub name: String,
    pub path: String,
    pub importable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceMcpServerSummary {
    pub name: String,
    pub path: String,
    pub importable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRootSummary {
    pub scope: String,
    pub path: String,
    pub discovered_count: usize,
    pub importable_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SourceSkillSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<SourceCommandSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins: Option<Vec<SourcePluginSummary>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<SourceMcpServerSummary>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySummary {
    pub category: String,
    pub discovered_count: usize,
    pub importable_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_paths: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_roots: Option<Vec<SourceRootSummary>>,
    pub selected_by_default: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummary {
    pub agent: String,
    pub discovered: bool,
    pub categories: Vec<CategorySummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryResult {
    pub agents: Vec<AgentSummary>,
}

// ---------------------------------------------------------------------------
// 候选
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SkillCandidate {
    pub agent: &'static str,
    pub name: String,
    pub name_key: String,
    pub version: Option<String>,
    pub source_root_scope: &'static str,
    pub source_root_path: PathBuf,
    pub source_path: PathBuf,
    pub target_root: PathBuf,
    pub target_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CommandCandidate {
    pub agent: &'static str,
    pub name: String,
    pub name_key: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    pub source_root_scope: &'static str,
    pub source_root_path: PathBuf,
    pub source_path: PathBuf,
    pub source_relative_path: PathBuf,
    pub target_root: PathBuf,
    pub target_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PluginCandidate {
    pub agent: &'static str,
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub source_root_scope: &'static str,
    pub source_root_path: PathBuf,
    pub source_path: PathBuf,
    pub target_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct McpCandidate {
    pub agent: &'static str,
    pub name: String,
    pub name_key: String,
    pub config: Value,
    pub source_root_scope: &'static str,
    pub source_root_path: PathBuf,
    pub source_path: String,
}

// ---------------------------------------------------------------------------
// 文件系统辅助
// ---------------------------------------------------------------------------

/// TS `exists()`：只有目录可读才算存在（技能目标用的是目录语义）。
pub fn exists_dir(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok()
}

pub fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn relative_path(root: &Path, file: &Path) -> PathBuf {
    file.strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| file.to_path_buf())
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn collect_skill_markdown_paths(root: &Path) -> Vec<PathBuf> {
    skills_service::walk_skill_markdown_paths(root)
}

pub fn collect_command_markdown_paths(root: &Path) -> Vec<PathBuf> {
    let mut discovered: BTreeSet<PathBuf> = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if file_type.is_file() && name.to_lowercase().ends_with(".md") {
                discovered.insert(path);
            }
        }
    }
    let mut paths: Vec<PathBuf> = discovered.into_iter().collect();
    paths.sort_by(|left, right| {
        js::locale_compare(&left.to_string_lossy(), &right.to_string_lossy())
    });
    paths
}

pub fn find_plugin_manifest_path(plugin_path: &Path) -> Option<PathBuf> {
    const CANDIDATES: [&[&str]; 4] = [
        &[".mode-plugin", "plugin.json"],
        &[".zcode-plugin", "plugin.json"],
        &[".claude-plugin", "plugin.json"],
        &[".codex-plugin", "plugin.json"],
    ];
    CANDIDATES
        .iter()
        .map(|parts| {
            parts
                .iter()
                .fold(plugin_path.to_path_buf(), |path, part| path.join(part))
        })
        .find(|candidate| path_exists(candidate))
}

pub fn collect_plugin_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut discovered = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let plugin_path = entry.path();
        if find_plugin_manifest_path(&plugin_path).is_some() {
            discovered.push(plugin_path);
        }
    }
    discovered.sort_by(|left, right| {
        js::locale_compare(&left.to_string_lossy(), &right.to_string_lossy())
    });
    discovered
}

// ---------------------------------------------------------------------------
// 元数据
// ---------------------------------------------------------------------------

fn strip_yaml_scalar_quotes(value: &str) -> String {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        return trimmed[1..trimmed.len() - 1].trim().to_string();
    }
    trimmed.to_string()
}

fn read_loose_skill_field(frontmatter: &str, key: &str) -> Option<String> {
    for line in frontmatter.split('\n') {
        let rest = line.strip_prefix(key)?;
        // `name` 后必须是可选的空白 + `:`（避免命中 `names:` 之类的键）。
        let after_spaces = rest.trim_start_matches([' ', '\t']);
        let Some(value) = after_spaces.strip_prefix(':') else {
            continue;
        };
        let value = value.trim_start_matches([' ', '\t']);
        if value.is_empty() {
            continue;
        }
        return Some(strip_yaml_scalar_quotes(value));
    }
    None
}

/// 受支持的 YAML 子集：只解析顶层 `key: scalar`，嵌套行忽略；
/// 非缩进行缺少 `:` 视为严格解析失败（与 `yaml.parse` 抛错后的回退一致）。
fn parse_top_level_scalars(frontmatter: &str) -> Option<BTreeMap<String, Value>> {
    let mut map = BTreeMap::new();
    for raw in frontmatter.split('\n') {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            // 嵌套内容对 name/version 没有贡献。
            continue;
        }
        let separator = line.find(':')?;
        let key = line[..separator].trim();
        if key.is_empty() {
            return None;
        }
        let raw_value = line[separator + 1..].trim();
        map.insert(key.to_string(), parse_yaml_scalar(raw_value));
    }
    Some(map)
}

fn parse_yaml_scalar(value: &str) -> Value {
    if value.is_empty() {
        return Value::Null;
    }
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        return Value::String(value[1..value.len() - 1].to_string());
    }
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        return Value::String(value[1..value.len() - 1].replace("''", "'"));
    }
    match value {
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        "null" | "Null" | "NULL" | "~" => return Value::Null,
        _ => {}
    }
    if let Ok(number) = value.parse::<i64>() {
        return Value::Number(number.into());
    }
    if let Ok(number) = value.parse::<f64>() {
        if let Some(number) = serde_json::Number::from_f64(number) {
            return Value::Number(number);
        }
    }
    Value::String(value.to_string())
}

fn split_skill_frontmatter(content: &str) -> Option<String> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---\n") {
        return None;
    }
    let rest = &normalized[4..];
    let end = rest.find("\n---")?;
    // 闭合行必须是行首 `---`，其后为换行或文件结束。
    let after = &rest[end + 4..];
    if !(after.is_empty() || after.starts_with('\n')) {
        return None;
    }
    Some(rest[..end].to_string())
}

pub fn read_skill_metadata_from_markdown(
    content: &str,
    fallback_name: &str,
) -> (String, Option<String>) {
    let Some(frontmatter) = split_skill_frontmatter(content) else {
        return (fallback_name.to_string(), None);
    };
    let parsed = parse_top_level_scalars(&frontmatter);
    match parsed {
        Some(scalars) => {
            let name = scalars
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| read_loose_skill_field(&frontmatter, "name"))
                .unwrap_or_else(|| fallback_name.to_string());
            let parsed_version = match scalars.get("version") {
                Some(Value::String(value)) => value.trim().to_string(),
                Some(Value::Number(number)) => number.to_string(),
                _ => String::new(),
            };
            let version = if parsed_version.is_empty() {
                read_loose_skill_field(&frontmatter, "version")
            } else {
                Some(parsed_version)
            };
            (name, version)
        }
        None => (
            read_loose_skill_field(&frontmatter, "name")
                .unwrap_or_else(|| fallback_name.to_string()),
            read_loose_skill_field(&frontmatter, "version"),
        ),
    }
}

pub fn read_skill_metadata(skill_path: &Path) -> (String, Option<String>) {
    let fallback = skill_path.parent().map(basename).unwrap_or_default();
    match std::fs::read(skill_path) {
        Ok(bytes) => {
            let content = String::from_utf8_lossy(&bytes);
            read_skill_metadata_from_markdown(&content, &fallback)
        }
        Err(_) => (fallback, None),
    }
}

fn normalize_name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

fn read_frontmatter_key(line: &str) -> Option<String> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let trimmed = line.trim();
    let separator = trimmed.find(':')?;
    let key = &trimmed[..separator];
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return None;
    }
    Some(key.to_lowercase())
}

fn frontmatter_lines(content: &str) -> Vec<&str> {
    let lines: Vec<&str> = content.split('\n').collect();
    let start = lines.iter().position(|line| line.trim() == "---");
    let Some(start) = start else {
        return Vec::new();
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(index, _)| index);
    match end {
        Some(end) => lines[start + 1..end].to_vec(),
        None => Vec::new(),
    }
}

fn read_multiline_value(lines: &[&str], target: &str) -> Option<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut collecting = false;
    for line in lines {
        if let Some(key) = read_frontmatter_key(line) {
            if collecting {
                break;
            }
            if key == target {
                collecting = true;
                let value = line
                    .split(':')
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(":")
                    .trim()
                    .to_string();
                if !value.is_empty() {
                    collected.push(value);
                }
            }
            continue;
        }
        if collecting && (line.starts_with(' ') || line.starts_with('\t')) {
            collected.push(line.trim().to_string());
        }
    }
    let value = collected.join(" ").trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn command_name_from_path(root: &Path, file: &Path) -> String {
    let relative = relative_path(root, file);
    let text = relative.to_string_lossy();
    let without_ext = text
        .strip_suffix(".md")
        .or_else(|| text.strip_suffix(".MD"))
        .unwrap_or(&text)
        .to_string();
    let segments: Vec<&str> = without_ext
        .split(['\\', '/'])
        .filter(|segment| !segment.is_empty())
        .collect();
    format!("/{}", segments.join("/"))
}

fn read_command_metadata(root: &Path, file: &Path) -> (String, Option<String>, Option<String>) {
    let name = command_name_from_path(root, file);
    let Ok(bytes) = std::fs::read(file) else {
        return (name, None, None);
    };
    let content = String::from_utf8_lossy(&bytes).into_owned();
    let lines = frontmatter_lines(&content);
    if lines.is_empty() {
        return (name, None, None);
    }
    (
        name,
        read_multiline_value(&lines, "description"),
        read_multiline_value(&lines, "argument-hint"),
    )
}

pub struct PluginMetadata {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
}

pub fn read_plugin_metadata(plugin_path: &Path) -> Option<PluginMetadata> {
    let manifest_path = find_plugin_manifest_path(plugin_path)?;
    let bytes = std::fs::read(&manifest_path).ok()?;
    let parsed: Value = serde_json::from_slice(&bytes).ok()?;
    let object = parsed.as_object()?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let version = object
        .get("version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Some(PluginMetadata {
        id: format!("{name}@inline"),
        name,
        version,
    })
}

// ---------------------------------------------------------------------------
// 候选收集
// ---------------------------------------------------------------------------

pub fn collect_skill_candidates(home: &Path, workspace: Option<&Path>) -> Vec<SkillCandidate> {
    let mut candidates = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in external_roots(SUPPORTED_SKILL_AGENT_SOURCES, home, workspace) {
        if !exists_dir(&root.root_path) {
            continue;
        }
        for skill_path in collect_skill_markdown_paths(&root.root_path) {
            if !seen.insert(skill_path.clone()) {
                continue;
            }
            let Some(source_dir) = skill_path.parent() else {
                continue;
            };
            let Some(target_root) = candidate_target_root(home, workspace, root.scope, "skills")
            else {
                continue;
            };
            let (name, version) = read_skill_metadata(&skill_path);
            candidates.push(SkillCandidate {
                agent: root.agent,
                name_key: normalize_name_key(&name),
                name,
                version,
                source_root_scope: if root.scope == "workspace" {
                    "project"
                } else {
                    "global"
                },
                source_root_path: root.root_path.clone(),
                source_path: source_dir.to_path_buf(),
                target_path: target_root.join(basename(source_dir)),
                target_root,
            });
        }
    }
    candidates
}

pub fn collect_command_candidates(home: &Path, workspace: Option<&Path>) -> Vec<CommandCandidate> {
    let mut candidates = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in external_roots(SUPPORTED_COMMAND_AGENT_SOURCES, home, workspace) {
        if !exists_dir(&root.root_path) {
            continue;
        }
        for command_path in collect_command_markdown_paths(&root.root_path) {
            if !seen.insert(command_path.clone()) {
                continue;
            }
            let Some(target_root) = candidate_target_root(home, workspace, root.scope, "commands")
            else {
                continue;
            };
            let (name, description, argument_hint) =
                read_command_metadata(&root.root_path, &command_path);
            let relative = relative_path(&root.root_path, &command_path);
            candidates.push(CommandCandidate {
                agent: root.agent,
                name_key: normalize_name_key(&name),
                name,
                description,
                argument_hint,
                source_root_scope: if root.scope == "workspace" {
                    "project"
                } else {
                    "global"
                },
                source_root_path: root.root_path.clone(),
                source_path: command_path.clone(),
                target_path: target_root.join(&relative),
                source_relative_path: relative,
                target_root,
            });
        }
    }
    candidates
}

pub fn collect_plugin_candidates(home: &Path, workspace: Option<&Path>) -> Vec<PluginCandidate> {
    let mut candidates = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in external_roots(SUPPORTED_PLUGIN_AGENT_SOURCES, home, workspace) {
        if !exists_dir(&root.root_path) {
            continue;
        }
        for plugin_path in collect_plugin_paths(&root.root_path) {
            if !seen.insert(plugin_path.clone()) {
                continue;
            }
            let Some(target_root) = candidate_target_root(home, workspace, root.scope, "plugins")
            else {
                continue;
            };
            let Some(metadata) = read_plugin_metadata(&plugin_path) else {
                continue;
            };
            candidates.push(PluginCandidate {
                agent: root.agent,
                id: metadata.id,
                name: metadata.name,
                version: metadata.version,
                source_root_scope: if root.scope == "workspace" {
                    "project"
                } else {
                    "global"
                },
                source_root_path: root.root_path.clone(),
                source_path: plugin_path.clone(),
                target_path: target_root.join(basename(&plugin_path)),
            });
        }
    }
    candidates
}

pub fn collect_mcp_candidates(home: &Path, workspace: Option<&Path>) -> Vec<McpCandidate> {
    let mut candidates = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for root in external_mcp_roots(home, workspace) {
        if !path_exists(&root.file_path) {
            continue;
        }
        let server_map = read_mcp_servers_from_source_file(&root.file_path, root.format);
        for (name, config) in server_map {
            let source_path = format!("{}#{name}", root.file_path.to_string_lossy());
            let seen_key = format!("{}:{source_path}", root.agent);
            if !seen.insert(seen_key) {
                continue;
            }
            candidates.push(McpCandidate {
                agent: root.agent,
                name_key: normalize_name_key(&name),
                name,
                config: strip_external_mcp_timeout_fields(&config),
                source_root_scope: if root.scope == "workspace" {
                    "project"
                } else {
                    "global"
                },
                source_root_path: root.file_path.clone(),
                source_path,
            });
        }
    }
    candidates
}

// ---------------------------------------------------------------------------
// 目标侧已有键
// ---------------------------------------------------------------------------

pub fn collect_existing_skill_name_keys(root: &Path) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if !exists_dir(root) {
        return keys;
    }
    for skill_path in collect_skill_markdown_paths(root) {
        let (name, _) = read_skill_metadata(&skill_path);
        keys.insert(normalize_name_key(&name));
    }
    keys
}

pub fn collect_existing_command_name_keys(root: &Path) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if !exists_dir(root) {
        return keys;
    }
    for command_path in collect_command_markdown_paths(root) {
        keys.insert(normalize_name_key(&command_name_from_path(
            root,
            &command_path,
        )));
    }
    keys
}

pub fn read_json_object_or_empty(path: &Path) -> serde_json::Map<String, Value> {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(object)) => object,
            _ => serde_json::Map::new(),
        },
        Err(_) => serde_json::Map::new(),
    }
}

fn read_string_array(value: Option<&Value>) -> Vec<String> {
    match value.and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    }
}

pub fn collect_configured_plugin_ids(config_path: &Path) -> BTreeSet<String> {
    let parsed = read_json_object_or_empty(config_path);
    let plugins = parsed.get("plugins").and_then(Value::as_object);
    let mut ids = BTreeSet::new();
    for plugin_path in read_string_array(plugins.and_then(|plugins| plugins.get("dirs"))) {
        let resolved = js::normalize_lexical(Path::new(&plugin_path));
        if let Some(metadata) = read_plugin_metadata(&resolved) {
            ids.insert(metadata.id.to_lowercase());
        }
    }
    ids
}

pub fn collect_existing_mcp_name_keys(config_path: &Path) -> BTreeSet<String> {
    let parsed = read_json_object_or_empty(config_path);
    read_mode_mcp_servers(&parsed)
        .keys()
        .map(|name| normalize_name_key(name))
        .collect()
}

// ---------------------------------------------------------------------------
// 跳过原因
// ---------------------------------------------------------------------------

pub fn skill_skip_reason(
    candidate: &SkillCandidate,
    existing: &mut HashMap<PathBuf, BTreeSet<String>>,
) -> Option<String> {
    if exists_dir(&candidate.target_path) {
        return Some("targetExists".to_string());
    }
    let keys = existing
        .entry(candidate.target_root.clone())
        .or_insert_with(|| collect_existing_skill_name_keys(&candidate.target_root));
    keys.contains(&candidate.name_key)
        .then(|| "sameNameExists".to_string())
}

pub fn command_skip_reason(
    candidate: &CommandCandidate,
    existing: &mut HashMap<PathBuf, BTreeSet<String>>,
) -> Option<String> {
    if path_exists(&candidate.target_path) {
        return Some("targetExists".to_string());
    }
    let keys = existing
        .entry(candidate.target_root.clone())
        .or_insert_with(|| collect_existing_command_name_keys(&candidate.target_root));
    keys.contains(&candidate.name_key)
        .then(|| "sameNameExists".to_string())
}

pub fn plugin_skip_reason(
    candidate: &PluginCandidate,
    existing: &mut HashMap<PathBuf, BTreeSet<String>>,
    home: &Path,
    workspace: Option<&Path>,
    target_scope: &str,
) -> Option<String> {
    if path_exists(&candidate.target_path) {
        return Some("targetExists".to_string());
    }
    let Some(config_path) = crate::sources::config_path_for_scope(home, workspace, target_scope)
    else {
        return Some("targetExists".to_string());
    };
    let ids = existing
        .entry(config_path.clone())
        .or_insert_with(|| collect_configured_plugin_ids(&config_path));
    ids.contains(&candidate.id.to_lowercase())
        .then(|| "sameNameExists".to_string())
}

pub fn mcp_skip_reason(
    candidate: &McpCandidate,
    existing: &mut HashMap<String, BTreeSet<String>>,
    home: &Path,
    workspace: Option<&Path>,
    target_scope: &str,
) -> Option<String> {
    let keys = existing.entry(target_scope.to_string()).or_insert_with(|| {
        crate::sources::config_path_for_scope(home, workspace, target_scope)
            .map(|path| collect_existing_mcp_name_keys(&path))
            .unwrap_or_default()
    });
    keys.contains(&candidate.name_key)
        .then(|| "sameNameExists".to_string())
}

// ---------------------------------------------------------------------------
// 汇总与发现
// ---------------------------------------------------------------------------

fn sorted_source_paths(candidates: &[(&'static str, PathBuf)]) -> Vec<String> {
    let mut paths: Vec<String> = candidates
        .iter()
        .map(|(_, path)| path.to_string_lossy().into_owned())
        .collect();
    paths.sort_by(|left, right| js::locale_compare(left, right));
    paths.dedup();
    paths
}

fn sort_summaries(summaries: &mut [SourceRootSummary]) {
    summaries.sort_by(|left, right| {
        js::locale_compare(&left.scope, &right.scope)
            .then_with(|| js::locale_compare(&left.path, &right.path))
    });
}

pub fn build_skill_summaries(
    home: &Path,
    workspace: Option<&Path>,
    agent: &str,
    candidates: &[SkillCandidate],
) -> Vec<SourceRootSummary> {
    let candidates: Vec<&SkillCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.agent == agent)
        .collect();
    let mut grouped: Vec<(String, Vec<&SkillCandidate>)> = Vec::new();
    for candidate in candidates {
        let key = format!(
            "{}:{}",
            candidate.source_root_scope,
            candidate.source_root_path.to_string_lossy()
        );
        match grouped.iter_mut().find(|(existing, _)| existing == &key) {
            Some((_, group)) => group.push(candidate),
            None => grouped.push((key, vec![candidate])),
        }
    }
    let _ = (home, workspace, agent);
    let mut summaries = Vec::new();
    let mut existing: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
    for (_, group) in grouped {
        let Some(first) = group.first() else {
            continue;
        };
        let mut importable_count = 0;
        let mut skills = Vec::new();
        for candidate in &group {
            let skip = skill_skip_reason(candidate, &mut existing);
            if skip.is_none() {
                importable_count += 1;
            }
            skills.push(SourceSkillSummary {
                name: candidate.name.clone(),
                path: candidate.source_path.to_string_lossy().into_owned(),
                importable: skip.is_none(),
                skip_reason: skip,
                version: candidate.version.clone(),
            });
        }
        skills.sort_by(|left, right| js::locale_compare(&left.name, &right.name));
        summaries.push(SourceRootSummary {
            scope: first.source_root_scope.to_string(),
            path: first.source_root_path.to_string_lossy().into_owned(),
            discovered_count: group.len(),
            importable_count,
            skipped_count: Some(group.len() - importable_count),
            skills: Some(skills),
            commands: None,
            plugins: None,
            mcp_servers: None,
        });
    }
    sort_summaries(&mut summaries);
    summaries
}

pub fn build_command_summaries(
    agent: &str,
    candidates: &[CommandCandidate],
) -> Vec<SourceRootSummary> {
    let candidates: Vec<&CommandCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.agent == agent)
        .collect();
    let mut grouped: Vec<(String, Vec<&CommandCandidate>)> = Vec::new();
    for candidate in candidates {
        let key = format!(
            "{}:{}",
            candidate.source_root_scope,
            candidate.source_root_path.to_string_lossy()
        );
        match grouped.iter_mut().find(|(existing, _)| existing == &key) {
            Some((_, group)) => group.push(candidate),
            None => grouped.push((key, vec![candidate])),
        }
    }
    let mut summaries = Vec::new();
    let mut existing: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
    for (_, group) in grouped {
        let Some(first) = group.first() else {
            continue;
        };
        let mut importable_count = 0;
        let mut commands = Vec::new();
        for candidate in &group {
            let skip = command_skip_reason(candidate, &mut existing);
            if skip.is_none() {
                importable_count += 1;
            }
            commands.push(SourceCommandSummary {
                name: candidate.name.clone(),
                path: candidate.source_path.to_string_lossy().into_owned(),
                importable: skip.is_none(),
                skip_reason: skip,
                description: candidate.description.clone(),
                argument_hint: candidate.argument_hint.clone(),
            });
        }
        commands.sort_by(|left, right| js::locale_compare(&left.name, &right.name));
        summaries.push(SourceRootSummary {
            scope: first.source_root_scope.to_string(),
            path: first.source_root_path.to_string_lossy().into_owned(),
            discovered_count: group.len(),
            importable_count,
            skipped_count: Some(group.len() - importable_count),
            skills: None,
            commands: Some(commands),
            plugins: None,
            mcp_servers: None,
        });
    }
    sort_summaries(&mut summaries);
    summaries
}

pub fn build_plugin_summaries(
    home: &Path,
    workspace: Option<&Path>,
    agent: &str,
    candidates: &[PluginCandidate],
) -> Vec<SourceRootSummary> {
    let candidates: Vec<&PluginCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.agent == agent)
        .collect();
    let mut grouped: Vec<(String, Vec<&PluginCandidate>)> = Vec::new();
    for candidate in candidates {
        let key = format!(
            "{}:{}",
            candidate.source_root_scope,
            candidate.source_root_path.to_string_lossy()
        );
        match grouped.iter_mut().find(|(existing, _)| existing == &key) {
            Some((_, group)) => group.push(candidate),
            None => grouped.push((key, vec![candidate])),
        }
    }
    let mut summaries = Vec::new();
    let mut existing: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
    for (_, group) in grouped {
        let Some(first) = group.first() else {
            continue;
        };
        let mut importable_count = 0;
        let mut plugins = Vec::new();
        for candidate in &group {
            let skip = plugin_skip_reason(
                candidate,
                &mut existing,
                home,
                workspace,
                candidate.source_root_scope,
            );
            if skip.is_none() {
                importable_count += 1;
            }
            plugins.push(SourcePluginSummary {
                name: candidate.name.clone(),
                path: candidate.source_path.to_string_lossy().into_owned(),
                importable: skip.is_none(),
                skip_reason: skip,
                version: candidate.version.clone(),
            });
        }
        plugins.sort_by(|left, right| js::locale_compare(&left.name, &right.name));
        summaries.push(SourceRootSummary {
            scope: first.source_root_scope.to_string(),
            path: first.source_root_path.to_string_lossy().into_owned(),
            discovered_count: group.len(),
            importable_count,
            skipped_count: Some(group.len() - importable_count),
            skills: None,
            commands: None,
            plugins: Some(plugins),
            mcp_servers: None,
        });
    }
    sort_summaries(&mut summaries);
    summaries
}

pub fn build_mcp_summaries(
    home: &Path,
    workspace: Option<&Path>,
    agent: &str,
    candidates: &[McpCandidate],
) -> Vec<SourceRootSummary> {
    let candidates: Vec<&McpCandidate> = candidates
        .iter()
        .filter(|candidate| candidate.agent == agent)
        .collect();
    let mut grouped: Vec<(String, Vec<&McpCandidate>)> = Vec::new();
    for candidate in candidates {
        let key = format!(
            "{}:{}",
            candidate.source_root_scope,
            candidate.source_root_path.to_string_lossy()
        );
        match grouped.iter_mut().find(|(existing, _)| existing == &key) {
            Some((_, group)) => group.push(candidate),
            None => grouped.push((key, vec![candidate])),
        }
    }
    let mut summaries = Vec::new();
    let mut existing: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (_, group) in grouped {
        let Some(first) = group.first() else {
            continue;
        };
        let mut importable_count = 0;
        let mut servers = Vec::new();
        for candidate in &group {
            let skip = mcp_skip_reason(
                candidate,
                &mut existing,
                home,
                workspace,
                candidate.source_root_scope,
            );
            if skip.is_none() {
                importable_count += 1;
            }
            servers.push(SourceMcpServerSummary {
                name: candidate.name.clone(),
                path: candidate.source_path.clone(),
                importable: skip.is_none(),
                skip_reason: skip,
            });
        }
        servers.sort_by(|left, right| js::locale_compare(&left.name, &right.name));
        summaries.push(SourceRootSummary {
            scope: first.source_root_scope.to_string(),
            path: first.source_root_path.to_string_lossy().into_owned(),
            discovered_count: group.len(),
            importable_count,
            skipped_count: Some(group.len() - importable_count),
            skills: None,
            commands: None,
            plugins: None,
            mcp_servers: Some(servers),
        });
    }
    sort_summaries(&mut summaries);
    summaries
}

pub fn build_skills_discovery(home: &Path, workspace: Option<&Path>) -> DiscoveryResult {
    let candidates = collect_skill_candidates(home, workspace);
    let mut agents = Vec::new();
    for source in SUPPORTED_SKILL_AGENT_SOURCES {
        let agent_candidates: Vec<&SkillCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.agent == source.agent)
            .collect();
        let discovered_count = agent_candidates.len();
        let mut existing: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
        let mut importable_count = 0;
        for candidate in &agent_candidates {
            if skill_skip_reason(candidate, &mut existing).is_none() {
                importable_count += 1;
            }
        }
        let source_paths = sorted_source_paths(
            &agent_candidates
                .iter()
                .map(|candidate| (candidate.agent, candidate.source_root_path.clone()))
                .collect::<Vec<_>>(),
        );
        let source_roots = build_skill_summaries(home, workspace, source.agent, &candidates);
        agents.push(AgentSummary {
            agent: source.agent.to_string(),
            discovered: discovered_count > 0,
            categories: vec![CategorySummary {
                category: "skills".to_string(),
                discovered_count,
                importable_count,
                skipped_count: Some(discovered_count - importable_count),
                source_paths: Some(source_paths),
                source_roots: Some(source_roots),
                selected_by_default: importable_count > 0,
            }],
        });
    }
    DiscoveryResult {
        agents: agents
            .into_iter()
            .filter(|agent| agent.discovered)
            .collect(),
    }
}

pub fn build_commands_discovery(home: &Path, workspace: Option<&Path>) -> DiscoveryResult {
    let candidates = collect_command_candidates(home, workspace);
    let mut agents = Vec::new();
    for source in SUPPORTED_COMMAND_AGENT_SOURCES {
        let agent_candidates: Vec<&CommandCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.agent == source.agent)
            .collect();
        let discovered_count = agent_candidates.len();
        let mut existing: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
        let mut importable_count = 0;
        for candidate in &agent_candidates {
            if command_skip_reason(candidate, &mut existing).is_none() {
                importable_count += 1;
            }
        }
        let source_paths = sorted_source_paths(
            &agent_candidates
                .iter()
                .map(|candidate| (candidate.agent, candidate.source_root_path.clone()))
                .collect::<Vec<_>>(),
        );
        let source_roots = build_command_summaries(source.agent, &candidates);
        agents.push(AgentSummary {
            agent: source.agent.to_string(),
            discovered: discovered_count > 0,
            categories: vec![CategorySummary {
                category: "commands".to_string(),
                discovered_count,
                importable_count,
                skipped_count: Some(discovered_count - importable_count),
                source_paths: Some(source_paths),
                source_roots: Some(source_roots),
                selected_by_default: false,
            }],
        });
    }
    DiscoveryResult {
        agents: agents
            .into_iter()
            .filter(|agent| agent.discovered)
            .collect(),
    }
}

pub fn build_plugins_discovery(home: &Path, workspace: Option<&Path>) -> DiscoveryResult {
    let candidates = collect_plugin_candidates(home, workspace);
    let mut agents = Vec::new();
    for source in SUPPORTED_PLUGIN_AGENT_SOURCES {
        let agent_candidates: Vec<&PluginCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.agent == source.agent)
            .collect();
        let discovered_count = agent_candidates.len();
        let mut existing: HashMap<PathBuf, BTreeSet<String>> = HashMap::new();
        let mut importable_count = 0;
        for candidate in &agent_candidates {
            if plugin_skip_reason(
                candidate,
                &mut existing,
                home,
                workspace,
                candidate.source_root_scope,
            )
            .is_none()
            {
                importable_count += 1;
            }
        }
        let source_paths = sorted_source_paths(
            &agent_candidates
                .iter()
                .map(|candidate| (candidate.agent, candidate.source_root_path.clone()))
                .collect::<Vec<_>>(),
        );
        let source_roots = build_plugin_summaries(home, workspace, source.agent, &candidates);
        agents.push(AgentSummary {
            agent: source.agent.to_string(),
            discovered: discovered_count > 0,
            categories: vec![CategorySummary {
                category: "plugins".to_string(),
                discovered_count,
                importable_count,
                skipped_count: Some(discovered_count - importable_count),
                source_paths: Some(source_paths),
                source_roots: Some(source_roots),
                selected_by_default: false,
            }],
        });
    }
    DiscoveryResult {
        agents: agents
            .into_iter()
            .filter(|agent| agent.discovered)
            .collect(),
    }
}

pub fn build_mcp_discovery(home: &Path, workspace: Option<&Path>) -> DiscoveryResult {
    let candidates = collect_mcp_candidates(home, workspace);
    let mut agents = Vec::new();
    for source in SUPPORTED_MCP_AGENT_SOURCES {
        let agent_candidates: Vec<&McpCandidate> = candidates
            .iter()
            .filter(|candidate| candidate.agent == source.agent)
            .collect();
        let discovered_count = agent_candidates.len();
        let mut existing: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut importable_count = 0;
        for candidate in &agent_candidates {
            if mcp_skip_reason(
                candidate,
                &mut existing,
                home,
                workspace,
                candidate.source_root_scope,
            )
            .is_none()
            {
                importable_count += 1;
            }
        }
        let source_paths = sorted_source_paths(
            &agent_candidates
                .iter()
                .map(|candidate| (candidate.agent, candidate.source_root_path.clone()))
                .collect::<Vec<_>>(),
        );
        let source_roots = build_mcp_summaries(home, workspace, source.agent, &candidates);
        agents.push(AgentSummary {
            agent: source.agent.to_string(),
            discovered: discovered_count > 0,
            categories: vec![CategorySummary {
                category: "mcpServers".to_string(),
                discovered_count,
                importable_count,
                skipped_count: Some(discovered_count - importable_count),
                source_paths: Some(source_paths),
                source_roots: Some(source_roots),
                selected_by_default: false,
            }],
        });
    }
    DiscoveryResult {
        agents: agents
            .into_iter()
            .filter(|agent| agent.discovered)
            .collect(),
    }
}
