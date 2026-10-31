//! MCP 配置文件路径与目录描述符。
//!
//! 真源：`mcpSyncService.ts` 的目录描述符、`buildDirectoryConfigPath`、
//! `buildDirectoryMcpLocation` 与路径改写辅助。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use plugin_discovery::js;

/// `mcp.servers`（Mode 自己的 config.json）或顶层 `mcpServers`（.agents/mcp.json）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigKey {
    McpServersNested,
    McpServersFlat,
}

#[derive(Debug, Clone, Copy)]
pub struct DirectoryDescriptor {
    pub source: &'static str,
    pub user_segments: &'static [&'static str],
    pub workspace_segments: &'static [&'static str],
    pub file_name: &'static str,
    pub config_key: ConfigKey,
}

pub const MODE_DESCRIPTOR: DirectoryDescriptor = DirectoryDescriptor {
    source: "mode",
    user_segments: &[".mode", "cli"],
    workspace_segments: &[".mode"],
    file_name: "config.json",
    config_key: ConfigKey::McpServersNested,
};

pub const AGENTS_DESCRIPTOR: DirectoryDescriptor = DirectoryDescriptor {
    source: "agents",
    user_segments: &[".agents"],
    workspace_segments: &[".agents"],
    file_name: "mcp.json",
    config_key: ConfigKey::McpServersFlat,
};

pub const DIRECTORY_DESCRIPTORS: [DirectoryDescriptor; 2] = [MODE_DESCRIPTOR, AGENTS_DESCRIPTOR];

/// `resolveUserHomeDir()`：`HOME.trim() || USERPROFILE.trim() || homedir()`。
pub fn resolve_user_home_dir(explicit: Option<&Path>) -> PathBuf {
    if let Some(home) = explicit {
        if !js::js_trim(&home.to_string_lossy()).is_empty() {
            return home.to_path_buf();
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|value| js::js_trim(&value).to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var("USERPROFILE")
                .ok()
                .map(|value| js::js_trim(&value).to_string())
                .filter(|value| !value.is_empty())
        })
        .map(PathBuf::from)
        .unwrap_or_else(js::node_homedir)
}

pub fn build_directory_config_path(
    home_dir: &Path,
    descriptor: DirectoryDescriptor,
    scope: &str,
    workspace_path: Option<&str>,
) -> Result<PathBuf, String> {
    let (base_dir, segments): (PathBuf, &[&str]) = if scope == "user" {
        (home_dir.to_path_buf(), descriptor.user_segments)
    } else {
        let Some(workspace_path) = workspace_path else {
            return Err(format!(
                "Missing workspace path for {} workspace MCP config",
                descriptor.source
            ));
        };
        (PathBuf::from(workspace_path), descriptor.workspace_segments)
    };
    let mut path = base_dir;
    for segment in segments {
        path = path.join(segment);
    }
    Ok(path.join(descriptor.file_name))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDirectoryLocation {
    pub source: String,
    pub scope: String,
    pub directory_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

pub fn build_directory_mcp_location(
    home_dir: &Path,
    descriptor: DirectoryDescriptor,
    scope: &str,
    workspace_path: Option<&str>,
) -> Result<SettingsDirectoryLocation, String> {
    let file_path = build_directory_config_path(home_dir, descriptor, scope, workspace_path)?;
    Ok(SettingsDirectoryLocation {
        source: descriptor.source.to_string(),
        scope: if scope == "workspace" {
            "project".to_string()
        } else {
            "user".to_string()
        },
        directory_path: file_path
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned())
            .unwrap_or_default(),
        project_path: workspace_path.map(str::to_string),
    })
}

pub fn find_descriptor_by_location(
    location: &SettingsDirectoryLocation,
) -> Result<DirectoryDescriptor, String> {
    DIRECTORY_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.source == location.source)
        .copied()
        .ok_or_else(|| {
            format!(
                "Unsupported MCP settings directory source: {}",
                location.source
            )
        })
}

pub fn user_mode_config_path(home_dir: &Path) -> PathBuf {
    build_directory_config_path(home_dir, MODE_DESCRIPTOR, "user", None)
        .expect("user config path has no workspace requirement")
}

/// `rewriteFilesystemMcpConfig`：仅重写 filesystem MCP 的 stdio args 路径。
pub fn rewrite_filesystem_mcp_config(
    name: &str,
    config: &Value,
    paths: &PathRewritePaths,
) -> Value {
    let Some(object) = config.as_object() else {
        return config.clone();
    };
    if !is_stdio_mcp_config(object) || !is_filesystem_mcp_server(name, object) {
        return config.clone();
    }
    let Some(args) = object.get("args").and_then(Value::as_array) else {
        return config.clone();
    };
    let rewritten: Vec<Value> = args
        .iter()
        .map(|arg| match arg.as_str() {
            Some(text) => Value::String(rewrite_path_arg_for_remote(text, paths)),
            None => arg.clone(),
        })
        .collect();
    let mut next = object.clone();
    next.insert("args".to_string(), Value::Array(rewritten));
    Value::Object(next)
}

pub struct PathRewritePaths {
    pub local_home_dir: String,
    pub local_workspace_path: Option<String>,
    pub remote_home_dir: String,
    pub remote_workspace_path: Option<String>,
}

fn is_stdio_mcp_config(object: &serde_json::Map<String, Value>) -> bool {
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    if kind.is_empty() {
        return object
            .get("command")
            .and_then(Value::as_str)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
    }
    kind == "stdio"
}

