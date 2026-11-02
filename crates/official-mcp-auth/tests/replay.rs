//! 与 TS `official-mcp-auth.ts` 的对拍：`pnpm official-mcp-auth:fixtures` 生成的夹具。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use official_mcp_auth::{
    find_official_mcp_reserved_headers, is_official_mcp_origin_trusted,
    is_official_mcp_reserved_header_name, read_mode_mcp_request_context_meta,
    summarize_official_mcp_identity_headers, IsOfficialMcpOriginTrustedInput, ModeApiOriginFuture,
    ModeApiOriginResolver, OfficialMcpTrustedOriginRegistry,
};
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Deserialize)]
struct Fixture {
    headers: Vec<HeaderKey>,
    find: Vec<FindCase>,
    summaries: Vec<SummaryCase>,
    meta: Vec<MetaCase>,
    trust: Vec<TrustCase>,
    registry: Vec<RegistryCase>,
}

#[derive(Debug, Deserialize)]
struct HeaderKey {
    key: String,
    reserved: bool,
}

#[derive(Debug, Deserialize)]
struct FindCase {
    name: String,
    headers: BTreeMap<String, String>,
    expected: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SummaryCase {
    name: String,
    headers: BTreeMap<String, String>,
    expected: Value,
}

#[derive(Debug, Deserialize)]
struct MetaCase {
    name: String,
    meta: Map<String, Value>,
    expected: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrustInput {
    origin: String,
    mode_api_origin: Option<String>,
    dev_trusted_origins_raw: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TrustCase {
    input: TrustInput,
    expected: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryInput {
    resolver: String,
    value: Option<String>,
    origin: String,
    dev_trusted_origins_raw: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegistryCase {
    input: RegistryInput,
    expected: Value,
}

struct StubResolver {
    result: Result<Option<String>, String>,
}

impl ModeApiOriginResolver for StubResolver {
    fn resolve_mode_api_origin(&self) -> ModeApiOriginFuture<'_> {
        let result = self.result.clone();
        Box::pin(async move { result })
    }
}

fn trust_value(result: official_mcp_auth::OfficialMcpTrustResult) -> Value {
    serde_json::to_value(result).expect("序列化信任结果")
}

#[test]
fn matches_ts_official_mcp_auth() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("official-mcp-auth.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.headers {
        assert_eq!(
            is_official_mcp_reserved_header_name(&case.key),
            case.reserved,
            "{}: reserved header",
            case.key
        );
    }
    for case in &fixture.find {
        assert_eq!(
            find_official_mcp_reserved_headers(&case.headers),
            case.expected,
            "{}: find reserved",
            case.name
        );
    }
    for case in &fixture.summaries {
        let summary = summarize_official_mcp_identity_headers(&case.headers);
        assert_eq!(
            serde_json::to_value(summary).expect("序列化摘要"),
            case.expected,
            "{}: summary",
            case.name
        );
    }
    for case in &fixture.meta {
        assert_eq!(
            read_mode_mcp_request_context_meta(Some(&case.meta)),
            case.expected,
            "{}: request context meta",
            case.name
        );
    }
    for case in &fixture.trust {
        let result = is_official_mcp_origin_trusted(&IsOfficialMcpOriginTrustedInput {
            dev_trusted_origins_raw: case.input.dev_trusted_origins_raw.as_deref(),
            origin: &case.input.origin,
            plugin_id: "plugin-x",
            mode_api_origin: case.input.mode_api_origin.as_deref(),
        });
        assert_eq!(trust_value(result), case.expected, "trust case");
    }
}

#[tokio::test]
async fn matches_ts_official_mcp_auth_registry() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("official-mcp-auth.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.registry {
        let result = match case.input.resolver.as_str() {
            "ok" => Ok(case.input.value.clone()),
            "none" => Ok(None),
            "error" => Err("resolver failed".to_string()),
            other => panic!("未知 resolver 类型: {other}"),
        };
        let registry = OfficialMcpTrustedOriginRegistry::new(
            case.input.dev_trusted_origins_raw.clone(),
            Arc::new(StubResolver { result }),
        );
        let actual = registry.is_trusted(&case.input.origin, "plugin-x").await;
        assert_eq!(trust_value(actual), case.expected, "registry case");
    }
}
