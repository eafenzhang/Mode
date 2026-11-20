//! `/api/server-info` 契约（LT-9）。真源：`packages/shared/src/server-remote.ts` +
//! `packages/server/src/http.ts` 的 `createServerInfo`。
//!
//! 只做传输层投影：不推断 LAN/远程桌面能力，未显式声明的可选键保持缺席（旧对端语义：
//! 缺键 = 不支持），与 TS 的 `...spread` 口径一致。

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const SERVER_REMOTE_PROTOCOL_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerRemoteWorkspaceInfo {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicRelayInfo {
    pub base_url: String,
    pub tunnel_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerRemoteCapabilities {
    pub desktop_continuous: bool,
    pub websocket_rpc: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computer_use: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_desktop: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_desktop_native: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan_peer_controller: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerRemoteInfo {
    pub server_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub version: String,
    pub protocol_version: u64,
    pub auth_required: bool,
    pub workspaces: Vec<ServerRemoteWorkspaceInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_relay: Option<PublicRelayInfo>,
    pub capabilities: ServerRemoteCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfoConfig {
    pub server_id: String,
    pub name: Option<String>,
    pub version: String,
    pub auth_required: bool,
    pub workspaces: Vec<ServerRemoteWorkspaceInfo>,
    pub public_relay: Option<PublicRelayInfo>,
    pub computer_use: Option<bool>,
    pub remote_desktop: Option<bool>,
    pub remote_desktop_native: Option<bool>,
    pub lan_peer_controller: Option<bool>,
}

impl ServerInfoConfig {
    /// 生产口径：`MODE_SERVER_*` env → hostname/cwd 兜底；空串按未设置处理。
    pub fn from_env() -> Self {
        let env: HashMap<String, String> = std::env::vars().collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        Self::from_parts(&env, &cwd, &hostname())
    }

    pub fn from_parts(env: &HashMap<String, String>, cwd: &Path, hostname: &str) -> Self {
        let server_id = read_env(env, "MODE_SERVER_ID")
            .or_else(|| (!hostname.trim().is_empty()).then(|| hostname.trim().to_string()))
            .unwrap_or_else(|| "mode-server".to_string());
        let workspace_path = read_env(env, "MODE_SERVER_WORKSPACE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| cwd.to_path_buf());
        let label = workspace_path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| {
                let display = workspace_path.display().to_string();
                (!display.is_empty()).then_some(display)
            });
        Self {
            server_id,
            name: read_env(env, "MODE_SERVER_NAME"),
            version: read_env(env, "MODE_VERSION")
                .or_else(|| read_env(env, "MODE_APP_VERSION"))
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
            auth_required: read_env(env, "MODE_SERVER_TOKEN").is_some(),
            workspaces: vec![ServerRemoteWorkspaceInfo {
                path: workspace_path.display().to_string(),
                label,
                workspace_identity: None,
            }],
            public_relay: None,
            computer_use: None,
            remote_desktop: None,
            remote_desktop_native: None,
            lan_peer_controller: None,
        }
    }

    pub fn projection(&self) -> ServerRemoteInfo {
        ServerRemoteInfo {
            server_id: self.server_id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            protocol_version: SERVER_REMOTE_PROTOCOL_VERSION,
            auth_required: self.auth_required,
            workspaces: self.workspaces.clone(),
            public_relay: self.public_relay.clone(),
            capabilities: ServerRemoteCapabilities {
                desktop_continuous: true,
                websocket_rpc: true,
                computer_use: self.computer_use,
                remote_desktop: self.remote_desktop,
                remote_desktop_native: self.remote_desktop_native,
                lan_peer_controller: self.lan_peer_controller,
            },
        }
    }
}

fn read_env(env: &HashMap<String, String>, key: &str) -> Option<String> {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn hostname() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|key| {
            std::env::var(key)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn env_precedence_and_trimming() {
        let config = ServerInfoConfig::from_parts(
            &env(&[
                ("MODE_SERVER_ID", " server-1 "),
                ("MODE_SERVER_NAME", " 我的服务 "),
                ("MODE_SERVER_TOKEN", "token"),
                ("MODE_SERVER_WORKSPACE", "C:/work/demo"),
                ("MODE_VERSION", " 9.9.9 "),
            ]),
            Path::new("C:/fallback"),
            "host",
        );
        assert_eq!(config.server_id, "server-1");
        assert_eq!(config.name.as_deref(), Some("我的服务"));
        assert_eq!(config.version, "9.9.9");
        assert!(config.auth_required);
        assert_eq!(config.workspaces[0].path, "C:/work/demo");
        assert_eq!(config.workspaces[0].label.as_deref(), Some("demo"));
    }

    #[test]
    fn blank_values_fall_back_to_hostname_and_cwd() {
        let config = ServerInfoConfig::from_parts(
            &env(&[
                ("MODE_SERVER_ID", "   "),
                ("MODE_SERVER_TOKEN", "  "),
                ("MODE_SERVER_WORKSPACE", ""),
                ("MODE_SERVER_NAME", ""),
            ]),
            Path::new("C:/fallback"),
            " host-2 ",
        );
        assert_eq!(config.server_id, "host-2");
        assert_eq!(config.name, None);
        assert!(!config.auth_required);
        assert_eq!(config.workspaces[0].path, "C:/fallback");
        assert_eq!(config.workspaces[0].label.as_deref(), Some("fallback"));
    }

    #[test]
    fn projection_omits_undeclared_capabilities_and_uses_camel_case() {
        let config = ServerInfoConfig::from_parts(
            &env(&[("MODE_SERVER_ID", "srv")]),
            Path::new("/workspace/mode"),
            "host",
        );
        let json = serde_json::to_value(config.projection()).expect("serialize");
        assert_eq!(json["serverId"], "srv");
        assert_eq!(json["protocolVersion"], SERVER_REMOTE_PROTOCOL_VERSION);
        assert_eq!(json["capabilities"]["desktopContinuous"], true);
        assert_eq!(json["capabilities"]["websocketRpc"], true);
        assert!(json["capabilities"].get("computerUse").is_none());
        assert!(json["capabilities"].get("remoteDesktop").is_none());
        assert!(json.get("name").is_none());
        assert_eq!(json["workspaces"][0]["path"], "/workspace/mode");
        assert_eq!(json["workspaces"][0]["label"], "mode");
    }
}
