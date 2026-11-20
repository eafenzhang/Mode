//! 从 PersonalLayer 解析余额查询目标（LT-15 P4 凭据装配）。
//!
//! 真源：`packages/services/src/usage-stats/providers/providerBalanceTargetResolver.ts`
//! （只有 `access.type === "api-key"` 的 Provider 参与余额查询；官方账号
//! `zhipu-account` 与 Coding Plan Key 属官方链路，不在这里处理）。
//! TS 的 `effectiveConfig` 是 builtin 模板 + personal 覆盖的合成；外部 api-key
//! Provider 由用户自配、落在 personal 层，这里从 `PersonalLayer` 直读
//! （builtin 模板合并待 ProviderSettingsView 移植后补）。

use std::future;
use std::sync::Arc;

use provider_config::ProviderAccess;
use provider_config_source::PersonalLayer;

use crate::service::{
    BalanceFuture, ProviderBalanceTarget, ProviderBalanceTargetResolver,
};

/// 从 personal 层解析单个 Provider 的余额查询目标。
///
/// 非 api-key access（官方账号）、未配置的 providerId 返回 `None`；
/// `providerName` trim 后为空回退 `providerId`（与 TS target resolver 同口径）。
pub fn provider_balance_target_from_personal_layer(
    layer: &PersonalLayer,
    provider_id: &str,
) -> Option<ProviderBalanceTarget> {
    let rule = layer.providers.get(provider_id)?;
    let Some(ProviderAccess::ApiKey(access)) = rule.config.access.value() else {
        return None;
    };
    let provider_name = rule
        .provider_name
        .value()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| provider_id.to_string());
    let base_url = rule
        .config
        .api
        .value()
        .and_then(|api| api.base_url.value())
        .cloned();
    let api_key = access.api_key.value().cloned();
    Some(ProviderBalanceTarget {
        provider_id: provider_id.to_string(),
        provider_name: Some(provider_name),
        base_url,
        api_key,
    })
}

/// 把「读当前 PersonalLayer」的闭包包装成 `ProviderBalanceTargetResolver`。
///
/// 装配示例：`PersonalLayerTargetResolver::new(move || repository.read())`。
pub struct PersonalLayerTargetResolver<F>
where
    F: Fn() -> PersonalLayer + Send + Sync,
{
    read_layer: Arc<F>,
}

impl<F> PersonalLayerTargetResolver<F>
where
    F: Fn() -> PersonalLayer + Send + Sync,
{
    pub fn new(read_layer: F) -> Self {
        Self {
            read_layer: Arc::new(read_layer),
        }
    }
}

impl<F> ProviderBalanceTargetResolver for PersonalLayerTargetResolver<F>
where
    F: Fn() -> PersonalLayer + Send + Sync + 'static,
{
    fn resolve_target(
        &self,
        provider_id: &str,
    ) -> BalanceFuture<'_, Result<Option<ProviderBalanceTarget>, String>> {
        let layer = (self.read_layer)();
        let target = provider_balance_target_from_personal_layer(&layer, provider_id);
        Box::pin(future::ready(Ok(target)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_config::{
        ApiKeyAccess, ApiKeyAccessType, Field, ProviderApiConfig, ProviderConfig,
        ProviderConfigRule,
    };
    use provider_config_source::empty_personal_layer;

    fn personal_layer_with(rules: Vec<ProviderConfigRule>) -> PersonalLayer {
        let mut layer = empty_personal_layer();
        for rule in rules {
            layer.providers.values.insert(rule.provider_id.clone(), rule);
        }
        layer
    }

    fn api_key_rule(provider_id: &str, name: &str, api_key: &str, base_url: &str) -> ProviderConfigRule {
        ProviderConfigRule {
            provider_id: provider_id.to_string(),
            template_id: Field::Absent,
            provider_name: Field::Value(name.to_string()),
            enabled: Field::Value(true),
            config: ProviderConfig {
                access: Field::Value(ProviderAccess::ApiKey(ApiKeyAccess {
                    access_type: ApiKeyAccessType::ApiKey,
                    api_key: Field::Value(api_key.to_string()),
                    api_key_management_url: Field::Absent,
                })),
                api: Field::Value(ProviderApiConfig {
                    api_type: Field::Absent,
                    base_url: Field::Value(base_url.to_string()),
                    headers: Field::Absent,
                }),
                ..ProviderConfig::default()
            },
        }
    }

    #[test]
    fn api_key_provider_resolves_target() {
        let layer = personal_layer_with(vec![api_key_rule(
            "deepseek",
            "DeepSeek 官方",
            "sk-abc",
            "https://api.deepseek.com/v1",
        )]);
        let target = provider_balance_target_from_personal_layer(&layer, "deepseek")
            .expect("target");
        assert_eq!(target.provider_id, "deepseek");
        assert_eq!(target.provider_name.as_deref(), Some("DeepSeek 官方"));
        assert_eq!(target.api_key.as_deref(), Some("sk-abc"));
        assert_eq!(
            target.base_url.as_deref(),
            Some("https://api.deepseek.com/v1")
        );
    }

    #[test]
    fn missing_provider_returns_none() {
        let layer = personal_layer_with(Vec::new());
        assert!(provider_balance_target_from_personal_layer(&layer, "absent").is_none());
    }

    #[test]
    fn blank_name_falls_back_to_provider_id() {
        let mut rule = api_key_rule("demo", "", "sk-1", "https://x.example");
        rule.provider_name = Field::Value("   ".to_string());
        let layer = personal_layer_with(vec![rule]);
        let target = provider_balance_target_from_personal_layer(&layer, "demo")
            .expect("target");
        assert_eq!(target.provider_name.as_deref(), Some("demo"));
    }

    #[test]
    fn resolver_adapter_wraps_closure() {
        let layer = personal_layer_with(vec![api_key_rule(
            "demo",
            "Demo",
            "sk-2",
            "https://api.demo.example",
        )]);
        let resolver = PersonalLayerTargetResolver::new(move || layer.clone());
        let target = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(resolver.resolve_target("demo"))
            .expect("resolve")
            .expect("target");
        assert_eq!(target.api_key.as_deref(), Some("sk-2"));
        // 官方账号链路：个人层未配置 → None（不产生意外网络请求）。
        let missing = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(resolver.resolve_target("official"))
            .expect("resolve");
        assert!(missing.is_none());
    }
}
