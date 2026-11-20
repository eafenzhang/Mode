//! `usage-stats` 频道适配器（LT-15 P4 服务面接线）。
//!
//! 真源：`packages/services/src/usage-stats/usageStats.ts` 的
//! `IUsageStatsService`（`ServiceChannels.UsageStats = "usage-stats"`）。
//! 本切片覆盖 `getProviderBalanceSnapshot`：外部 api-key Provider 的账户余额
//! （凭据链路 = Personal 层 target resolver + reqwest fetcher，见
//! `provider-balance` crate）。官方套餐/额度（codingPlan/account）链路已随
//! 去智谱化整体下线（TS `assertOfficialServiceRemoved` 恒抛），不接线。

use std::sync::Arc;

use provider_balance::{
    PersonalLayerTargetResolver, ProviderBalanceProvider, ProviderBalanceTargetResolver,
    ReqwestBalanceFetcher,
};
use provider_config_repository::PersonalConfigRepository;
use provider_config_source::PersonalLayer;
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Deserialize;
use serde_json::json;

use crate::params::first_arg_object;

pub const USAGE_STATS_CHANNEL: &str = "usage-stats";
pub const GET_PROVIDER_BALANCE_SNAPSHOT: &str = "getProviderBalanceSnapshot";

pub struct UsageStatsChannel {
    provider_balance: Arc<ProviderBalanceProvider>,
}

impl UsageStatsChannel {
    pub fn new(provider_balance: Arc<ProviderBalanceProvider>) -> Self {
        Self { provider_balance }
    }

    /// TS 缺省装配（无 `resolveProviderBalanceTarget`）：一律 `unsupported`，
    /// 保证只装配了官方能力的 Environment 不产生意外网络请求。
    pub fn default_channel() -> Self {
        Self::new(default_provider_balance())
    }

    /// 标准装配：Personal 层读层（1s 轮询 LKG）+ reqwest fetcher。
    ///
    /// repository 构造失败不影响频道可用性——返回 `default_channel()` 语义
    /// （unsupported）由调用方兜底。
    pub fn with_personal_layer(repository: Arc<PersonalConfigRepository>) -> Result<Self, String> {
        let resolver = PersonalLayerTargetResolver::new(move || repository.read());
        let fetcher = Arc::new(ReqwestBalanceFetcher::new()?);
        Ok(Self::new(Arc::new(ProviderBalanceProvider::new(
            Some(Arc::new(resolver) as Arc<dyn ProviderBalanceTargetResolver>),
            fetcher,
            Arc::new(now_ms),
        ))))
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// TS 缺省装配等价：无 resolver（一律 unsupported），仍装 reqwest fetcher。
pub fn default_provider_balance() -> Arc<ProviderBalanceProvider> {
    let fetcher = ReqwestBalanceFetcher::new().ok().map(Arc::new);
    match fetcher {
        Some(fetcher) => Arc::new(ProviderBalanceProvider::new(
            None,
            fetcher,
            Arc::new(now_ms),
        )),
        None => {
            // reqwest client 构造失败（极罕见）：退化为纯 unsupported 面板。
            struct NeverFetcher;
            impl provider_balance::ProviderBalanceFetcher for NeverFetcher {
                fn fetch_balance(
                    &self,
                    _url: &str,
                    _api_key: &str,
                ) -> provider_balance::BalanceFuture<'_, Result<serde_json::Value, provider_balance::FetchError>>
                {
                    Box::pin(std::future::ready(Err(
                        provider_balance::FetchError::Network,
                    )))
                }
            }
            Arc::new(ProviderBalanceProvider::new(
                None,
                Arc::new(NeverFetcher),
                Arc::new(now_ms),
            ))
        }
    }
}

/// TS `ProviderBalanceRequest`：`{ providerId?: string }`。
/// 缺省/undefined 与空串同语义（进 `get_snapshot` 后按 unsupported 快照返回，
/// 与 TS `request.providerId?.trim() ?? ""` 一致，不作为参数错误）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderBalanceArgs {
    #[serde(default)]
    provider_id: Option<String>,
}

fn run(
    provider_balance: Arc<ProviderBalanceProvider>,
    command: &str,
    arg: &Value,
) -> Result<BoxFuture<Result<Value, ChannelError>>, ChannelError> {
    match command {
        GET_PROVIDER_BALANCE_SNAPSHOT => {
            let params = first_arg_object(arg)?;
            let args: ProviderBalanceArgs =
                serde_json::from_value(serde_json::Value::Object(params))
                    .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))?;
            let provider = provider_balance;
            Ok(Box::pin(async move {
                let snapshot = provider
                    .get_snapshot(args.provider_id.as_deref().unwrap_or_default())
                    .await;
                Ok(json_value(&serde_json::to_value(&snapshot).map_err(
                    |error| ChannelError::error(error.to_string()),
                )?))
            }))
        }
        other => Err(ChannelError::error(format!(
            "usage-stats channel: unknown command {other}"
        ))),
    }
}

impl ServerChannel for UsageStatsChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        // run 只做同步解析并构造 future（内部已 Arc::clone），future 是 'static 的。
        match run(Arc::clone(&self.provider_balance), &command, &arg) {
            Ok(future) => future,
            Err(error) => Box::pin(std::future::ready(Err(error))),
        }
    }
}

fn json_value(value: &serde_json::Value) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}

/// 壳侧装配 helper：给定 personal 层读层闭包（`repository.read()`）构造频道。
/// 供 inprocess 装配以外需要自定义读取来源的场景（测试/其他宿主）复用。
pub fn channel_with_layer<F>(read_layer: F) -> Result<UsageStatsChannel, String>
where
    F: Fn() -> PersonalLayer + Send + Sync + 'static,
{
    let resolver = PersonalLayerTargetResolver::new(read_layer);
    let fetcher = Arc::new(ReqwestBalanceFetcher::new()?);
    Ok(UsageStatsChannel::new(Arc::new(
        ProviderBalanceProvider::new(
            Some(Arc::new(resolver) as Arc<dyn ProviderBalanceTargetResolver>),
            fetcher,
            Arc::new(now_ms),
        ),
    )))
}

/// 参数缺省时 TS 语义的哨兵常量（`{}` → providerId undefined）。
pub fn empty_args() -> Value {
    Value::Json(json!([{}]).to_string())
}
