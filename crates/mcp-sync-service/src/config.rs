//! MCP 配置文件的读写、历史 `enable` 折叠与候选记录。
//!
//! 真源：`mcpSyncService.ts` 的 `readJsonObject` / `readServerMap*` /
//! `writeServerMapToJson` / `migrateLegacyEnableFlag` / `setServerEnabled` /
//! `collectEffectiveUserMcpRecords` / `recordToCandidate`。
//!
//! 顶层与 `mcp`/`mcpServers` 都用 `IndexMap` 保留**文档顺序**：TS 的
//! `Object.entries` 按读入顺序产出，`loadMcpFromUserDirectory` 的数组顺序依赖它。
//! 其余嵌套对象仍按 serde_json 的字母序写出（对拍时 JSON 文件按键排序比较）。

use std::path::Path;

use atomic_file::{write_private_text_atomic, WriteOptions};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::paths::{
    build_directory_config_path, build_directory_mcp_location, resolve_user_home_dir, ConfigKey,
    DirectoryDescriptor, SettingsDirectoryLocation, AGENTS_DESCRIPTOR, MODE_DESCRIPTOR,
};
use plugin_discovery::js;

const ENABLED_KEY: &str = "enabled";
const LEGACY_ENABLE_KEY: &str = "enable";

pub type ServerMap = IndexMap<String, Value>;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct McpSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers: Option<ServerMap>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct McpDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpSection>,
    #[serde(
        default,
        rename = "mcpServers",
        skip_serializing_if = "Option::is_none"
    )]
    pub mcp_servers: Option<ServerMap>,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeMcpFileReference {
    pub format: String,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeMcpServerRecord {
    pub source: String,
    pub scope: String,
    pub name: String,
    pub config: Value,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    pub location: SettingsDirectoryLocation,
    pub file: NativeMcpFileReference,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSyncCandidate {
    pub id: String,
    pub name: String,
    pub config: Value,
    pub enabled: bool,
    pub source: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct UserMcpRecord {
    pub name: String,
    pub config: Value,
    pub enabled: bool,
    pub source: String,
    pub path: String,
}

pub fn read_json_object(path: &Path) -> Result<Option<McpDocument>, String> {
    let raw = match js::read_text(path) {
        Some(raw) => raw,
        None => {
            if std::fs::metadata(path)
                .map(|meta| meta.is_file())
                .unwrap_or(false)
            {
                return Err(format!(
                    "无法读取 MCP 配置文件 {}: read failed",
                    path.to_string_lossy()
                ));
            }
            return Ok(None);
        }
    };
    let parsed: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("无法解析 MCP 配置文件 {}: {error}", path.to_string_lossy()))?;
    if !parsed.is_object() {
        return Err(format!(
            "MCP 配置文件 {} 必须是 JSON 对象",
            path.to_string_lossy()
        ));
    }
    // 第二次解析保留文档顺序（`Value` 的对象映射按键排序）。
    serde_json::from_str::<McpDocument>(&raw)
        .map(Some)
        .map_err(|error| format!("无法解析 MCP 配置文件 {}: {error}", path.to_string_lossy()))
}

/// `writeTextAtomic`：两空格缩进 + 尾随换行，临时文件 0600（Windows 无等价位）。
pub fn write_document_private(path: &Path, document: &McpDocument) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(document).map_err(|error| error.to_string())?
    );
    write_private_text_atomic(path, &text, &WriteOptions::default())
        .map_err(|error| error.to_string())
}

pub fn read_server_map(document: &McpDocument, key: ConfigKey) -> ServerMap {
    match key {
        ConfigKey::McpServersNested => document
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.servers.clone())
            .unwrap_or_default(),
        ConfigKey::McpServersFlat => document.mcp_servers.clone().unwrap_or_default(),
    }
}

pub fn write_server_map(document: &McpDocument, key: ConfigKey, servers: ServerMap) -> McpDocument {
    let mut next = document.clone();
    match key {
        ConfigKey::McpServersFlat => {
            next.mcp_servers = Some(servers);
        }
        ConfigKey::McpServersNested => {
            let mut mcp = next.mcp.clone().unwrap_or_default();
            mcp.servers = Some(servers);
            next.mcp = Some(mcp);
        }
    }
    next
}

pub fn server_enabled(config: &Value) -> bool {
    match config.as_object() {
        Some(object) => object.get(ENABLED_KEY) != Some(&Value::Bool(false)),
        None => true,
    }
}

pub fn set_server_enabled(config: &Value, enabled: bool) -> Value {
    let mut object = config.as_object().cloned().unwrap_or_default();
    object.remove(LEGACY_ENABLE_KEY);
    object.remove(ENABLED_KEY);
    if !enabled {
        object.insert(ENABLED_KEY.to_string(), Value::Bool(false));
    }
    Value::Object(object)
}

fn migrate_legacy_enable_flag(server_map: ServerMap) -> (ServerMap, bool) {
    let mut changed = false;
    let mut migrated = ServerMap::new();
    for (name, config) in server_map {
        let Some(object) = config.as_object() else {
            migrated.insert(name, config);
            continue;
        };
        if !object.contains_key(LEGACY_ENABLE_KEY) {
            migrated.insert(name, config);
            continue;
        }
        changed = true;
        let legacy_disabled = object.get(LEGACY_ENABLE_KEY) == Some(&Value::Bool(false));
        let enabled_disabled = object.get(ENABLED_KEY) == Some(&Value::Bool(false));
        migrated.insert(
            name,
            set_server_enabled(&config, !(legacy_disabled || enabled_disabled)),
        );
    }
    (migrated, changed)
}

