//! 官方 Server MCP 鉴权的共享常量、保留头与 origin 信任判定。
//!
//! 真源：`packages/shared/src/official-mcp-auth.ts`。纯函数 + 可注入 origin 解析器；
//! 不发网络请求。规范见 `docs/specs/official-mcp-auth.md`。

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use url::Url;

pub const MODE_OFFICIAL_MCP_AUTH_TYPE: &str = "mode_official";
pub const MODE_OFFICIAL_MCP_AUTH_PROVIDER_JWT_TOKEN: &str = "jwt_token";

pub const OFFICIAL_MCP_AUTH_HEADER_NAMES: [(&str, &str); 5] = [
    ("authorization", "Authorization"),
    ("codingPlanAuthorization", "X-Bigmodel-Authorization"),
    ("targetType", "Bigmodel-Target-Type"),
    ("organization", "Bigmodel-Organization"),
    ("project", "Bigmodel-Project"),
];

pub const OFFICIAL_MCP_AUTH_META_KEY: &str = "com.mode/official-mcp-auth";
pub const MODE_MCP_REQUEST_CONTEXT_META_KEY: &str = "com.mode/request-context";
pub const MODE_MCP_REQUEST_CONTEXT_META_KEY_LEGACY: &str = "com.zcode/request-context";
pub const OFFICIAL_MCP_DEV_TRUSTED_ORIGINS_ENV: &str = "MODE_OFFICIAL_MCP_DEV_TRUSTED_ORIGINS";

pub const OFFICIAL_MCP_RESERVED_HEADER_NAMES: [&str; 8] = [
    "authorization",
    "x-bigmodel-authorization",
    "bigmodel-target-type",
    "bigmodel-organization",
    "bigmodel-project",
    "x-coding-plan-api-key",
    "mcp-session-id",
    "mcp-protocol-version",
];

pub const OFFICIAL_MCP_AUTH_FAILURE_REASONS: [&str; 2] =
    ["official_auth_unavailable", "official_auth_plan_required"];
pub const OFFICIAL_MCP_AUTH_PORT_FAILURE_REASONS: [&str; 3] = [
    "official_auth_unavailable",
    "official_auth_plan_required",
    "official_mcp_origin_untrusted",
];
/// MCP adapter 侧的完整失败分类（含网络层结果）。仅用于记录状态与日志。
pub const OFFICIAL_MCP_AUTH_FAILURE_KINDS: [&str; 6] = [
    "official_auth_unavailable",
    "official_auth_plan_required",
    "official_mcp_origin_untrusted",
    "official_auth_rejected",
    "official_auth_forbidden",
    "official_auth_redirect_blocked",
];

/// 读请求上下文：新键优先，改名前的旧键兜底（`??` 语义：null 也算缺失）。
pub fn read_mode_mcp_request_context_meta(meta: Option<&Map<String, Value>>) -> Option<Value> {
    let meta = meta?;
    for key in [
        MODE_MCP_REQUEST_CONTEXT_META_KEY,
        MODE_MCP_REQUEST_CONTEXT_META_KEY_LEGACY,
    ] {
        if let Some(value) = meta.get(key).filter(|value| !value.is_null()) {
            return Some(value.clone());
        }
    }
    None
}

pub fn is_official_mcp_reserved_header_name(name: &str) -> bool {
    let normalized = name.trim().to_ascii_lowercase();
    OFFICIAL_MCP_RESERVED_HEADER_NAMES
        .iter()
        .any(|candidate| *candidate == normalized)
}

/// 返回命中的保留头（小写、去重、稳定排序）。
pub fn find_official_mcp_reserved_headers(headers: &BTreeMap<String, String>) -> Vec<String> {
    let mut hits = BTreeSet::new();
    for name in headers.keys() {
        let normalized = name.trim().to_ascii_lowercase();
        if OFFICIAL_MCP_RESERVED_HEADER_NAMES
            .iter()
            .any(|candidate| *candidate == normalized)
        {
            hits.insert(normalized);
        }
    }
    hits.into_iter().collect()
}

/// 身份头的安全日志摘要：只含 header 名、Team 成对性与 TargetType，不含任何值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialMcpIdentitySummary {
    pub identity_header_names: Vec<String>,
    pub identity_organization_present: bool,
    pub identity_project_present: bool,
    pub identity_team_paired: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_target_type: Option<String>,
}

