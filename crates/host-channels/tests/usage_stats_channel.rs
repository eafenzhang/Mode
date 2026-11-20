//! `usage-stats` 频道行为契约（LT-15 P4）。
//!
//! 链路：`PersonalConfigRepository`（临时 personal config 文件）→
//! `PersonalLayerTargetResolver` → stub fetcher → 快照 JSON。真实 HTTP 取回在
//! `provider-balance` 的 mock server 用例覆盖，这里用 stub 隔离网络。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use provider_balance::{
    BalanceFuture, FetchError, ProviderBalanceFetcher, ProviderBalanceProvider,
    ProviderBalanceTargetResolver,
};
use provider_config::Field;
use provider_config::{
    ApiKeyAccess, ApiKeyAccessType, ProviderAccess, ProviderApiConfig, ProviderConfig,
    ProviderConfigRule,
};
use provider_config_file_codec::ProviderConfigLayerUpdate;
use provider_config_repository::{
    PersonalConfigRepository, PersonalConfigRepositoryOptions, PollingInterval,
};
use provider_config_source::empty_personal_layer;
use rpc_host::CancellationToken;
use rpc_wire::Value;
use serde_json::{json, Value as Json};

use rpc_host::{ChannelError, ServerChannel as _};
use host_channels::usage_stats::{
    default_provider_balance, GET_PROVIDER_BALANCE_SNAPSHOT, USAGE_STATS_CHANNEL,
    UsageStatsChannel,
};

struct TempHome(PathBuf);

impl TempHome {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "usage-stats-channel-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempHome(dir)
    }

    fn config_path(&self) -> PathBuf {
        self.0.join("provider_config.json")
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 固定应答的 stub fetcher：返回 deepseek 形状的余额 JSON。
struct StubFetcher {
    payload: Json,
}

impl ProviderBalanceFetcher for StubFetcher {
    fn fetch_balance(
        &self,
        _url: &str,
        _api_key: &str,
    ) -> BalanceFuture<'_, Result<Json, FetchError>> {
        let payload = self.payload.clone();
        Box::pin(std::future::ready(Ok(payload)))
    }
}