fn is_filesystem_mcp_server(name: &str, object: &serde_json::Map<String, Value>) -> bool {
    let normalized = name.trim().to_lowercase();
    if matches!(normalized.as_str(), "filesystem" | "file-system" | "fs") {
        return true;
    }
    let mut haystack = String::new();
    if let Some(command) = object.get("command").and_then(Value::as_str) {
        haystack.push_str(command);
        haystack.push(' ');
    }
    if let Some(args) = object.get("args").and_then(Value::as_array) {
        for arg in args {
            if let Some(text) = arg.as_str() {
                haystack.push_str(text);
                haystack.push(' ');
            }
        }
    }
    let haystack = haystack.to_lowercase();
    haystack.contains("@modelcontextprotocol/server-filesystem")
        || haystack.contains("mcp-server-filesystem")
}

fn rewrite_path_arg_for_remote(arg: &str, paths: &PathRewritePaths) -> String {
    if let Some(relative) = get_relative_path_if_within(paths.local_workspace_path.as_deref(), arg)
    {
        if let Some(remote_workspace) = paths
            .remote_workspace_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return join_remote_path(remote_workspace, &relative);
        }
    }
    if let Some(relative) = get_relative_path_if_within(Some(&paths.local_home_dir), arg) {
        return join_remote_path(&paths.remote_home_dir, &relative);
    }
    arg.to_string()
}

fn get_relative_path_if_within(base_path: Option<&str>, candidate_path: &str) -> Option<String> {
    let base_path = base_path?.trim();
    if base_path.is_empty() {
        return None;
    }
    let base = normalize_comparable_path(base_path)?;
    let candidate = normalize_comparable_path(candidate_path)?;
    let (base_value, candidate_value) = if base.case_insensitive {
        (base.value.to_lowercase(), candidate.value.to_lowercase())
    } else {
        (base.value.clone(), candidate.value.clone())
    };
    if candidate_value == base_value {
        return Some(String::new());
    }
    let prefix = if base_value.ends_with('/') {
        base_value
    } else {
        format!("{base_value}/")
    };
    if !candidate_value.starts_with(&prefix) {
        return None;
    }
    Some(candidate.value[prefix.len()..].to_string())
}

struct ComparablePath {
    value: String,
    case_insensitive: bool,
}

fn normalize_comparable_path(raw: &str) -> Option<ComparablePath> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let is_windows = is_windows_absolute(trimmed);
    let is_posix = trimmed.starts_with('/');
    if !is_windows && !is_posix {
        return None;
    }
    let mut value = trimmed.replace('\\', "/");
    while value.ends_with('/') {
        value.pop();
    }
    if value.is_empty() {
        value = "/".to_string();
    }
    Some(ComparablePath {
        value,
        case_insensitive: is_windows,
    })
}

fn is_windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/'))
        || value.starts_with("\\\\")
}

fn join_remote_path(remote_base_path: &str, relative_path: &str) -> String {
    if relative_path.is_empty() {
        return remote_base_path.to_string();
    }
    let segments: Vec<&str> = relative_path
        .split(['\\', '/'])
        .filter(|segment| !segment.is_empty())
        .collect();
    let base = remote_base_path.trim();
    if base.starts_with('/') {
        return posix_join(&base.replace('\\', "/"), &segments);
    }
    if is_windows_absolute(base) {
        return win32_join(base, &segments);
    }
    posix_join(&base.replace('\\', "/"), &segments)
}

fn posix_join(base: &str, segments: &[&str]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for part in base.split('/').chain(segments.iter().copied()) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if !parts.is_empty() {
                parts.pop();
            }
            continue;
        }
        parts.push(part.to_string());
    }
    format!("/{}", parts.join("/"))
}

fn win32_join(base: &str, segments: &[&str]) -> String {
    let normalized_base = base.replace('/', "\\");
    let mut parts: Vec<String> = Vec::new();
    for part in normalized_base.split('\\').chain(segments.iter().copied()) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if parts.len() > 1 {
                parts.pop();
            }
            continue;
        }
        parts.push(part.to_string());
    }
    parts.join("\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_filesystem_args_between_hosts() {
        let paths = PathRewritePaths {
            local_home_dir: "C:/Users/me".to_string(),
            local_workspace_path: Some("C:/work/project".to_string()),
            remote_home_dir: "/home/me".to_string(),
            remote_workspace_path: Some("/srv/project".to_string()),
        };
        let config = serde_json::json!({
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "C:/work/project/docs", "C:/Users/me/data"],
        });
        let rewritten = rewrite_filesystem_mcp_config("filesystem", &config, &paths);
        assert_eq!(
            rewritten["args"],
            serde_json::json!([
                "-y",
                "@modelcontextprotocol/server-filesystem",
                "/srv/project/docs",
                "/home/me/data"
            ])
        );
    }

    #[test]
    fn non_filesystem_configs_are_untouched() {
        let paths = PathRewritePaths {
            local_home_dir: "C:/Users/me".to_string(),
            local_workspace_path: None,
            remote_home_dir: "/home/me".to_string(),
            remote_workspace_path: None,
        };
        let config = serde_json::json!({ "command": "node", "args": ["C:/Users/me/x"] });
        assert_eq!(
            rewrite_filesystem_mcp_config("other", &config, &paths),
            config
        );
    }

    #[test]
    fn remote_join_keeps_remote_style() {
        assert_eq!(join_remote_path("/srv", "a/b"), "/srv/a/b");
        assert_eq!(join_remote_path("C:\\srv", "a/b"), "C:\\srv\\a\\b");
        assert_eq!(join_remote_path("/srv", ""), "/srv");
    }
}
