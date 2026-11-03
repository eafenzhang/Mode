//! Mode 官方 Server MCP 额度：归属匹配、总额度映射与响应信封解析。
//!
//! 真源：`packages/services/src/usage-stats/providers/modeMcpQuotaProvider.ts`。
//! 网络请求、凭证读取、日志与 TTL 缓存在宿主编排层；本模块只做纯投影。

use serde_json::Value;

use crate::quota::UsageQuotaLimit;

pub const MCP_USAGE_QUOTA_LIMIT_TYPE: &str = "MCP_USAGE_LIMIT";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageMcpQuotaScope {
    pub provider_family: String,
    pub target_type: String,
    pub organization_id: Option<String>,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpQuotaRequestScope {
    pub provider_family: String,
    pub organization_id: Option<String>,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct McpUsageTotal {
    pub used: f64,
    pub limit: f64,
    pub remaining: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpUsageSnapshot {
    pub aggregate: UsageQuotaLimit,
    pub level: Option<String>,
    /// Unix 毫秒（TS `serverTime`）。
    pub server_time: f64,
}

/// 凭证归属是否与本次 entitlement 查询一致；不一致时该 tab 不应展示 MCP 额度。
pub fn matches_mcp_quota_scope(scope: &UsageMcpQuotaScope, request: &McpQuotaRequestScope) -> bool {
    if scope.provider_family != request.provider_family {
        return false;
    }
    let request_organization_id = request
        .organization_id
        .as_deref()
        .map(str::trim)
        .unwrap_or("");
    let request_project_id = request.project_id.as_deref().map(str::trim).unwrap_or("");
    let request_is_team = !request_organization_id.is_empty() && !request_project_id.is_empty();
    if !request_is_team {
        return scope.target_type == "PERSONAL";
    }
    if scope.target_type != "TEAM" {
        return false;
    }
    scope
        .organization_id
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        == request_organization_id
        && scope.project_id.as_deref().map(str::trim).unwrap_or("") == request_project_id
}

/// 服务端汇总总额度映射成一条 `UsageQuotaLimit`；`percentage` 为已使用占比。
pub fn build_mcp_quota_aggregate_limit(
    total: &McpUsageTotal,
    next_reset_time_ms: Option<f64>,
) -> Option<UsageQuotaLimit> {
    let limit = total.limit.max(0.0);
    if limit <= 0.0 {
        return None;
    }
    let remaining = total.remaining.max(0.0).min(limit);
    let used = total.used.max(0.0);
    let percentage = (100.0 - (remaining / limit) * 100.0).clamp(0.0, 100.0);
    Some(UsageQuotaLimit {
        limit_type: MCP_USAGE_QUOTA_LIMIT_TYPE.to_string(),
        unit: None,
        number: None,
        usage: Some(used),
        current_value: Some(used),
        remaining: Some(remaining),
        percentage: Some(percentage),
        next_reset_time: next_reset_time_ms,
        usage_details: Vec::new(),
    })
}

/// 解析 `GET /api/v1/mcp/usage` 的响应 JSON；schema、业务码或缺失总额度都返回 `None`。
pub fn parse_mcp_usage_envelope(payload: &Value) -> Option<McpUsageSnapshot> {
    let envelope = payload.as_object()?;
    let code = finite_number(envelope.get("code"))?;
    if code != 0.0 {
        return None;
    }
    if let Some(message) = envelope.get("msg") {
        message.as_str()?;
    }
    let data = match envelope.get("data") {
        None | Some(Value::Null) => return None,
        Some(value) => value.as_object()?,
    };
    let server_time = finite_number(data.get("server_time"))?;
    let next_refresh_at = match data.get("next_refresh_at") {
        None => None,
        Some(value) => Some(finite_number(Some(value))?),
    };
    let level = match data.get("level") {
        None => None,
        Some(value) => {
            let text = value.as_str()?.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
    };
    let total_usage = data.get("total_usage")?.as_object()?;
    let total = McpUsageTotal {
        used: finite_number(total_usage.get("used"))?,
        limit: finite_number(total_usage.get("limit"))?,
        remaining: finite_number(total_usage.get("remaining"))?,
    };
    let aggregate =
        build_mcp_quota_aggregate_limit(&total, next_refresh_at.map(|value| value * 1000.0))?;
    Some(McpUsageSnapshot {
        aggregate,
        level,
        server_time: server_time * 1000.0,
    })
}

fn finite_number(value: Option<&Value>) -> Option<f64> {
    value?.as_f64().filter(|number| number.is_finite())
}
