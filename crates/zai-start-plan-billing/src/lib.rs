//! ZAI Start Plan balance 的纯逻辑。
//!
//! 真源：`packages/services/src/model-provider/zaiStartPlanBilling.ts`。
//! 网络请求、缓存与 HTTP Date 由调用方负责；本 crate 只做 URL、modelIds 与过期归一化。

mod quota;

use serde_json::{json, Value};

pub use quota::{
    build_zai_start_plan_quota, build_zai_start_plan_remaining, build_zai_start_plan_subscription,
    normalize_zai_start_plan_balance_limits, pick_current_zai_start_plan,
    pick_zai_start_plan_billing_cycle,
};

const CANONICAL_GLM_MODEL_IDS: [&str; 22] = [
    "GLM-5.3",
    "GLM-5.3-Flash",
    "GLM-5V-Turbo",
    "GLM-5.2",
    "GLM-5.1",
    "GLM-5.1-Highspeed",
    "GLM-5",
    "GLM-5-Turbo",
    "GLM-4.7",
    "GLM-4.7-FlashX",
    "GLM-4.7-Flash",
    "GLM-4.6",
    "GLM-4.5-Air",
    "GLM-4.5",
    "GLM-4.6V",
    "GLM-4.6V-Flash",
    "GLM-4.6V-FlashX",
    "GLM-4.1V-Thinking-FlashX",
    "GLM-4.1V-Thinking-Flash",
    "GLM-4-FlashX-250414",
    "GLM-4-Flash-250414",
    "GLM-4V-Flash",
];

fn normalize_official_glm_model_id(model_id: &str) -> String {
    let lower = model_id.to_lowercase();
    CANONICAL_GLM_MODEL_IDS
        .iter()
        .find(|candidate| candidate.to_lowercase() == lower)
        .map(|candidate| (*candidate).to_string())
        .unwrap_or_else(|| model_id.to_string())
}

pub fn build_zai_start_plan_balance_url_for(
    base_url: &str,
    app_version: &str,
) -> Result<String, String> {
    let mut url = url::Url::parse(base_url).map_err(|error| error.to_string())?;
    let mut replaced = false;
    let mut pairs: Vec<(String, String)> = Vec::new();
    for (key, value) in url.query_pairs() {
        if key == "app_version" {
            if !replaced {
                pairs.push(("app_version".to_string(), app_version.to_string()));
                replaced = true;
            }
            continue;
        }
        pairs.push((key.into_owned(), value.into_owned()));
    }
    if !replaced {
        pairs.push(("app_version".to_string(), app_version.to_string()));
    }
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(&key, &value);
        }
    }
    Ok(url.to_string())
}

pub fn resolve_zai_start_plan_balance_model_ids(payload: &Value) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut model_ids = Vec::new();
    let balances = payload
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("balances"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for balance in balances {
        let Some(balance) = balance.as_object() else {
            continue;
        };
        let from_capabilities: Vec<String> = balance
            .get("capabilities")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter_map(|capability| {
                if capability.to_lowercase().starts_with("model:") {
                    Some(capability["model:".len()..].trim().to_string())
                } else {
                    None
                }
            })
            .filter(|value| !value.is_empty())
            .collect();
        let candidates = if from_capabilities.is_empty() {
            vec![balance
                .get("show_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()]
        } else {
            from_capabilities
        };
        for candidate in candidates {
            let model_id = normalize_official_glm_model_id(candidate.trim());
            let key = model_id.to_lowercase();
            if model_id.is_empty() || !seen.insert(key) {
                continue;
            }
            model_ids.push(model_id);
        }
    }
    model_ids
}

fn js_number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Some(0.0)
            } else {
                trimmed
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
            }
        }
        Value::Bool(boolean) => Some(if *boolean { 1.0 } else { 0.0 }),
        Value::Null => Some(0.0),
        _ => None,
    }
}

fn field_status_is_active(plan: &serde_json::Map<String, Value>) -> bool {
    plan.get("status")
        .and_then(Value::as_str)
        .map(|status| status.trim().to_lowercase() == "active")
        .unwrap_or(false)
}

