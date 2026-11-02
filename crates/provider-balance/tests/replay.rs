//! 与 TS `providerBalanceSpecs.ts` 的对拍：
//! `pnpm provider-balance:fixtures` 生成的 `fixtures/provider-balance.json`。

use std::path::Path;

use std::sync::Arc;

use provider_balance::{
    parse_provider_balance, provider_balance_target_from_view, resolve_provider_balance,
    BalanceFuture, FetchError, ProviderBalanceFetcher, ProviderBalanceProvider,
    ProviderBalanceTarget, ProviderBalanceTargetResolver,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    resolve: Vec<ResolveCase>,
    parse: Vec<ParseCase>,
    service: Vec<ServiceCase>,
    targets: Vec<TargetCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetCase {
    name: String,
    view: Value,
    provider_id: String,
    expected: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServiceCase {
    name: String,
    provider_id: String,
    now: i64,
    resolver: Value,
    response: Value,
    expected: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetInput {
    provider_name: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
}

struct ResolverStub {
    kind: String,
    target: Option<TargetInput>,
}

impl ProviderBalanceTargetResolver for ResolverStub {
    fn resolve_target(
        &self,
        provider_id: &str,
    ) -> BalanceFuture<'_, Result<Option<ProviderBalanceTarget>, String>> {
        let kind = self.kind.clone();
        let target = self.target.as_ref().map(|target| ProviderBalanceTarget {
            provider_id: provider_id.to_string(),
            provider_name: target.provider_name.clone(),
            base_url: target.base_url.clone(),
            api_key: target.api_key.clone(),
        });
        Box::pin(async move {
            match kind.as_str() {
                "null" => Ok(None),
                "throw" => Err("resolver failed".to_string()),
                _ => Ok(target),
            }
        })
    }
}

struct FetcherStub {
    kind: String,
    status: Option<u16>,
    payload: Option<Value>,
}

impl ProviderBalanceFetcher for FetcherStub {
    fn fetch_balance(
        &self,
        _url: &str,
        _api_key: &str,
    ) -> BalanceFuture<'_, Result<Value, FetchError>> {
        let kind = self.kind.clone();
        let status = self.status;
        let payload = self.payload.clone();
        Box::pin(async move {
            match kind.as_str() {
                "network" => Err(FetchError::Network),
                "status" => Err(FetchError::HttpStatus(status.unwrap_or(500))),
                _ => {
                    let status = status.unwrap_or(200);
                    if status != 200 {
                        return Err(FetchError::HttpStatus(status));
                    }
                    payload.ok_or(FetchError::Network)
                }
            }
        })
    }
}

fn resolver_from(case: &ServiceCase) -> Option<Arc<dyn ProviderBalanceTargetResolver>> {
    let kind = case.resolver["kind"].as_str().expect("resolver kind");
    if kind == "absent" {
        return None;
    }
    let target = case
        .resolver
        .get("target")
        .filter(|target| !target.is_null())
        .and_then(|target| serde_json::from_value::<TargetInput>(target.clone()).ok());
    Some(Arc::new(ResolverStub {
        kind: kind.to_string(),
        target,
    }))
}

fn fetcher_from(case: &ServiceCase) -> Arc<dyn ProviderBalanceFetcher> {
    Arc::new(FetcherStub {
        kind: case.response["kind"]
            .as_str()
            .expect("response kind")
            .to_string(),
        status: case.response["status"].as_u64().map(|status| status as u16),
        payload: case.response.get("payload").cloned(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveCase {
    base_url: String,
    expected: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParseCase {
    name: String,
    base_url: String,
    payload: Value,
    expected: Option<Value>,
}

#[tokio::test]
async fn matches_ts_provider_balance_provider() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("provider-balance.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.service {
        let now = case.now;
        let provider = ProviderBalanceProvider::new(
            resolver_from(case),
            fetcher_from(case),
            Arc::new(move || now),
        );
        let snapshot = provider.get_snapshot(&case.provider_id).await;
        let actual = serde_json::to_value(snapshot).expect("序列化快照");
        assert_eq!(actual, case.expected, "{}: 余额快照不一致", case.name);
    }
}

#[test]
fn matches_ts_provider_balance_specs() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("provider-balance.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.resolve {
        let actual = resolve_provider_balance(Some(&case.base_url));
        match (&actual, &case.expected) {
            (None, None) => {}
            (Some(resolved), Some(expected)) => {
                let actual = json!({ "url": resolved.url, "host": resolved.host });
                assert_eq!(actual, *expected, "{}: 识别结果不一致", case.base_url);
            }
            _ => panic!(
                "{}: 识别结果存在性不一致（Rust: {}）",
                case.base_url,
                actual.is_some()
            ),
        }
    }

    assert!(resolve_provider_balance(None).is_none());
    assert!(resolve_provider_balance(Some("  ")).is_none());

    for case in &fixture.parse {
        let resolved = resolve_provider_balance(Some(&case.base_url));
        match (resolved, &case.expected) {
            (None, None) => {}
            (Some(resolved), Some(expected)) => {
                let entries = parse_provider_balance(&resolved, &case.payload);
                let actual = serde_json::to_value(entries).expect("序列化余额条目");
                assert_eq!(actual, *expected, "{}: 解析结果不一致", case.name);
            }
            (resolved, expected) => panic!(
                "{}: 识别/期望存在性不一致（Rust resolved={}, expected={}）",
                case.name,
                resolved.is_some(),
                expected.is_some()
            ),
        }
    }
}

#[test]
fn matches_ts_provider_balance_target_resolver() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("provider-balance.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));

    for case in &fixture.targets {
        let actual = provider_balance_target_from_view(&case.view, &case.provider_id)
            .map(|target| serde_json::to_value(target).expect("序列化目标"));
        assert_eq!(actual, case.expected, "{}: target", case.name);
    }
}
