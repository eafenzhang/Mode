//! 外部 API Key Provider 余额识别与响应解析。
//!
//! 真源：`packages/services/src/usage-stats/providers/providerBalanceSpecs.ts`。
//! 纯函数：只按 `baseUrl` 主机名识别供应商，解析固定余额接口的响应 JSON；
//! 不发起网络请求。规范见 `docs/specs/provider-balance.md`。

mod service;

pub use service::{
    provider_balance_target_from_view, BalanceFuture, FetchError, ProviderBalanceFetcher,
    ProviderBalanceProvider, ProviderBalanceSnapshot, ProviderBalanceStatus, ProviderBalanceTarget,
    ProviderBalanceTargetResolver,
};

use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::{Number, Value};
use url::Url;

/// 供应商固定余额查询地址与解析规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBalanceKind {
    DeepSeek,
    Moonshot,
    SiliconFlow,
    StepFun,
    OpenRouter,
    Novita,
}

impl ProviderBalanceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeepSeek => "deepseek",
            Self::Moonshot => "moonshot",
            Self::SiliconFlow => "siliconflow",
            Self::StepFun => "stepfun",
            Self::OpenRouter => "openrouter",
            Self::Novita => "novita",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProviderBalance {
    pub kind: ProviderBalanceKind,
    pub url: String,
    /// 识别时使用的小写 hostname；Moonshot/SiliconFlow 的币种由它决定。
    pub host: String,
}

/// `ProviderBalanceEntry` 投影：`total`/`used` 需要区分「缺省」与「显式 null」。
#[derive(Debug, Clone, PartialEq)]
pub enum BalanceAmount {
    Absent,
    Null,
    Value(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderBalanceEntry {
    pub label: String,
    pub unit: Option<String>,
    pub remaining: Option<f64>,
    pub total: BalanceAmount,
    pub used: BalanceAmount,
    pub is_available: bool,
}

fn json_number(value: f64) -> Number {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        Number::from(value as i64)
    } else {
        Number::from_f64(value).unwrap_or_else(|| Number::from(0))
    }
}

impl Serialize for ProviderBalanceEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("label", &self.label)?;
        map.serialize_entry(
            "unit",
            &self
                .unit
                .as_ref()
                .map(|unit| Value::String(unit.clone()))
                .unwrap_or(Value::Null),
        )?;
        map.serialize_entry(
            "remaining",
            &self
                .remaining
                .map(|value| Value::Number(json_number(value)))
                .unwrap_or(Value::Null),
        )?;
        for (key, amount) in [("total", &self.total), ("used", &self.used)] {
            match amount {
                BalanceAmount::Absent => {}
                BalanceAmount::Null => map.serialize_entry(key, &Value::Null)?,
                BalanceAmount::Value(value) => {
                    map.serialize_entry(key, &Value::Number(json_number(*value)))?
                }
            }
        }
        map.serialize_entry("isAvailable", &self.is_available)?;
        map.end()
    }
}

fn as_record(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value.as_object()
}

fn nested_record<'a>(
    record: Option<&'a serde_json::Map<String, Value>>,
    key: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    record?.get(key).and_then(as_record)
}

