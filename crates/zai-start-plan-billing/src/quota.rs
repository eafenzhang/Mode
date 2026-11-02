//! ZAI Start Plan subscription / quota / remaining 纯映射。
//!
//! 真源：`packages/services/src/usage-stats/providers/bigmodelUsageQuotaProvider.ts`
//! 中导出的 Zai Start Plan helper。

use serde_json::{json, Map, Number, Value};

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

fn read_non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_number(value: Option<&Value>) -> Option<f64> {
    let numeric = match value? {
        Value::Number(number) => number.as_f64()?,
        Value::String(text) if !text.trim().is_empty() => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    numeric.is_finite().then_some(numeric)
}

fn parse_unix_seconds(value: Option<&Value>) -> Option<f64> {
    parse_number(value).filter(|value| *value > 0.0)
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

fn format_unix_seconds_as_iso(value: Option<&Value>) -> Value {
    let Some(seconds) = parse_unix_seconds(value) else {
        return Value::Null;
    };
    let ms = (seconds * 1000.0).round() as i64;
    let days = ms.div_euclid(86_400_000);
    let ms_of_day = ms.rem_euclid(86_400_000);
    let second_of_day = ms_of_day / 1000;
    let millis = ms_of_day % 1000;
    let (year, month, day) = civil_from_days(days);
    Value::String(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60
    ))
}

fn array(value: Option<&Value>) -> &[Value] {
    value
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn active_plan(plan: &Map<String, Value>) -> bool {
    read_non_empty_string(plan.get("status"))
        .map(|status| status.to_lowercase() == "active")
        .unwrap_or(false)
}

fn pick_billing_cycle(plan: &Map<String, Value>) -> Option<String> {
    array(plan.get("entitlements"))
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|entry| read_non_empty_string(entry.get("period")))
        .next()
}

pub fn pick_zai_start_plan_billing_cycle(plan: &Value) -> Value {
    plan.as_object()
        .and_then(pick_billing_cycle)
        .map(Value::String)
        .unwrap_or(Value::Null)
}

pub fn build_zai_start_plan_subscription(plans: Option<&Value>) -> Value {
    let details: Vec<Value> = array(plans)
        .iter()
        .filter_map(Value::as_object)
        .filter(|plan| active_plan(plan))
        .map(|plan| {
            let entitlements: Vec<Value> = array(plan.get("entitlements"))
                .iter()
                .filter_map(Value::as_object)
                .filter_map(|entitlement| {
                    let entitlement_id = read_non_empty_string(entitlement.get("entitlement_id"))?;
                    let mut value = Map::new();
                    value.insert("entitlementId".to_string(), json!(entitlement_id));
                    value.insert(
                        "showName".to_string(),
                        read_non_empty_string(entitlement.get("show_name"))
                            .map(Value::String)
                            .unwrap_or(Value::Null),
                    );
                    value.insert(
                        "effectiveTime".to_string(),
                        format_unix_seconds_as_iso(entitlement.get("effective_at")),
                    );
                    Some(Value::Object(value))
                })
                .collect();
            json!({
                "productId": read_non_empty_string(plan.get("plan_id")).unwrap_or_default(),
                "productName": read_non_empty_string(plan.get("name")).unwrap_or_else(|| "编程套餐".to_string()),
                "purchaseTime": null,
                "beginTime": format_unix_seconds_as_iso(plan.get("starts_at")),
                "billingCycle": pick_billing_cycle(plan),
                "renewTime": null,
                "expireTime": format_unix_seconds_as_iso(plan.get("ends_at")),
                "entitlements": entitlements,
            })
        })
        .collect();
    json!({
        "identityType": "unknown",
        "identityMasked": null,
        "details": details,
    })
}

fn strict_eq(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn normalize_capabilities(value: Option<&Value>) -> Vec<String> {
    array(value)
        .iter()
        .filter_map(Value::as_str)
        .map(|capability| {
            capability
                .strip_prefix("model:")
                .unwrap_or(capability)
                .to_string()
        })
        .filter(|capability| !capability.trim().is_empty())
        .collect()
}

pub fn normalize_zai_start_plan_balance_limits(
    balances: Option<&Value>,
    plans: Option<&Value>,
) -> Vec<Value> {
    let Some(balances) = balances.and_then(Value::as_array) else {
        return Vec::new();
    };
    balances
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|balance| {
            let total = parse_number(balance.get("total_units"));
            let used = parse_number(balance.get("used_units"));
            let remaining = parse_number(balance.get("remaining_units"));
            if total.is_none() && used.is_none() && remaining.is_none() {
                return None;
            }
            let next_reset_seconds = parse_unix_seconds(balance.get("expires_at"));
            let capability_labels = normalize_capabilities(balance.get("capabilities"));
            let capability_type =
                read_non_empty_string(Some(&Value::String(capability_labels.join(", "))));
            let display_name = read_non_empty_string(balance.get("show_name"));
            let plan_id = read_non_empty_string(balance.get("plan_id"));
            let user_plan_id = read_non_empty_string(balance.get("user_plan_id"));
            let plan = array(plans)
                .iter()
                .filter_map(Value::as_object)
                .find(|candidate| {
                    match (
                        user_plan_id.as_deref(),
                        read_non_empty_string(candidate.get("user_plan_id")).as_deref(),
                    ) {
                        (Some(user_plan_id), Some(candidate_user)) => {
                            user_plan_id == candidate_user
                        }
                        _ => match (&plan_id, candidate.get("plan_id")) {
                            (Some(plan_id), Some(Value::String(candidate_plan_id))) => {
                                plan_id == candidate_plan_id
                            }
                            (None, Some(Value::Null)) => true,
                            _ => false,
                        },
                    }
                });
            let period = plan
                .and_then(|plan| {
                    array(plan.get("entitlements"))
                        .iter()
                        .filter_map(Value::as_object)
                        .find(|entry| {
                            strict_eq(entry.get("entitlement_id"), balance.get("entitlement_id"))
                        })
                })
                .and_then(|entry| read_non_empty_string(entry.get("period")));
            let period_start = parse_unix_seconds(balance.get("period_start"));
            let period_end = parse_unix_seconds(balance.get("period_end"));
            let entitlement_id = read_non_empty_string(balance.get("entitlement_id"));
            let meter = read_non_empty_string(balance.get("meter"));
            let limit_type = entitlement_id
                .clone()
                .or(capability_type.clone())
                .or(meter.clone())
                .unwrap_or_else(|| "model_usage".to_string());
            let mut value = Map::new();
            if let Some(bucket_id) = read_non_empty_string(balance.get("bucket_id")) {
                value.insert("bucketId".to_string(), json!(bucket_id));
            }
            if let Some(user_plan_id) = &user_plan_id {
                value.insert("userPlanId".to_string(), json!(user_plan_id));
            }
            if let Some(period_start) = period_start {
                value.insert(
                    "periodStart".to_string(),
                    json_number(period_start * 1000.0),
                );
            }
            if let Some(period_end) = period_end {
                value.insert("periodEnd".to_string(), json_number(period_end * 1000.0));
            }
            if let Some(period) = &period {
                value.insert("period".to_string(), json!(period));
            }
            if let Some(meter) = &meter {
                value.insert("meter".to_string(), json!(meter));
            }
            if let Some(unit_type) = read_non_empty_string(balance.get("unit_type")) {
                value.insert("unitType".to_string(), json!(unit_type));
            }
            value.insert("type".to_string(), json!(limit_type));
            if let Some(plan_id) = &plan_id {
                value.insert("planId".to_string(), json!(plan_id));
            }
            if let Some(unit) = optional_number(total) {
                value.insert("unit".to_string(), unit.clone());
                value.insert("number".to_string(), unit);
            }
            if let Some(used) = optional_number(used) {
                value.insert("usage".to_string(), used.clone());
                value.insert("currentValue".to_string(), used);
            }
            if let Some(remaining) = optional_number(remaining) {
                value.insert("remaining".to_string(), remaining);
            }
            let percentage = match (total, remaining) {
                (Some(total), Some(remaining)) if total > 0.0 => Some(remaining / total),
                _ => None,
            };
            if let Some(percentage) = optional_number(percentage) {
                value.insert("percentage".to_string(), percentage);
            }
            if let Some(next_reset_seconds) = next_reset_seconds {
                value.insert(
                    "nextResetTime".to_string(),
                    json_number(next_reset_seconds * 1000.0),
                );
            }
            let usage_details: Vec<Value> = capability_labels
                .iter()
                .map(|model_code| {
                    let mut detail = Map::new();
                    detail.insert("modelCode".to_string(), json!(model_code));
                    if let Some(display_name) = &display_name {
                        detail.insert("displayName".to_string(), json!(display_name));
                    }
                    detail.insert("usage".to_string(), json_number(used.unwrap_or(0.0)));
                    Value::Object(detail)
                })
                .collect();
            value.insert("usageDetails".to_string(), Value::Array(usage_details));
            Some(Value::Object(value))
        })
        .collect()
}

pub fn build_zai_start_plan_remaining(balances: Option<&Value>) -> Value {
    let limits = normalize_zai_start_plan_balance_limits(balances, None);
    if limits.is_empty() {
        return Value::Null;
    }
    let total: f64 = limits
        .iter()
        .map(|limit| limit["number"].as_f64().unwrap_or(0.0))
        .sum();
    let remaining: f64 = limits
        .iter()
        .map(|limit| limit["remaining"].as_f64().unwrap_or(0.0))
        .sum();
    let next_reset = limits
        .iter()
        .filter_map(|limit| limit["nextResetTime"].as_f64())
        .min_by(|a, b| a.total_cmp(b));
    let mut value = Map::new();
    value.insert("count".to_string(), json_number(remaining));
    value.insert("isShow".to_string(), Value::Bool(true));
    if total > 0.0 {
        value.insert("percentage".to_string(), json_number(remaining / total));
    }
    value.insert(
        "nextResetTime".to_string(),
        next_reset.map(json_number).unwrap_or(Value::Null),
    );
    Value::Object(value)
}

pub fn build_zai_start_plan_quota(balances: Option<&Value>, plans: Option<&Value>) -> Value {
    let limits = normalize_zai_start_plan_balance_limits(balances, plans);
    if limits.is_empty() {
        return Value::Null;
    }
    json!({ "level": "Start", "limits": limits })
}

fn start_plan_identity(value: &str) -> bool {
    value.contains("start-plan") || value.contains("start plan")
}

pub fn pick_current_zai_start_plan(plans: Option<&Value>) -> Value {
    array(plans)
        .iter()
        .filter_map(Value::as_object)
        .find(|plan| {
            let status =
                read_non_empty_string(plan.get("status")).map(|status| status.to_lowercase());
            let plan_id =
                read_non_empty_string(plan.get("plan_id")).map(|value| value.to_lowercase());
            let name = read_non_empty_string(plan.get("name")).map(|value| value.to_lowercase());
            status.as_deref() == Some("active")
                && (plan_id.as_deref().is_some_and(start_plan_identity)
                    || name.as_deref().is_some_and(start_plan_identity))
        })
        .map(|plan| Value::Object(plan.clone()))
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_quota_limits() {
        let balances = json!([{
            "bucket_id": "b",
            "plan_id": "plan",
            "entitlement_id": "e",
            "show_name": "GLM",
            "meter": "tokens",
            "capabilities": ["model:GLM-5"],
            "total_units": 100,
            "used_units": 25,
            "remaining_units": 75,
            "expires_at": 1700000000,
            "period_start": 1690000000,
            "period_end": 1700000000
        }]);
        let limits = normalize_zai_start_plan_balance_limits(Some(&balances), None);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0]["type"], "e");
        assert_eq!(limits[0]["percentage"], 0.75);
    }
}