fn strict_eq(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

pub fn normalize_start_plan_expiry(
    payload: &Value,
    response_time: Option<f64>,
    now_secs: f64,
) -> Value {
    let Some(payload_object) = payload.as_object() else {
        return payload.clone();
    };
    let Some(data) = payload_object.get("data").and_then(Value::as_object) else {
        return payload.clone();
    };
    let server_time = data.get("server_time").and_then(Value::as_f64);
    let now = response_time.unwrap_or_else(|| {
        server_time
            .filter(|value| value.is_finite() && *value >= 0.0)
            .unwrap_or(now_secs)
    });
    let plans_input = data
        .get("plans")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let plans: Vec<Value> = plans_input
        .iter()
        .map(|plan| {
            let Some(plan_object) = plan.as_object() else {
                return plan.clone();
            };
            let end = js_number(plan_object.get("ends_at"));
            let expired = field_status_is_active(plan_object)
                && end.is_some_and(|end| end.is_finite() && end > 0.0 && end <= now);
            if !expired {
                return plan.clone();
            }
            let mut plan = plan_object.clone();
            plan.insert("status".to_string(), json!("expired"));
            Value::Object(plan)
        })
        .collect();
    let mut next_data = data.clone();
    next_data.insert("plans".to_string(), Value::Array(plans.clone()));
    if let Some(balances) = data.get("balances").and_then(Value::as_array) {
        let filtered: Vec<Value> = balances
            .iter()
            .filter(|balance| {
                let Some(balance_object) = balance.as_object() else {
                    return true;
                };
                let owners: Vec<&serde_json::Map<String, Value>> = plans
                    .iter()
                    .filter_map(Value::as_object)
                    .filter(|plan| {
                        let balance_user = balance_object
                            .get("user_plan_id")
                            .and_then(Value::as_str)
                            .filter(|value| !value.is_empty());
                        let plan_user = plan
                            .get("user_plan_id")
                            .and_then(Value::as_str)
                            .filter(|value| !value.is_empty());
                        match (balance_user, plan_user) {
                            (Some(balance_user), Some(plan_user)) => balance_user == plan_user,
                            _ => strict_eq(balance_object.get("plan_id"), plan.get("plan_id")),
                        }
                    })
                    .collect();
                owners.is_empty()
                    || owners.iter().any(|plan| {
                        plan.get("status")
                            .and_then(Value::as_str)
                            .map(|status| status.trim().to_lowercase() != "expired")
                            .unwrap_or(true)
                    })
            })
            .cloned()
            .collect();
        next_data.insert("balances".to_string(), Value::Array(filtered));
    }
    let mut output = payload_object.clone();
    output.insert("data".to_string(), Value::Object(next_data));
    Value::Object(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_known_glm_ids() {
        assert_eq!(
            resolve_zai_start_plan_balance_model_ids(&json!({
                "data": { "balances": [{ "capabilities": ["model:glm-5", "model:GLM-5"] }] }
            })),
            vec!["GLM-5"]
        );
    }

    #[test]
    fn expires_active_plan_by_response_time() {
        let payload = json!({
            "data": {
                "plans": [{ "plan_id": "p", "status": " ACTIVE ", "ends_at": 100 }],
                "balances": [{ "plan_id": "p", "remaining_units": 1 }]
            }
        });
        let normalized = normalize_start_plan_expiry(&payload, Some(100.0), 0.0);
        assert_eq!(normalized["data"]["plans"][0]["status"], "expired");
        assert!(normalized["data"]["balances"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

/// Start Plan 可用性：balance 业务信封 + plans 身份/状态 + 有效期。
///
/// 与 TS `resolveStartPlanBalanceAvailability` 同口径：业务失败 → auth_failed；
/// 无 active Start Plan → not_entitled；模型为空且权益全部在未来 → pending；
/// 否则 available（有模型白名单则带上）。
pub fn resolve_start_plan_availability(
    payload: &Value,
    now_seconds: f64,
) -> account_provider_overlay::CodingPlanAvailabilityResult {
    use account_provider_overlay::{
        CodingPlanAvailabilityResult, CodingPlanUnavailableReason,
    };

    if !is_successful_business_envelope(payload) {
        return CodingPlanAvailabilityResult::Unavailable {
            reason: CodingPlanUnavailableReason::CodingPlanAuthFailed,
        };
    }
    let plans = payload
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("plans"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let active_plans: Vec<&serde_json::Map<String, Value>> = plans
        .iter()
        .filter_map(Value::as_object)
        .filter(|plan| plan_status_is_active(plan) && plan_identity_matches(plan))
        .collect();
    if active_plans.is_empty() {
        return CodingPlanAvailabilityResult::Unavailable {
            reason: CodingPlanUnavailableReason::CodingPlanNotEntitled,
        };
    }

    let models = resolve_zai_start_plan_balance_model_ids(payload);
    let server_time = payload
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("server_time"))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0);
    let reference_now = server_time.unwrap_or(now_seconds);
    let mut effective_times: Vec<Option<f64>> = Vec::new();
    for plan in active_plans {
        let entitlements = plan.get("entitlements").and_then(Value::as_array);
        match entitlements {
            Some(entries) if !entries.is_empty() => {
                for entry in entries {
                    effective_times.push(
                        entry
                            .get("effective_at")
                            .and_then(js_time_number),
                    );
                }
            }
            _ => effective_times.push(
                plan.get("starts_at").and_then(js_time_number),
            ),
        }
    }
    if models.is_empty()
        && !effective_times.is_empty()
        && effective_times
            .iter()
            .all(|value| value.is_some_and(|time| time.is_finite() && time > reference_now))
    {
        let effective_at = effective_times
            .iter()
            .flatten()
            .copied()
            .fold(f64::INFINITY, f64::min);
        return CodingPlanAvailabilityResult::Pending {
            effective_at: effective_at as i64,
            models: Vec::new(),
        };
    }
    CodingPlanAvailabilityResult::Available {
        models: (!models.is_empty()).then_some(models),
    }
}

fn is_successful_business_envelope(payload: &Value) -> bool {
    if payload.get("success").and_then(Value::as_bool) == Some(false) {
        return false;
    }
    match payload.get("code") {
        None => true,
        Some(value) => matches!(value.as_i64(), Some(0 | 200)),
    }
}

fn plan_status_is_active(plan: &serde_json::Map<String, Value>) -> bool {
    plan.get("status")
        .and_then(Value::as_str)
        .map(|status| status.trim().to_lowercase() == "active")
        .unwrap_or(false)
}

fn plan_identity_matches(plan: &serde_json::Map<String, Value>) -> bool {
    let normalize = |key: &str| {
        plan.get(key)
            .and_then(Value::as_str)
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty())
    };
    let plan_id = normalize("plan_id");
    let name = normalize("name");
    match (plan_id, name) {
        (None, None) => true,
        (plan_id, name) => [plan_id, name]
            .into_iter()
            .flatten()
            .any(|value| value.contains("start-plan") || value.contains("start plan")),
    }
}

/// 与 TS `Number(value)` 对齐，但空串/null 在调用方语义里表示「无时间」。
fn js_time_number(value: &Value) -> Option<f64> {
    match value {
        Value::Null => None,
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                trimmed.parse::<f64>().ok().filter(|value| value.is_finite())
            }
        }
        Value::Bool(boolean) => Some(if *boolean { 1.0 } else { 0.0 }),
        _ => None,
    }
}

#[cfg(test)]
mod availability_tests {
    use super::*;
    use account_provider_overlay::{
        CodingPlanAvailabilityResult, CodingPlanUnavailableReason,
    };
    use serde_json::json;

    fn active_plan_payload() -> Value {
        json!({
            "success": true,
            "code": 0,
            "data": {
                "plans": [{ "plan_id": "mode-v3-start-plan", "name": "Mode V3 Start Plan", "status": "active" }],
                "balances": [{ "capabilities": ["model:glm-4.6"] }]
            }
        })
    }

    #[test]
    fn active_start_plan_with_models_is_available() {
        let result = resolve_start_plan_availability(&active_plan_payload(), 1_000.0);
        assert_eq!(
            result,
            CodingPlanAvailabilityResult::Available {
                models: Some(vec!["GLM-4.6".to_string()])
            }
        );
    }

    #[test]
    fn no_active_start_plan_is_not_entitled() {
        let payload = json!({
            "success": true,
            "data": { "plans": [{ "plan_id": "mode-v3-coding-plan", "status": "active" }] }
        });
        assert_eq!(
            resolve_start_plan_availability(&payload, 1_000.0),
            CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanNotEntitled
            }
        );
    }

    #[test]
    fn business_failure_is_auth_failed() {
        let payload = json!({ "success": false, "code": 401 });
        assert_eq!(
            resolve_start_plan_availability(&payload, 1_000.0),
            CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanAuthFailed
            }
        );
    }

    #[test]
    fn future_entitlement_without_models_is_pending_using_server_time() {
        let payload = json!({
            "success": true,
            "data": {
                "server_time": 1_000,
                "plans": [{
                    "plan_id": "mode-v3-start-plan",
                    "status": "active",
                    "entitlements": [{ "effective_at": 2_000 }]
                }],
                "balances": []
            }
        });
        // 调用方本地时间已过 2000，但服务端时间仍是 1000 → pending。
        assert_eq!(
            resolve_start_plan_availability(&payload, 5_000.0),
            CodingPlanAvailabilityResult::Pending {
                effective_at: 2_000,
                models: Vec::new()
            }
        );
    }

    #[test]
    fn active_plan_without_identity_is_accepted() {
        let payload = json!({
            "success": true,
            "data": { "plans": [{ "status": " active " }], "balances": [] }
        });
        assert_eq!(
            resolve_start_plan_availability(&payload, 1_000.0),
            CodingPlanAvailabilityResult::Available { models: None }
        );
    }
}