fn js_number(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("0x") {
        return u64::from_str_radix(rest, 16).ok().map(|value| value as f64);
    }
    if let Some(rest) = lower.strip_prefix("0b") {
        return u64::from_str_radix(rest, 2).ok().map(|value| value as f64);
    }
    if let Some(rest) = lower.strip_prefix("0o") {
        return u64::from_str_radix(rest, 8).ok().map(|value| value as f64);
    }
    trimmed
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn read_number(record: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<f64> {
    let value = record?.get(key)?;
    match value {
        Value::Number(number) => number.as_f64().filter(|value| value.is_finite()),
        Value::String(text) if !text.trim().is_empty() => js_number(text),
        _ => None,
    }
}

fn read_boolean(record: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<bool> {
    record?.get(key)?.as_bool()
}

fn read_string(record: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<String> {
    let text = record?.get(key)?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn amount(value: Option<f64>) -> BalanceAmount {
    match value {
        None => BalanceAmount::Null,
        Some(value) => BalanceAmount::Value(value),
    }
}

fn entry(
    label: &str,
    unit: Option<&str>,
    remaining: Option<f64>,
    total: BalanceAmount,
    used: BalanceAmount,
    is_available: Option<bool>,
) -> ProviderBalanceEntry {
    ProviderBalanceEntry {
        label: label.to_string(),
        unit: unit.map(str::to_string),
        remaining,
        total,
        used,
        is_available: is_available.unwrap_or(true),
    }
}

fn available_from_remaining(remaining: Option<f64>) -> bool {
    remaining.is_none_or(|remaining| remaining > 0.0)
}

fn parse_deep_seek(payload: &Value) -> Vec<ProviderBalanceEntry> {
    let Some(root) = as_record(payload) else {
        return Vec::new();
    };
    let is_available = read_boolean(Some(root), "is_available");
    let infos = root
        .get("balance_infos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut balances = Vec::new();
    for info in infos {
        let Some(record) = as_record(&info) else {
            continue;
        };
        let currency = read_string(Some(record), "currency").unwrap_or_else(|| "CNY".to_string());
        balances.push(entry(
            &currency,
            Some(&currency),
            read_number(Some(record), "total_balance"),
            BalanceAmount::Absent,
            BalanceAmount::Absent,
            is_available,
        ));
    }
    balances
}

fn parse_moonshot(payload: &Value, host: &str) -> Vec<ProviderBalanceEntry> {
    let root = as_record(payload);
    let data = nested_record(root, "data").or(root);
    let Some(data) = data else {
        return Vec::new();
    };
    let unit = if host.contains("moonshot.ai") {
        "USD"
    } else {
        "CNY"
    };
    let remaining = read_number(Some(data), "available_balance");
    vec![entry(
        unit,
        Some(unit),
        remaining,
        amount(read_number(Some(data), "voucher_balance")),
        BalanceAmount::Absent,
        Some(available_from_remaining(remaining)),
    )]
}

fn parse_silicon_flow(payload: &Value, host: &str) -> Vec<ProviderBalanceEntry> {
    let root = as_record(payload);
    let data = nested_record(root, "data").or(root);
    let Some(data) = data else {
        return Vec::new();
    };
    let unit = if host.contains("siliconflow.com") {
        "USD"
    } else {
        "CNY"
    };
    let remaining =
        read_number(Some(data), "totalBalance").or_else(|| read_number(Some(data), "balance"));
    vec![entry(
        unit,
        Some(unit),
        remaining,
        amount(read_number(Some(data), "chargeBalance")),
        BalanceAmount::Absent,
        Some(available_from_remaining(remaining)),
    )]
}

fn parse_step_fun(payload: &Value) -> Vec<ProviderBalanceEntry> {
    let Some(root) = as_record(payload) else {
        return Vec::new();
    };
    let remaining = read_number(Some(root), "balance");
    vec![entry(
        "CNY",
        Some("CNY"),
        remaining,
        amount(read_number(Some(root), "total_cash_balance")),
        BalanceAmount::Absent,
        Some(available_from_remaining(remaining)),
    )]
}

fn parse_open_router(payload: &Value) -> Vec<ProviderBalanceEntry> {
    let root = as_record(payload);
    let data = nested_record(root, "data").or(root);
    let Some(data) = data else {
        return Vec::new();
    };
    let total = read_number(Some(data), "total_credits");
    let used = read_number(Some(data), "total_usage");
    let remaining = total.map(|total| total - used.unwrap_or(0.0));
    vec![entry(
        "USD",
        Some("USD"),
        remaining,
        amount(total),
        amount(used),
        Some(available_from_remaining(remaining)),
    )]
}

fn parse_novita(payload: &Value) -> Vec<ProviderBalanceEntry> {
    let Some(root) = as_record(payload) else {
        return Vec::new();
    };
    let raw = read_number(Some(root), "availableBalance");
    let remaining = raw.map(|value| value / 10_000.0);
    let raw_total = read_number(Some(root), "cashBalance");
    vec![entry(
        "USD",
        Some("USD"),
        remaining,
        amount(raw_total.map(|value| value / 10_000.0)),
        BalanceAmount::Absent,
        Some(available_from_remaining(remaining)),
    )]
}

fn hostname_of(base_url: Option<&str>) -> Option<String> {
    let value = base_url?.trim();
    if value.is_empty() {
        return None;
    }
    Url::parse(value).ok()?.host_str().map(str::to_lowercase)
}

fn is_host(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// `resolveProviderBalanceProvider`。
pub fn resolve_provider_balance(base_url: Option<&str>) -> Option<ResolvedProviderBalance> {
    let host = hostname_of(base_url)?;
    let resolved = |kind, url: String| ResolvedProviderBalance {
        kind,
        url,
        host: host.clone(),
    };
    if is_host(&host, "api.deepseek.com") {
        return Some(resolved(
            ProviderBalanceKind::DeepSeek,
            "https://api.deepseek.com/user/balance".to_string(),
        ));
    }
    if is_host(&host, "api.moonshot.cn") || is_host(&host, "api.moonshot.ai") {
        return Some(resolved(
            ProviderBalanceKind::Moonshot,
            format!("https://{host}/v1/users/me/balance"),
        ));
    }
    if is_host(&host, "api.siliconflow.cn") || is_host(&host, "api.siliconflow.com") {
        return Some(resolved(
            ProviderBalanceKind::SiliconFlow,
            format!("https://{host}/v1/user/info"),
        ));
    }
    if is_host(&host, "api.stepfun.com") || is_host(&host, "api.stepfun.ai") {
        return Some(resolved(
            ProviderBalanceKind::StepFun,
            format!("https://{host}/v1/accounts"),
        ));
    }
    if is_host(&host, "openrouter.ai") {
        return Some(resolved(
            ProviderBalanceKind::OpenRouter,
            "https://openrouter.ai/api/v1/credits".to_string(),
        ));
    }
    if is_host(&host, "api.novita.ai") {
        return Some(resolved(
            ProviderBalanceKind::Novita,
            "https://api.novita.ai/v3/user/balance".to_string(),
        ));
    }
    None
}

/// 按已识别供应商解析响应；未识别时返回空表。
pub fn parse_provider_balance(
    resolved: &ResolvedProviderBalance,
    payload: &Value,
) -> Vec<ProviderBalanceEntry> {
    match resolved.kind {
        ProviderBalanceKind::DeepSeek => parse_deep_seek(payload),
        ProviderBalanceKind::Moonshot => parse_moonshot(payload, &resolved.host),
        ProviderBalanceKind::SiliconFlow => parse_silicon_flow(payload, &resolved.host),
        ProviderBalanceKind::StepFun => parse_step_fun(payload),
        ProviderBalanceKind::OpenRouter => parse_open_router(payload),
        ProviderBalanceKind::Novita => parse_novita(payload),
    }
}