pub fn summarize_official_mcp_identity_headers(
    headers: &BTreeMap<String, String>,
) -> OfficialMcpIdentitySummary {
    let lower: BTreeMap<String, &String> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .collect();
    let organization = lower.contains_key("bigmodel-organization");
    let project = lower.contains_key("bigmodel-project");
    OfficialMcpIdentitySummary {
        identity_header_names: lower.keys().cloned().collect(),
        identity_organization_present: organization,
        identity_project_present: project,
        identity_team_paired: organization == project,
        identity_target_type: lower
            .get("bigmodel-target-type")
            .filter(|value| !value.is_empty())
            .map(|value| (*value).clone()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OfficialMcpTrustDetail {
    Ok,
    InvalidInput,
    OriginMismatch,
    ModeOriginUnresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OfficialMcpTrustResult {
    pub trusted: bool,
    pub detail: OfficialMcpTrustDetail,
}

fn trust(detail: OfficialMcpTrustDetail, trusted: bool) -> OfficialMcpTrustResult {
    OfficialMcpTrustResult { trusted, detail }
}

fn normalize_https_origin(candidate: &str) -> Option<String> {
    let url = Url::parse(candidate).ok()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    (url.scheme() == "https").then(|| url.origin().ascii_serialization())
}

fn normalize_loopback_origin(candidate: &str) -> Option<String> {
    let url = Url::parse(candidate).ok()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    (url.scheme() == "http" && loopback).then(|| url.origin().ascii_serialization())
}

fn parse_dev_trusted_origins(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone)]
pub struct IsOfficialMcpOriginTrustedInput<'a> {
    /// 本地自测开关的原始值（通常来自 env），只放开 http loopback。
    pub dev_trusted_origins_raw: Option<&'a str>,
    pub origin: &'a str,
    /// 声明该 MCP 的插件 id；不参与信任判定，仅用于日志与凭证归属。
    pub plugin_id: &'a str,
    pub mode_api_origin: Option<&'a str>,
}

/// `isOfficialMcpOriginTrusted`。
pub fn is_official_mcp_origin_trusted(
    input: &IsOfficialMcpOriginTrustedInput<'_>,
) -> OfficialMcpTrustResult {
    let _ = input.plugin_id;
    let origin = input.origin.trim();
    if origin.is_empty() {
        return trust(OfficialMcpTrustDetail::InvalidInput, false);
    }

    // 必须先把目标 origin 限定为 loopback：否则两侧 normalize 都是 None，
    // `None == None` 会让开关放行任意 origin（含 https 远端）。
    if let Some(loopback_origin) = normalize_loopback_origin(origin) {
        for candidate in parse_dev_trusted_origins(input.dev_trusted_origins_raw) {
            if normalize_loopback_origin(&candidate).as_deref() == Some(&loopback_origin) {
                return trust(OfficialMcpTrustDetail::Ok, true);
            }
        }
    }

    let expected = input.mode_api_origin.and_then(normalize_https_origin);
    let Some(expected) = expected else {
        return trust(OfficialMcpTrustDetail::ModeOriginUnresolved, false);
    };
    if normalize_https_origin(origin).as_deref() != Some(&expected) {
        return trust(OfficialMcpTrustDetail::OriginMismatch, false);
    }
    trust(OfficialMcpTrustDetail::Ok, true)
}

pub type ModeApiOriginFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send + 'a>>;

/// 当前 Mode API origin 的解析器；两侧必须用等价口径，否则会一侧放行一侧拒绝。
pub trait ModeApiOriginResolver: Send + Sync {
    fn resolve_mode_api_origin(&self) -> ModeApiOriginFuture<'_>;
}

/// `createOfficialMcpTrustedOriginRegistry`。
pub struct OfficialMcpTrustedOriginRegistry {
    dev_trusted_origins_raw: Option<String>,
    resolver: Arc<dyn ModeApiOriginResolver>,
}

impl OfficialMcpTrustedOriginRegistry {
    pub fn new(
        dev_trusted_origins_raw: Option<String>,
        resolver: Arc<dyn ModeApiOriginResolver>,
    ) -> Self {
        Self {
            dev_trusted_origins_raw,
            resolver,
        }
    }

    pub async fn is_trusted(&self, origin: &str, plugin_id: &str) -> OfficialMcpTrustResult {
        let mode_api_origin = match self.resolver.resolve_mode_api_origin().await {
            Ok(origin) => origin,
            // 解析失败按不可信处理，绝不因为拿不到 origin 就放行。
            Err(_) => return trust(OfficialMcpTrustDetail::ModeOriginUnresolved, false),
        };
        is_official_mcp_origin_trusted(&IsOfficialMcpOriginTrustedInput {
            dev_trusted_origins_raw: self.dev_trusted_origins_raw.as_deref(),
            origin,
            plugin_id,
            mode_api_origin: mode_api_origin.as_deref(),
        })
    }
}
