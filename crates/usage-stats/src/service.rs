//! usageStatsService 编排：Coding Plan 门控与依赖委派。
//!
//! 真源：`packages/services/src/usage-stats/usageStatsService.ts`。
//! 请求/响应暂以 `serde_json::Value` 承接（共享协议类型尚未 Rust 化）；Provider 实现由宿主注入。

use std::sync::Arc;

use serde_json::{json, Value};

pub const NO_BIGMODEL_API_KEY: &str = "no_bigmodel_api_key";

const CODING_PLAN_PROVIDER_IDS: [&str; 6] = [
    "account:zai-individual-coding-plan",
    "account:zai-team-coding-plan",
    "account:zai-start-plan",
    "account:bigmodel-individual-coding-plan",
    "account:bigmodel-team-coding-plan",
    "account:bigmodel-start-plan",
];

pub trait AppUsageStatsSource: Send + Sync {
    fn get_app_usage_stats(&self, request: &Value) -> Result<Value, String>;
}

pub trait CodingPlanQuotaSource: Send + Sync {
    fn get_coding_plan_usage_snapshot(&self, request: &Value) -> Result<Value, String>;
    fn get_coding_plan_reset_status(&self, request: &Value) -> Result<Value, String>;
    fn request_coding_plan_reset_opportunity(&self, request: &Value) -> Result<Value, String>;
    fn use_coding_plan_reset(&self, request: &Value) -> Result<Value, String>;
    fn mark_coding_plan_reset_history_read(&self, request: &Value) -> Result<(), String>;
    fn get_usage_stats_snapshot(&self, request: &Value) -> Result<Value, String>;
    fn get_entitlement_snapshot(&self, request: &Value) -> Result<Value, String>;
}

pub trait ProviderBalanceSource: Send + Sync {
    fn get_provider_balance_snapshot(&self, request: &Value) -> Result<Value, String>;
}

#[derive(Clone)]
pub struct UsageStatsService {
    app_usage: Arc<dyn AppUsageStatsSource>,
    coding_plan_quota: Arc<dyn CodingPlanQuotaSource>,
    provider_balance: Arc<dyn ProviderBalanceSource>,
}

impl UsageStatsService {
    pub fn new(
        app_usage: Arc<dyn AppUsageStatsSource>,
        coding_plan_quota: Arc<dyn CodingPlanQuotaSource>,
        provider_balance: Arc<dyn ProviderBalanceSource>,
    ) -> Self {
        Self {
            app_usage,
            coding_plan_quota,
            provider_balance,
        }
    }

    pub fn get_app_usage_snapshot(&self, request: &Value) -> Result<Value, String> {
        self.app_usage.get_app_usage_stats(request)
    }

    pub fn get_coding_plan_usage_snapshot(&self, request: &Value) -> Result<Value, String> {
        self.require_coding_plan(request)?;
        self.coding_plan_quota
            .get_coding_plan_usage_snapshot(request)
    }

    pub fn get_coding_plan_reset_status(&self, request: &Value) -> Result<Value, String> {
        self.require_coding_plan(request)?;
        self.coding_plan_quota.get_coding_plan_reset_status(request)
    }

    pub fn request_coding_plan_reset_opportunity(&self, request: &Value) -> Result<Value, String> {
        self.require_coding_plan(request)?;
        self.coding_plan_quota
            .request_coding_plan_reset_opportunity(request)
    }

    pub fn use_coding_plan_reset(&self, request: &Value) -> Result<Value, String> {
        self.require_coding_plan(request)?;
        self.coding_plan_quota.use_coding_plan_reset(request)
    }

    pub fn mark_coding_plan_reset_history_read(&self, request: &Value) -> Result<(), String> {
        self.require_coding_plan(request)?;
        self.coding_plan_quota
            .mark_coding_plan_reset_history_read(request)
    }

    pub fn get_snapshot(&self, request: &Value) -> Result<Value, String> {
        self.coding_plan_quota.get_usage_stats_snapshot(request)
    }

    /// 缺省请求按 `{}` 处理（TS `request: UsageEntitlementRequest = {}`）。
    pub fn get_entitlement_snapshot(&self, request: Option<&Value>) -> Result<Value, String> {
        let empty = json!({});
        let request = match request {
            None | Some(Value::Null) => &empty,
            Some(value) => value,
        };
        self.coding_plan_quota.get_entitlement_snapshot(request)
    }

    pub fn get_provider_balance_snapshot(&self, request: &Value) -> Result<Value, String> {
        self.provider_balance.get_provider_balance_snapshot(request)
    }

    fn require_coding_plan(&self, request: &Value) -> Result<(), String> {
        let preferred = request
            .as_object()
            .and_then(|object| object.get("preferredProviderId"))
            .and_then(Value::as_str);
        if preferred.map(is_coding_plan_model_provider_id) == Some(true) {
            Ok(())
        } else {
            Err(NO_BIGMODEL_API_KEY.to_string())
        }
    }
}

pub fn is_coding_plan_model_provider_id(id: &str) -> bool {
    CODING_PLAN_PROVIDER_IDS.contains(&id)
}
