//! 外部 Provider 余额快照编排：目标解析 → 供应商识别 → 注入的 HTTP 取回 → 投影。
//!
//! 真源：`packages/services/src/usage-stats/providers/providerBalanceProvider.ts`。
//! 网络与 Provider 设置读取由调用方注入；失败一律 fail-soft 成带 status 的空快照。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::{parse_provider_balance, resolve_provider_balance, ProviderBalanceEntry};

pub type BalanceFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBalanceTarget {
    pub provider_id: String,
    pub provider_name: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

/// 从 Provider Settings View（JSON）解析余额查询目标。
pub fn provider_balance_target_from_view(
    view: &Value,
    provider_id: &str,
) -> Option<ProviderBalanceTarget> {
    let providers = view.get("providers")?.as_array()?;
    let provider = providers
        .iter()
        .find(|provider| provider.get("providerId").and_then(Value::as_str) == Some(provider_id))?;
    let access = provider.get("effectiveConfig")?.get("access")?;
    if access.get("type").and_then(Value::as_str) != Some("api-key") {
        return None;
    }
    let provider_name = provider
        .get("providerName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(provider_id)
        .to_string();
    let base_url = provider
        .get("effectiveConfig")
        .and_then(|config| config.get("api"))
        .and_then(|api| api.get("baseUrl"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let api_key = access
        .get("apiKey")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(ProviderBalanceTarget {
        provider_id: provider_id.to_string(),
        provider_name: Some(provider_name),
        base_url,
        api_key,
    })
}

pub trait ProviderBalanceTargetResolver: Send + Sync {
    fn resolve_target(
        &self,
        provider_id: &str,
    ) -> BalanceFuture<'_, Result<Option<ProviderBalanceTarget>, String>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    HttpStatus(u16),
    Network,
}

pub trait ProviderBalanceFetcher: Send + Sync {
    fn fetch_balance(
        &self,
        url: &str,
        api_key: &str,
    ) -> BalanceFuture<'_, Result<Value, FetchError>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ProviderBalanceStatus {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "not_configured")]
    NotConfigured,
    #[serde(rename = "unauthorized")]
    Unauthorized,
    #[serde(rename = "error")]
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBalanceSnapshot {
    pub provider_id: String,
    pub provider_name: String,
    pub generated_at: i64,
    pub status: ProviderBalanceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub balances: Vec<ProviderBalanceEntry>,
}

fn empty_snapshot(
    generated_at: i64,
    provider_id: &str,
    provider_name: &str,
    status: ProviderBalanceStatus,
    message: Option<&str>,
) -> ProviderBalanceSnapshot {
    ProviderBalanceSnapshot {
        provider_id: provider_id.to_string(),
        provider_name: provider_name.to_string(),
        generated_at,
        status,
        message: message.map(str::to_string),
        balances: Vec::new(),
    }
}

pub struct ProviderBalanceProvider {
    resolver: Option<Arc<dyn ProviderBalanceTargetResolver>>,
    fetcher: Arc<dyn ProviderBalanceFetcher>,
    now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl ProviderBalanceProvider {
    pub fn new(
        resolver: Option<Arc<dyn ProviderBalanceTargetResolver>>,
        fetcher: Arc<dyn ProviderBalanceFetcher>,
        now_ms: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            resolver,
            fetcher,
            now_ms,
        }
    }

    pub async fn get_snapshot(&self, provider_id: &str) -> ProviderBalanceSnapshot {
        let generated_at = (self.now_ms)();
        let provider_id = provider_id.trim();
        let Some(resolver) = &self.resolver else {
            return empty_snapshot(
                generated_at,
                provider_id,
                provider_id,
                ProviderBalanceStatus::Unsupported,
                None,
            );
        };
        if provider_id.is_empty() {
            return empty_snapshot(
                generated_at,
                provider_id,
                provider_id,
                ProviderBalanceStatus::Unsupported,
                None,
            );
        }

        // Provider 配置读取失败属于瞬时状态：按未配置处理，不把内部错误透给 UI。
        let target = resolver.resolve_target(provider_id).await.ok().flatten();
        let provider_name = target
            .as_ref()
            .and_then(|target| target.provider_name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(provider_id)
            .to_string();
        let Some(target) = target else {
            return empty_snapshot(
                generated_at,
                provider_id,
                &provider_name,
                ProviderBalanceStatus::Unsupported,
                None,
            );
        };

        let Some(spec) = resolve_provider_balance(target.base_url.as_deref()) else {
            return empty_snapshot(
                generated_at,
                provider_id,
                &provider_name,
                ProviderBalanceStatus::Unsupported,
                None,
            );
        };

        let api_key = target.api_key.as_deref().map(str::trim).unwrap_or("");
        if api_key.is_empty() {
            return empty_snapshot(
                generated_at,
                provider_id,
                &provider_name,
                ProviderBalanceStatus::NotConfigured,
                None,
            );
        }

        let payload = match self.fetcher.fetch_balance(&spec.url, api_key).await {
            Ok(payload) => payload,
            Err(FetchError::HttpStatus(401 | 403)) => {
                return empty_snapshot(
                    generated_at,
                    provider_id,
                    &provider_name,
                    ProviderBalanceStatus::Unauthorized,
                    None,
                );
            }
            Err(_) => {
                return empty_snapshot(
                    generated_at,
                    provider_id,
                    &provider_name,
                    ProviderBalanceStatus::Error,
                    Some("request_failed"),
                );
            }
        };

        // TS 侧对 parse 异常有 invalid_response 兜底；Rust 解析器不抛异常，
        // 只可能得到空表（empty_response）。
        let balances = parse_provider_balance(&spec, &payload);
        if balances.is_empty() {
            return empty_snapshot(
                generated_at,
                provider_id,
                &provider_name,
                ProviderBalanceStatus::Error,
                Some("empty_response"),
            );
        }
        ProviderBalanceSnapshot {
            provider_id: provider_id.to_string(),
            provider_name,
            generated_at,
            status: ProviderBalanceStatus::Ok,
            message: None,
            balances,
        }
    }
}