fn api_key_rule(provider_id: &str, api_key: &str, base_url: &str) -> ProviderConfigRule {
    ProviderConfigRule {
        provider_id: provider_id.to_string(),
        template_id: Field::Absent,
        provider_name: Field::Value("DeepSeek 官方".to_string()),
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

fn repository(path: &Path) -> PersonalConfigRepository {
    PersonalConfigRepository::new(PersonalConfigRepositoryOptions {
        file_path: path.to_path_buf(),
        import_legacy: None,
        on_recovery: None,
        on_polling_error: None,
        polling_interval: PollingInterval::Millis(1_000),
    })
    .expect("repository")
}

fn write_rule(home: &TempHome, rule: ProviderConfigRule) -> Arc<PersonalConfigRepository> {
    let repo = repository(&home.config_path());
    let mut providers = empty_personal_layer().providers;
    providers.values.insert(rule.provider_id.clone(), rule);
    let mut layer = empty_personal_layer();
    layer.providers = providers;
    repo.update(|_| ProviderConfigLayerUpdate {
        providers: layer.providers,
        models: layer.models,
        provider_order: layer.provider_order,
        default_model_selection: None,
    })
    .expect("write personal layer");
    Arc::new(repo)
}

fn channel_with_stub(home: &TempHome, rule: ProviderConfigRule) -> UsageStatsChannel {
    let repository = write_rule(home, rule);
    let resolver = provider_balance::PersonalLayerTargetResolver::new(move || repository.read());
    UsageStatsChannel::new(Arc::new(ProviderBalanceProvider::new(
        Some(Arc::new(resolver) as Arc<dyn ProviderBalanceTargetResolver>),
        Arc::new(StubFetcher {
            payload: json!({
                "is_available": true,
                "balance_infos": [
                    { "currency": "CNY", "total_balance": "42.50" },
                ],
            }),
        }),
        Arc::new(|| 1_700_000_000_000),
    )))
}

async fn call(channel: &UsageStatsChannel, arg: Json) -> Json {
    let value = channel
        .call(
            "ctx".into(),
            GET_PROVIDER_BALANCE_SNAPSHOT.into(),
            Value::Json(arg.to_string()),
            CancellationToken::default(),
        )
        .await
        .expect("call");
    match value {
        Value::Json(text) => serde_json::from_str(&text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn api_key_provider_returns_ok_snapshot() {
    let home = TempHome::new("ok");
    let channel = channel_with_stub(
        &home,
        api_key_rule("deepseek", "sk-abc", "https://api.deepseek.com/v1"),
    );
    let snapshot = call(&channel, json!({ "providerId": "deepseek" })).await;
    assert_eq!(snapshot["providerId"], "deepseek");
    assert_eq!(snapshot["providerName"], "DeepSeek 官方");
    assert_eq!(snapshot["status"], "ok");
    assert_eq!(snapshot["balances"][0]["label"], "CNY");
    assert_eq!(snapshot["balances"][0]["remaining"], 42.5);
}

#[tokio::test]
async fn missing_provider_id_reports_unsupported() {
    let home = TempHome::new("unsupported");
    let channel = channel_with_stub(
        &home,
        api_key_rule("deepseek", "sk-abc", "https://api.deepseek.com/v1"),
    );
    // 层里没有 official provider → resolver 返回 None → unsupported。
    let snapshot = call(&channel, json!({ "providerId": "official" })).await;
    assert_eq!(snapshot["status"], "unsupported");
    assert_eq!(snapshot["balances"], json!([]));
    // providerId 缺省（undefined）→ 与 TS `?? ""` 同语义：unsupported 快照，非参数错误。
    let snapshot = call(&channel, json!({})).await;
    assert_eq!(snapshot["status"], "unsupported");
}

#[tokio::test]
async fn empty_api_key_reports_not_configured() {
    let home = TempHome::new("not-configured");
    let channel = channel_with_stub(
        &home,
        api_key_rule("deepseek", "   ", "https://api.deepseek.com/v1"),
    );
    let snapshot = call(&channel, json!({ "providerId": "deepseek" })).await;
    assert_eq!(snapshot["status"], "not_configured");
    assert_eq!(snapshot["balances"], json!([]));
}

#[tokio::test]
async fn unrecognized_base_url_reports_unsupported() {
    let home = TempHome::new("unknown-host");
    let channel = channel_with_stub(
        &home,
        api_key_rule("mystery", "sk-1", "https://api.mystery.example/v1"),
    );
    let snapshot = call(&channel, json!({ "providerId": "mystery" })).await;
    assert_eq!(snapshot["status"], "unsupported");
}

#[tokio::test]
async fn default_channel_has_no_resolver_and_reports_unsupported() {
    let channel = UsageStatsChannel::default_channel();
    let snapshot = call(&channel, json!({ "providerId": "deepseek" })).await;
    assert_eq!(snapshot["status"], "unsupported");
    let snapshot = call(
        &channel,
        json!({ "providerId": "deepseek", "extra": true }),
    )
    .await;
    assert_eq!(snapshot["status"], "unsupported");
}

#[tokio::test]
async fn unknown_command_errors() {
    let channel = UsageStatsChannel::default_channel();
    let error = channel
        .call(
            "ctx".into(),
            "getAppUsageSnapshot".into(),
            Value::Json("{}".to_string()),
            CancellationToken::default(),
        )
        .await
        .expect_err("unknown command");
    match error {
        ChannelError::Error { message, .. } => {
            assert!(message.contains("unknown command"));
        }
        other => panic!("期望 Error 变体，实际 {other:?}"),
    }
}

#[tokio::test]
async fn channel_name_and_default_provider_balance_assembly() {
    // 频道名与 TS ServiceChannels 对齐；default 装配（无 repository）可用。
    assert_eq!(USAGE_STATS_CHANNEL, "usage-stats");
    let channel = UsageStatsChannel::new(default_provider_balance());
    let snapshot = call(&channel, json!({ "providerId": "x" })).await;
    assert_eq!(snapshot["status"], "unsupported");
}
