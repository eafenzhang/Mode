//! BigModel Coding Plan 额度与订阅摘要投影。
//!
//! 真源：`packages/services/src/usage-stats/providers/bigmodelUsageQuotaMapper.ts`。
//! 日期解析使用调用方传入的固定本地偏移，不读取系统时区。

use regex::Regex;
use serde_json::{json, Number, Value};

const MS_PER_DAY: i64 = 86_400_000;

#[derive(Clone, Debug, PartialEq)]
pub struct UsageQuotaUsageDetail {
    pub model_code: String,
    pub display_name: Option<String>,
    pub usage: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageQuotaLimit {
    pub limit_type: String,
    pub unit: Option<f64>,
    pub number: Option<f64>,
    pub usage: Option<f64>,
    pub current_value: Option<f64>,
    pub remaining: Option<f64>,
    pub percentage: Option<f64>,
    pub next_reset_time: Option<f64>,
    pub usage_details: Vec<UsageQuotaUsageDetail>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BigModelSubscriptionListItem {
    pub product_id: Option<String>,
    pub product_name: Option<String>,
    pub status: Option<String>,
    pub in_current_period: Option<bool>,
    pub auto_renew: Option<Value>,
    pub next_renew_time: Option<String>,
    pub valid: Option<String>,
    pub billing_cycle: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentSubscriptionSummary {
    pub product_id: String,
    pub product_name: String,
    pub billing_cycle: Option<String>,
    pub renew_time: Option<String>,
    pub expire_time: Option<String>,
}

fn number_value(value: Option<&Value>) -> Option<f64> {
    value?.as_f64()
}

fn json_number(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        Value::Number(Number::from(value as i64))
    } else {
        Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

fn optional_number(value: Option<f64>) -> Option<Value> {
    value.map(json_number)
}

fn normalize_usage_details(details: Option<&Value>) -> Vec<UsageQuotaUsageDetail> {
    let Some(details) = details.and_then(Value::as_array) else {
        return Vec::new();
    };
    details
        .iter()
        .filter_map(|detail| {
            let detail = detail.as_object()?;
            let model_code = detail.get("modelCode")?.as_str()?.to_string();
            let display_name = detail
                .get("displayName")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let usage = number_value(detail.get("usage")).unwrap_or(0.0);
            Some(UsageQuotaUsageDetail {
                model_code,
                display_name,
                usage,
            })
        })
        .collect()
}

pub fn normalize_limits(limits: Option<&Value>) -> Vec<UsageQuotaLimit> {
    let Some(limits) = limits.and_then(Value::as_array) else {
        return Vec::new();
    };
    limits
        .iter()
        .filter_map(|limit| {
            let limit = limit.as_object()?;
            let limit_type = limit.get("type")?.as_str()?.to_string();
            if limit_type.is_empty() {
                return None;
            }
            Some(UsageQuotaLimit {
                limit_type,
                unit: number_value(limit.get("unit")),
                number: number_value(limit.get("number")),
                usage: number_value(limit.get("usage")),
                current_value: number_value(limit.get("currentValue")),
                remaining: number_value(limit.get("remaining")),
                percentage: number_value(limit.get("percentage")),
                next_reset_time: number_value(limit.get("nextResetTime")),
                usage_details: normalize_usage_details(limit.get("usageDetails")),
            })
        })
        .collect()
}

pub fn pick_primary_limit(limits: &[UsageQuotaLimit]) -> Option<&UsageQuotaLimit> {
    limits
        .iter()
        .find(|limit| limit.limit_type == "TIME_LIMIT")
        .or_else(|| limits.iter().find(|limit| limit.remaining.is_some()))
        .or_else(|| limits.first())
}

fn is_coding_plan_product(item: &BigModelSubscriptionListItem) -> bool {
    item.product_id
        .iter()
        .chain(item.product_name.iter())
        .any(|field| field.to_lowercase().contains("coding"))
}

fn is_active_personal_coding_plan(item: &BigModelSubscriptionListItem) -> bool {
    is_coding_plan_product(item)
        && item.status.as_deref() == Some("VALID")
        && item.in_current_period == Some(true)
}

pub fn pick_current_subscription_from_list(
    payload: Option<&Value>,
    offset_secs: i32,
) -> Option<CurrentSubscriptionSummary> {
    let payload = payload?.as_array()?;
    if payload.is_empty() {
        return None;
    }
    let mut current = None;
    for entry in payload {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let item = BigModelSubscriptionListItem {
            product_id: entry
                .get("productId")
                .and_then(Value::as_str)
                .map(str::to_string),
            product_name: entry
                .get("productName")
                .and_then(Value::as_str)
                .map(str::to_string),
            status: entry
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string),
            in_current_period: entry.get("inCurrentPeriod").and_then(Value::as_bool),
            auto_renew: entry.get("autoRenew").cloned(),
            next_renew_time: entry
                .get("nextRenewTime")
                .and_then(Value::as_str)
                .map(str::to_string),
            valid: entry
                .get("valid")
                .and_then(Value::as_str)
                .map(str::to_string),
            billing_cycle: entry
                .get("billingCycle")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        if is_active_personal_coding_plan(&item) {
            current = Some(item);
            break;
        }
    }
    let current = current?;
    let product_id = current.product_id.filter(|value| !value.is_empty())?;
    let product_name = current
        .product_name
        .clone()
        .unwrap_or_else(|| product_id.clone());
    let billing_cycle = current
        .billing_cycle
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let next_renew_time = current
        .next_renew_time
        .as_deref()
        .and_then(|value| parse_bigmodel_local_date_time(value, offset_secs));
    let auto_renew = current.auto_renew.as_ref().is_some_and(is_auto_renew);
    let renew_time = next_renew_time.clone().filter(|_| auto_renew);
    let expire_time = if next_renew_time.is_some() && !auto_renew {
        next_renew_time
    } else {
        current
            .valid
            .as_deref()
            .and_then(parse_valid_period_end)
            .and_then(|value| parse_bigmodel_local_date_time(&value, offset_secs))
    };
    Some(CurrentSubscriptionSummary {
        product_id,
        product_name,
        billing_cycle,
        renew_time,
        expire_time,
    })
}

fn is_auto_renew(value: &Value) -> bool {
    value.as_bool() == Some(true) || value.as_f64() == Some(1.0)
}

fn parse_valid_period_end(value: &str) -> Option<String> {
    let pattern = Regex::new(r"\d{4}-\d{2}-\d{2}(?:[ T]\d{2}:\d{2}:\d{2})?")
        .expect("valid period 正则可编译");
    pattern
        .find_iter(value)
        .last()
        .map(|match_| match_.as_str().to_string())
}

fn parse_bigmodel_local_date_time(value: &str, offset_secs: i32) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = if trimmed.contains('T') || !trimmed.contains(' ') {
        trimmed.to_string()
    } else {
        trimmed.replacen(' ', "T", 1)
    };
    let date_only = Regex::new(r"^(\d{4})-(\d{2})-(\d{2})$").expect("date-only 正则可编译");
    if let Some(captures) = date_only.captures(&normalized) {
        let year = captures[1].parse::<i64>().ok()?;
        let month = captures[2].parse::<u32>().ok()?;
        let day = captures[3].parse::<u32>().ok()?;
        if !(1..=12).contains(&month) {
            return None;
        }
        return Some(to_iso(days_from_civil(year, month, 1) + day as i64 - 1, 0));
    }
    let date_time = Regex::new(
        r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2})(?:\.(\d{1,3}))?)?(Z|z|[+-]\d{2}:\d{2})?$",
    )
    .expect("date-time 正则可编译");
    let captures = date_time.captures(&normalized)?;
    let year = captures[1].parse::<i64>().ok()?;
    let month = captures[2].parse::<u32>().ok()?;
    let day = captures[3].parse::<i64>().ok()?;
    let hour = captures[4].parse::<i64>().ok()?;
    let minute = captures[5].parse::<i64>().ok()?;
    let second = captures
        .get(6)
        .map_or(Some(0), |value| value.as_str().parse::<i64>().ok())?;
    if !(1..=12).contains(&month)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let millis = captures.get(7).map_or(0, |value| {
        let fraction = value.as_str();
        let padded = format!("{fraction:0<3}");
        padded.parse::<i64>().unwrap_or(0)
    });
    let zone_offset = match captures.get(8).map(|value| value.as_str()) {
        None => offset_secs as i64 * 1000,
        Some("Z" | "z") => 0,
        Some(zone) => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let hours = zone[1..3].parse::<i64>().ok()?;
            let minutes = zone[4..6].parse::<i64>().ok()?;
            sign * (hours * 3600 + minutes * 60) * 1000
        }
    };
    let local_ms = (days_from_civil(year, month, 1) + day - 1) * MS_PER_DAY
        + hour * 3_600_000
        + minute * 60_000
        + second * 1000
        + millis;
    let utc_ms = local_ms - zone_offset;
    Some(to_iso(
        utc_ms.div_euclid(MS_PER_DAY),
        utc_ms.rem_euclid(MS_PER_DAY),
    ))
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let month = month as i64;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}

