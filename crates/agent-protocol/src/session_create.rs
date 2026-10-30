//! `session/create` 的嵌套参数类型：MCP server 与导入历史。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的 `modeProtocolMcpServerSchema` /
//! `modeProtocolMcpOAuthSchema` / `modeSessionImportHistorySchema`。规范见
//! `docs/specs/session-write-group.md`。
//!
//! 都是 `.strict()`；联合类型用 `#[serde(untagged)]` + 分支 `deny_unknown_fields`。
//! `source` / `type` 的取值一律写字面量，不用 `rename_all` 推导。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpEntry {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpIsolation {
    #[serde(rename = "session")]
    Session,
    #[serde(rename = "workspace")]
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpProtocolVersion {
    #[serde(rename = "legacy")]
    Legacy,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "2026-07-28")]
    V20260728,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientCredentialsType {
    #[serde(rename = "client_credentials")]
    ClientCredentials,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorizationCodeType {
    #[serde(rename = "authorization_code")]
    AuthorizationCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpOAuthClientCredentials {
    #[serde(rename = "type")]
    pub oauth_type: ClientCredentialsType,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpOAuthAuthorizationCode {
    #[serde(rename = "type")]
    pub oauth_type: AuthorizationCodeType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// `modeProtocolMcpOAuthSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpOAuth {
    ClientCredentials(McpOAuthClientCredentials),
    AuthorizationCode(McpOAuthAuthorizationCode),
}

/// stdio 形态的 MCP server（无 `type` 字段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerStdio {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<McpEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<McpIsolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<McpProtocolVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpTransportType {
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "sse")]
    Sse,
}

/// http/sse 形态的 MCP server。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerHttp {
    pub name: String,
    #[serde(rename = "type")]
    pub transport: McpTransportType,
    pub url: String,
    pub headers: Vec<McpEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<McpIsolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<McpProtocolVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// `modeProtocolMcpServerSchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServer {
    Stdio(McpServerStdio),
    Http(McpServerHttp),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportMessageRole {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "assistant")]
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportMessage {
    pub role: ImportMessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaudeCodeSource {
    #[serde(rename = "claudeCode")]
    ClaudeCode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteMirrorSource {
    #[serde(rename = "remoteMirror")]
    RemoteMirror,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SharedContextSource {
    #[serde(rename = "sharedContext")]
    SharedContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaudeCodeImport {
    pub source: ClaudeCodeSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    pub messages: Vec<ImportMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteMirrorImport {
    pub source: RemoteMirrorSource,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    pub origin_session_id: String,
    pub messages: Vec<ImportMessage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SharedContextStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "reserved")]
    Reserved,
    #[serde(rename = "attached")]
    Attached,
    #[serde(rename = "discarded")]
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledArtifact {
    pub artifact_id: String,
    pub workspace_relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedContextProvenance {
    pub share_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SharedContextStatus>,
    pub projection_sha256: String,
    pub artifact_set_sha256: String,
    /// `z.literal(1)`。
    pub formatter_version: u64,
    pub markdown_sha256: String,
    pub installed_artifacts: Vec<InstalledArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedContextImport {
    pub source: SharedContextSource,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    pub markdown: String,
    pub provenance: SharedContextProvenance,
}

/// `modeSessionImportHistorySchema`（按 `source` 的判别联合）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ImportHistory {
    ClaudeCode(ClaudeCodeImport),
    RemoteMirror(RemoteMirrorImport),
    SharedContext(SharedContextImport),
}
