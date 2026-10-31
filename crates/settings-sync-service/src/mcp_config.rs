//! MCP 配置读取与合并（JSON / OpenCode / Codex TOML）。
//!
//! 真源：`settingsSyncService.ts` 的 `readMcpServersFromSourceFile` /
//! `readModeMcpServers` / `readWrappedMcpServers` / `readOpenCodeMcpServers` /
//! `readTomlMcpServers` / `addMcpServerToModeConfig`。

use std::path::Path;

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::sources::McpConfigFormat;

pub type ServerMap = IndexMap<String, Value>;

pub fn normalize_mcp_server_map(value: Option<&Value>) -> ServerMap {
    let mut servers = ServerMap::new();
    let Some(object) = value.and_then(Value::as_object) else {
        return servers;
    };
    for (name, config) in object {
        if name.trim().is_empty() || !config.is_object() {
            continue;
        }
        servers.insert(name.clone(), config.clone());
    }
    servers
}

pub fn read_mode_mcp_servers(parsed: &Map<String, Value>) -> ServerMap {
    let Some(mcp) = parsed.get("mcp").and_then(Value::as_object) else {
        return ServerMap::new();
    };
    normalize_mcp_server_map(mcp.get("servers"))
}

fn read_wrapped_mcp_servers(parsed: &Map<String, Value>) -> ServerMap {
    normalize_mcp_server_map(parsed.get("mcpServers"))
}

fn normalize_open_code_mcp_config(config: &Value) -> Value {
    let Some(object) = config.as_object() else {
        return config.clone();
    };
    let raw_command = object.get("command");
    if let Some(Value::Array(items)) = raw_command {
        let strings: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
        if let Some((first, args)) = strings.split_first() {
            let mut next = object.clone();
            next.remove("command");
            let kind = object.get("type").and_then(Value::as_str);
            next.remove("type");
            next.insert("command".to_string(), Value::String((*first).to_string()));
            next.insert(
                "args".to_string(),
                Value::Array(
                    args.iter()
                        .map(|arg| Value::String((*arg).to_string()))
                        .collect(),
                ),
            );
            match kind {
                Some("local") => {
                    next.insert("type".to_string(), Value::String("stdio".to_string()));
                }
                Some("remote") => {
                    next.insert("type".to_string(), Value::String("http".to_string()));
                }
                _ => {}
            }
            return Value::Object(next);
        }
    }
    if object.get("type").and_then(Value::as_str) == Some("local") {
        if let Some(command) = object.get("command").and_then(Value::as_str) {
            let trimmed = command.trim();
            if !trimmed.is_empty() {
                let mut next = object.clone();
                next.insert("type".to_string(), Value::String("stdio".to_string()));
                next.insert("command".to_string(), Value::String(trimmed.to_string()));
                return Value::Object(next);
            }
        }
    }
    config.clone()
}

fn read_open_code_mcp_servers(parsed: &Map<String, Value>) -> ServerMap {
    normalize_mcp_server_map(parsed.get("mcp"))
        .into_iter()
        .map(|(name, config)| (name, normalize_open_code_mcp_config(&config)))
        .collect()
}

fn read_toml_mcp_servers(parsed: &Map<String, Value>) -> ServerMap {
    normalize_mcp_server_map(
        parsed
            .get("mcp_servers")
            .or_else(|| parsed.get("mcpServers")),
    )
}

fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(value) => Value::String(value),
        toml::Value::Integer(value) => Value::Number(value.into()),
        toml::Value::Float(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(value) => Value::Bool(value),
        toml::Value::Datetime(value) => Value::String(value.to_string()),
        toml::Value::Array(values) => Value::Array(values.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(key, value)| (key, toml_to_json(value)))
                .collect(),
        ),
    }
}

pub fn read_mcp_servers_from_source_file(path: &Path, format: McpConfigFormat) -> ServerMap {
    let Ok(bytes) = std::fs::read(path) else {
        return ServerMap::new();
    };
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    if format == McpConfigFormat::CodexToml {
        let Ok(parsed) = raw.parse::<toml::Value>() else {
            return ServerMap::new();
        };
        let Some(table) = parsed.as_table() else {
            return ServerMap::new();
        };
        let as_json: Map<String, Value> = table
            .iter()
            .map(|(key, value)| (key.clone(), toml_to_json(value.clone())))
            .collect();
        return read_toml_mcp_servers(&as_json);
    }
    let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&raw) else {
        return ServerMap::new();
    };
    if format == McpConfigFormat::McpJson {
        read_open_code_mcp_servers(&parsed)
    } else {
        read_wrapped_mcp_servers(&parsed)
    }
}

pub fn strip_external_mcp_timeout_fields(config: &Value) -> Value {
    let Some(object) = config.as_object() else {
        return config.clone();
    };
    let mut next = object.clone();
    next.remove("timeout");
    next.remove("startup_timeout_sec");
    Value::Object(next)
}

pub fn read_json_object_or_empty(path: &Path) -> Map<String, Value> {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(object)) => object,
            _ => Map::new(),
        },
        Err(_) => Map::new(),
    }
}

pub fn write_json_file(path: &Path, value: &Map<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(value.clone()))
            .map_err(|error| error.to_string())?
    );
    std::fs::write(path, text).map_err(|error| error.to_string())
}

pub fn add_mcp_server_to_mode_config(
    file_path: &Path,
    name: &str,
    config: &Value,
) -> Result<(), String> {
    let parsed = read_json_object_or_empty(file_path);
    let mut current_mcp = parsed
        .get("mcp")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut servers = read_mode_mcp_servers(&parsed);
    servers.insert(name.to_string(), config.clone());
    let mut next = parsed;
    current_mcp.insert(
        "servers".to_string(),
        Value::Object(servers.into_iter().collect()),
    );
    next.insert("mcp".to_string(), Value::Object(current_mcp));
    write_json_file(file_path, &next)
}

/// `addPluginDirToConfig`：把插件目录追加到 `plugins.dirs`（按 resolve 后路径去重）。
pub fn add_plugin_dir_to_config(file_path: &Path, plugin_path: &Path) -> Result<(), String> {
    let parsed = read_json_object_or_empty(file_path);
    let mut plugins = parsed
        .get("plugins")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut dirs: Vec<String> = plugins
        .get("dirs")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let resolved = crate::sources::absolute_path(plugin_path);
    let resolved_text = resolved.to_string_lossy().into_owned();
    let already = dirs.iter().any(|dir| {
        crate::sources::absolute_path(Path::new(dir)).to_string_lossy() == resolved_text
    });
    if !already {
        dirs.push(resolved_text);
    }
    plugins.insert(
        "dirs".to_string(),
        Value::Array(dirs.into_iter().map(Value::String).collect()),
    );
    let mut next = parsed;
    next.insert("plugins".to_string(), Value::Object(plugins));
    write_json_file(file_path, &next)
}
