use serde_json::json;
use usage_stats::{
    build_mcp_quota_aggregate_limit, matches_mcp_quota_scope, parse_mcp_usage_envelope,
    McpQuotaRequestScope, McpUsageTotal, UsageMcpQuotaScope, MCP_USAGE_QUOTA_LIMIT_TYPE,
};

fn scope(
    family: &str,
    target: &str,
    organization: Option<&str>,
    project: Option<&str>,
) -> UsageMcpQuotaScope {
    UsageMcpQuotaScope {
        provider_family: family.to_string(),
        target_type: target.to_string(),
        organization_id: organization.map(str::to_string),
        project_id: project.map(str::to_string),
    }
}

fn request(
    family: &str,
    organization: Option<&str>,
    project: Option<&str>,
) -> McpQuotaRequestScope {
    McpQuotaRequestScope {
        provider_family: family.to_string(),
        organization_id: organization.map(str::to_string),
        project_id: project.map(str::to_string),
    }
}

#[test]
fn personal_scope_matches_only_same_family() {
    assert!(matches_mcp_quota_scope(
        &scope("zai", "PERSONAL", None, None),
        &request("zai", None, None)
    ));
    assert!(!matches_mcp_quota_scope(
        &scope("bigmodel", "PERSONAL", None, None),
        &request("zai", None, None)
    ));
    assert!(!matches_mcp_quota_scope(
        &scope("zai", "TEAM", Some("org"), Some("proj")),
        &request("zai", None, None)
    ));
}

#[test]
fn team_scope_requires_matching_trimmed_org_and_project() {
    let credential = scope("zai", "TEAM", Some(" org "), Some(" proj "));
    assert!(matches_mcp_quota_scope(
        &credential,
        &request("zai", Some("org"), Some("proj"))
    ));
    assert!(!matches_mcp_quota_scope(
        &credential,
        &request("zai", Some("org"), Some("other"))
    ));
    assert!(!matches_mcp_quota_scope(
        &scope("zai", "PERSONAL", None, None),
        &request("zai", Some("org"), Some("proj"))
    ));
}

#[test]
fn aggregate_limit_clamps_and_computes_used_percentage() {
    let limit = build_mcp_quota_aggregate_limit(
        &McpUsageTotal {
            used: 25.0,
            limit: 100.0,
            remaining: 75.0,
        },
        Some(1_700_000_000_000.0),
    )
    .expect("limit");
    assert_eq!(limit.limit_type, MCP_USAGE_QUOTA_LIMIT_TYPE);
    assert_eq!(limit.current_value, Some(25.0));
    assert_eq!(limit.usage, Some(25.0));
    assert_eq!(limit.remaining, Some(75.0));
    assert_eq!(limit.percentage, Some(25.0));
    assert_eq!(limit.next_reset_time, Some(1_700_000_000_000.0));
    assert!(limit.usage_details.is_empty());
}

#[test]
fn aggregate_limit_clamps_anomalies_and_rejects_zero_limit() {
    let clamped = build_mcp_quota_aggregate_limit(
        &McpUsageTotal {
            used: -5.0,
            limit: 100.0,
            remaining: 150.0,
        },
        None,
    )
    .expect("limit");
    assert_eq!(clamped.usage, Some(0.0));
    assert_eq!(clamped.remaining, Some(100.0));
    assert_eq!(clamped.percentage, Some(0.0));

    assert!(build_mcp_quota_aggregate_limit(
        &McpUsageTotal {
            used: 0.0,
            limit: 0.0,
            remaining: 0.0,
        },
        None,
    )
    .is_none());
}

#[test]
fn envelope_parses_seconds_to_milliseconds_and_trims_level() {
    let snapshot = parse_mcp_usage_envelope(&json!({
        "code": 0,
        "msg": "ok",
        "data": {
            "server_time": 1_700_000_000.0,
            "next_refresh_at": 1_700_000_060.0,
            "level": "  coding-plan  ",
            "total_usage": {"used": 10.0, "limit": 100.0, "remaining": 90.0}
        }
    }))
    .expect("snapshot");
    assert_eq!(snapshot.server_time, 1_700_000_000_000.0);
    assert_eq!(snapshot.level.as_deref(), Some("coding-plan"));
    assert_eq!(snapshot.aggregate.percentage, Some(10.0));
    assert_eq!(
        snapshot.aggregate.next_reset_time,
        Some(1_700_000_060_000.0)
    );
}

#[test]
fn envelope_fails_closed_on_business_or_schema_errors() {
    for payload in [
        json!({"code": 1000, "data": {"server_time": 1.0, "total_usage": {"used": 1, "limit": 2, "remaining": 1}}}),
        json!({"code": 0}),
        json!({"code": 0, "data": null}),
        json!({"code": 0, "data": {"server_time": 1.0}}),
        json!({"code": 0, "data": {"server_time": 1.0, "total_usage": {"used": 1, "limit": 2}}}),
        json!({"code": 0, "data": {"server_time": 1.0, "total_usage": {"used": "1", "limit": 2, "remaining": 1}}}),
        json!({"code": 0, "data": {"server_time": 1.0, "total_usage": {"used": 0, "limit": 0, "remaining": 0}}}),
        json!({"code": 0, "msg": 1, "data": {"server_time": 1.0, "total_usage": {"used": 1, "limit": 2, "remaining": 1}}}),
        json!({"code": 0, "data": {"server_time": 1.0, "level": 2, "total_usage": {"used": 1, "limit": 2, "remaining": 1}}}),
        json!([1, 2, 3]),
    ] {
        assert!(
            parse_mcp_usage_envelope(&payload).is_none(),
            "应拒绝：{payload}"
        );
    }
}

#[test]
fn envelope_without_optional_fields_is_valid() {
    let snapshot = parse_mcp_usage_envelope(&json!({
        "code": 0,
        "data": {
            "server_time": 10.0,
            "total_usage": {"used": 0.0, "limit": 50.0, "remaining": 50.0}
        }
    }))
    .expect("snapshot");
    assert_eq!(snapshot.level, None);
    assert_eq!(snapshot.aggregate.next_reset_time, None);
    assert_eq!(snapshot.aggregate.percentage, Some(0.0));
}