fn read_server_map_with_legacy_migration(
    file_path: &Path,
    document: &McpDocument,
    key: ConfigKey,
) -> ServerMap {
    let (servers, changed) = migrate_legacy_enable_flag(read_server_map(document, key));
    if !changed {
        return servers;
    }
    // 写盘失败不阻断读取：内存结果已是正确口径，下次加载会重试。
    let next = write_server_map(document, key, servers.clone());
    let _ = write_document_private(file_path, &next);
    servers
}

pub fn read_directory_servers_from_file(
    home_dir: &Path,
    descriptor: DirectoryDescriptor,
    scope: &str,
    workspace_path: Option<&str>,
) -> Result<Vec<NativeMcpServerRecord>, String> {
    let file_path = build_directory_config_path(home_dir, descriptor, scope, workspace_path)?;
    let Some(document) = read_json_object(&file_path)? else {
        return Ok(Vec::new());
    };
    let server_map =
        read_server_map_with_legacy_migration(&file_path, &document, descriptor.config_key);
    let location = build_directory_mcp_location(home_dir, descriptor, scope, workspace_path)?;
    Ok(server_map
        .into_iter()
        .map(|(name, config)| NativeMcpServerRecord {
            source: "zcodeagentmcp".to_string(),
            scope: scope.to_string(),
            name,
            enabled: server_enabled(&config),
            project_path: if scope == "workspace" {
                workspace_path.map(str::to_string)
            } else {
                None
            },
            location: location.clone(),
            file: NativeMcpFileReference {
                format: "json".to_string(),
                file_path: file_path.to_string_lossy().into_owned(),
            },
            config,
        })
        .collect())
}

pub fn read_directory_servers_from_preferred_sources(
    home_dir: &Path,
    scope: &str,
    workspace_path: Option<&str>,
) -> Result<Vec<NativeMcpServerRecord>, String> {
    let mode_servers =
        read_directory_servers_from_file(home_dir, MODE_DESCRIPTOR, scope, workspace_path)?;
    if !mode_servers.is_empty() {
        return Ok(mode_servers);
    }
    read_directory_servers_from_file(home_dir, AGENTS_DESCRIPTOR, scope, workspace_path)
}

pub fn read_user_records_from_file(
    home_dir: &Path,
    descriptor: DirectoryDescriptor,
) -> Result<Vec<UserMcpRecord>, String> {
    let file_path = build_directory_config_path(home_dir, descriptor, "user", None)?;
    let Some(document) = read_json_object(&file_path)? else {
        return Ok(Vec::new());
    };
    let server_map =
        read_server_map_with_legacy_migration(&file_path, &document, descriptor.config_key);
    Ok(server_map
        .into_iter()
        .map(|(name, config)| UserMcpRecord {
            name,
            enabled: server_enabled(&config),
            config,
            source: descriptor.source.to_string(),
            path: file_path.to_string_lossy().into_owned(),
        })
        .collect())
}

pub fn sort_records(records: &mut [UserMcpRecord]) {
    records.sort_by(|left, right| js::locale_compare(&left.name, &right.name));
}

pub fn collect_effective_user_records(home_dir: &Path) -> Result<Vec<UserMcpRecord>, String> {
    let mode_records = read_user_records_from_file(home_dir, MODE_DESCRIPTOR)?;
    if !mode_records.is_empty() {
        let mut records = mode_records;
        sort_records(&mut records);
        return Ok(records);
    }
    let mut records = read_user_records_from_file(home_dir, AGENTS_DESCRIPTOR)?;
    sort_records(&mut records);
    Ok(records)
}

pub fn collect_effective_user_records_by_name(
    home_dir: &Path,
) -> Result<Vec<(String, UserMcpRecord)>, String> {
    let mut result: Vec<(String, UserMcpRecord)> = Vec::new();
    for record in collect_effective_user_records(home_dir)? {
        let key = normalize_mcp_name_key(&record.name);
        if result.iter().all(|(existing, _)| existing != &key) {
            result.push((key, record));
        }
    }
    Ok(result)
}

pub fn normalize_mcp_name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

pub fn record_to_candidate(record: &UserMcpRecord) -> McpSyncCandidate {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}:{}", record.source, record.path, record.name).as_bytes());
    let id = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    McpSyncCandidate {
        id,
        name: record.name.clone(),
        config: record.config.clone(),
        enabled: record.enabled,
        source: record.source.clone(),
        path: record.path.clone(),
    }
}

/// `checkRemoteSyncDirectoryWriteAccess`：目录内写一个临时标记并删除。
pub fn check_directory_write_access(directory: &Path) -> Value {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let marker = directory.join(format!(
        ".mode-sync-preflight-{}-{nanos}.tmp",
        std::process::id()
    ));
    let path_text = directory.to_string_lossy().into_owned();
    let result = (|| -> Result<(), std::io::Error> {
        std::fs::create_dir_all(directory)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)?;
        use std::io::Write;
        file.write_all(b"ok")?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&marker);
    match result {
        Ok(()) => serde_json::json!({ "ok": true, "path": path_text }),
        Err(error) => {
            serde_json::json!({ "ok": false, "path": path_text, "error": error.to_string() })
        }
    }
}

pub fn resolve_home(home_override: Option<&Path>) -> std::path::PathBuf {
    resolve_user_home_dir(home_override)
}