fn to_iso(days: i64, ms_of_day: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    let second_of_day = ms_of_day / 1000;
    let millis = ms_of_day % 1000;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60
    )
}

impl UsageQuotaUsageDetail {
    fn to_json(&self) -> Value {
        let mut value = json!({ "modelCode": self.model_code, "usage": json_number(self.usage) });
        if let Some(display_name) = &self.display_name {
            value["displayName"] = json!(display_name);
        }
        value
    }
}

impl UsageQuotaLimit {
    pub fn to_json(&self) -> Value {
        let mut value = json!({ "type": self.limit_type });
        for (key, field) in [
            ("unit", self.unit),
            ("number", self.number),
            ("usage", self.usage),
            ("currentValue", self.current_value),
            ("remaining", self.remaining),
            ("percentage", self.percentage),
            ("nextResetTime", self.next_reset_time),
        ] {
            if let Some(number) = optional_number(field) {
                value[key] = number;
            }
        }
        value["usageDetails"] = Value::Array(
            self.usage_details
                .iter()
                .map(UsageQuotaUsageDetail::to_json)
                .collect(),
        );
        value
    }
}

impl CurrentSubscriptionSummary {
    pub fn to_json(&self) -> Value {
        json!({
            "productId": self.product_id,
            "productName": self.product_name,
            "billingCycle": self.billing_cycle,
            "renewTime": self.renew_time,
            "expireTime": self.expire_time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_date_time_with_injected_offset() {
        assert_eq!(
            parse_bigmodel_local_date_time("2026-01-02 03:04:05", 8 * 3600).as_deref(),
            Some("2026-01-01T19:04:05.000Z")
        );
        assert_eq!(
            parse_bigmodel_local_date_time("2026-01-02T03:04:05Z", 8 * 3600).as_deref(),
            Some("2026-01-02T03:04:05.000Z")
        );
    }

    #[test]
    fn picks_time_limit_first() {
        let limits = vec![
            UsageQuotaLimit {
                limit_type: "TOKENS_LIMIT".to_string(),
                unit: None,
                number: None,
                usage: None,
                current_value: None,
                remaining: Some(1.0),
                percentage: None,
                next_reset_time: None,
                usage_details: Vec::new(),
            },
            UsageQuotaLimit {
                limit_type: "TIME_LIMIT".to_string(),
                unit: None,
                number: None,
                usage: None,
                current_value: None,
                remaining: None,
                percentage: None,
                next_reset_time: None,
                usage_details: Vec::new(),
            },
        ];
        assert_eq!(
            pick_primary_limit(&limits).map(|limit| limit.limit_type.as_str()),
            Some("TIME_LIMIT")
        );
    }
}
