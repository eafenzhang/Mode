//! `mcp/list` 的严格镜像。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的 `modeMcpListParamsSchema` /
//! `modeMcpListResultSchema` / `modeMcpServerStatusSnapshotSchema`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::session_create::McpServer;
use crate::session_read::SessionWorkspaceRef;

pub const MCP_LIST_METHOD: &str = "mcp/list";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpListMode {
    #[serde(rename = "connect")]
    Connect,
    #[serde(rename = "status")]
    Status,
}

/// `modeMcpListParamsSchema`；`mode` 在 schema 里默认 `connect`，但服务层只在下发时写字段，
/// 所以镜像用 `Option` 保留省略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpListParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<McpServer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<McpListMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpServerStatusKind {
    #[serde(rename = "connecting")]
    Connecting,
    #[serde(rename = "connected")]
    Connected,
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "disconnected")]
    Disconnected,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "untrusted")]
    Untrusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpServerFailureKind {
    #[serde(rename = "config_invalid")]
    ConfigInvalid,
    #[serde(rename = "runtime_unavailable")]
    RuntimeUnavailable,
    #[serde(rename = "process_start_failed")]
    ProcessStartFailed,
    #[serde(rename = "network_unreachable")]
    NetworkUnreachable,
    #[serde(rename = "connection_timeout")]
    ConnectionTimeout,
    #[serde(rename = "protocol_negotiation_failed")]
    ProtocolNegotiationFailed,
    #[serde(rename = "tool_list_failed")]
    ToolListFailed,
    #[serde(rename = "unexpected_disconnect")]
    UnexpectedDisconnect,
    #[serde(rename = "oauth_authorization_failed")]
    OauthAuthorizationFailed,
    #[serde(rename = "official_origin_untrusted")]
    OfficialOriginUntrusted,
    #[serde(rename = "not_authenticated")]
    NotAuthenticated,
    #[serde(rename = "coding_plan_required")]
    CodingPlanRequired,
    #[serde(rename = "server_not_found")]
    ServerNotFound,
    #[serde(rename = "server_unavailable")]
    ServerUnavailable,
    #[serde(rename = "rate_limited")]
    RateLimited,
    #[serde(rename = "server_internal_error")]
    ServerInternalError,
    #[serde(rename = "protocol_error")]
    ProtocolError,
    #[serde(rename = "status_unavailable")]
    StatusUnavailable,
    #[serde(rename = "connection_failed")]
    ConnectionFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpTransportKind {
    #[serde(rename = "stdio")]
    Stdio,
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "sse")]
    Sse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpProtocolEra {
    #[serde(rename = "legacy")]
    Legacy,
    #[serde(rename = "modern")]
    Modern,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OauthAuthorizationType {
    #[serde(rename = "oauth_authorization_code")]
    OauthAuthorizationCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpAuthorization {
    #[serde(rename = "type")]
    pub authorization_type: OauthAuthorizationType,
    pub authorization_url: String,
    pub started_at: String,
}

/// `modeMcpServerStatusSnapshotSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerStatusSnapshot {
    pub status: McpServerStatusKind,
    pub transport: McpTransportKind,
    pub tool_count: u64,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<McpServerFailureKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_era: Option<McpProtocolEra>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<McpAuthorization>,
}

/// `modeMcpListResultSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpListResult {
    pub statuses: BTreeMap<String, McpServerStatusSnapshot>,
}
