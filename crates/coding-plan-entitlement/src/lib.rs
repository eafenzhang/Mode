//! Coding Plan 订阅权益解析：个人/团队订阅列表信封 → 可用性。
//!
//! 真源：`packages/services/src/bigmodel/codingPlanEntitlement.ts` 与
//! `codingPlanProviderAvailability.ts` 的 `validateSubscriptionListAvailability`。
//! 网络请求与 401/403 分类由调用方负责；本 crate 只判定已取得的 payload。

use account_provider_overlay::{
    CodingPlanAvailabilityResult, CodingPlanUnavailableReason,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutoRenew {
    Bool(bool),
    Number(f64),
}

/// 个人 Coding Plan 订阅条目（zod `personalSchema` 的必填/可选字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalCodingPlanSubscription {
    #[serde(default)]
    pub product_id: Option<String>,
    #[serde(default)]
    pub product_name: Option<String>,
    pub status: String,
    pub in_current_period: bool,
    #[serde(default)]
    pub auto_renew: Option<AutoRenew>,
    #[serde(default)]
    pub next_renew_time: Option<String>,
    #[serde(default)]
    pub valid: Option<String>,
    #[serde(default)]
    pub billing_cycle: Option<String>,
}

/// 团队 Coding Plan 订阅详情（zod `teamSchema`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamCodingPlanSubscription {
    pub has_subscription: bool,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub member_grant_status: Option<String>,
    #[serde(default)]
    pub product_id: Option<String>,
    #[serde(default)]
    pub product_name: Option<String>,
    #[serde(default)]
    pub subscribe_end_time: Option<String>,
    #[serde(default)]
    pub subscribe_period: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitlementUnavailableReason {
    Expired,
    Unassigned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodingPlanEntitlement<T> {
    Available { subscription: T },
    Unavailable { reason: Option<EntitlementUnavailableReason> },
    Unknown,
}

/// `isCodingPlanProduct`：productId/productName 任一含 `coding`（大小写不敏感）。
pub fn is_coding_plan_product(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    ["productId", "productName"].iter().any(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|field| field.to_lowercase().contains("coding"))
    })
}

/// `isActivePersonalCodingPlan`。
pub fn is_active_personal_coding_plan(subscription: &PersonalCodingPlanSubscription) -> bool {
    let product = serde_json::json!({
        "productId": subscription.product_id.clone(),
        "productName": subscription.product_name.clone(),
    });
    is_coding_plan_product(&product)
        && subscription.status == "VALID"
        && subscription.in_current_period
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    code: Option<f64>,
    #[serde(default)]
    data: Option<Value>,
}

fn successful_data(payload: &Value) -> Option<Value> {
    let envelope: Envelope = serde_json::from_value(payload.clone()).ok()?;
    if envelope.success == Some(false) {
        return None;
    }
    if let Some(code) = envelope.code {
        if code != 0.0 && code != 200.0 {
            return None;
        }
    }
    envelope.data
}

/// `fetchPersonalCodingPlanEntitlement` 的纯解析部分。
pub fn parse_personal_coding_plan_entitlement(
    payload: &Value,
) -> CodingPlanEntitlement<PersonalCodingPlanSubscription> {
    let Some(data) = successful_data(payload) else {
        return CodingPlanEntitlement::Unknown;
    };
    let Some(items) = data.as_array() else {
        return CodingPlanEntitlement::Unknown;
    };
    let mut malformed_coding_entry = false;
    for item in items {
        match serde_json::from_value::<PersonalCodingPlanSubscription>(item.clone()) {
            Ok(subscription) if is_active_personal_coding_plan(&subscription) => {
                return CodingPlanEntitlement::Available { subscription };
            }
            Ok(_) => {}
            Err(_) if is_coding_plan_product(item) => malformed_coding_entry = true,
            Err(_) => {}
        }
    }
    if malformed_coding_entry {
        CodingPlanEntitlement::Unknown
    } else {
        CodingPlanEntitlement::Unavailable { reason: None }
    }
}

/// `fetchTeamCodingPlanEntitlement` 的纯解析部分。
pub fn parse_team_coding_plan_entitlement(
    payload: &Value,
) -> CodingPlanEntitlement<TeamCodingPlanSubscription> {
    let Some(data) = successful_data(payload) else {
        return CodingPlanEntitlement::Unknown;
    };
    let Ok(subscription) = serde_json::from_value::<TeamCodingPlanSubscription>(data) else {
        return CodingPlanEntitlement::Unknown;
    };
    if !subscription.has_subscription {
        return CodingPlanEntitlement::Unavailable { reason: None };
    }
    if subscription.status.as_deref() == Some("EXPIRED") {
        return CodingPlanEntitlement::Unavailable {
            reason: Some(EntitlementUnavailableReason::Expired),
        };
    }
    if subscription.status.as_deref() == Some("EFFECTIVE")
        && subscription.member_grant_status.as_deref() == Some("UNASSIGNED")
    {
        return CodingPlanEntitlement::Unavailable {
            reason: Some(EntitlementUnavailableReason::Unassigned),
        };
    }
    if subscription.status.as_deref() != Some("EFFECTIVE")
        || subscription.member_grant_status.as_deref() != Some("VALID")
    {
        return CodingPlanEntitlement::Unknown;
    }
    CodingPlanEntitlement::Available { subscription }
}

/// `validateSubscriptionListAvailability`：Key 未准备好时只能表示未知。
pub fn resolve_subscription_list_availability(
    payload: &Value,
    has_api_key: bool,
) -> CodingPlanAvailabilityResult {
    if !has_api_key {
        return CodingPlanAvailabilityResult::Unknown;
    }
    match parse_personal_coding_plan_entitlement(payload) {
        CodingPlanEntitlement::Available { .. } => CodingPlanAvailabilityResult::Available {
            models: None,
        },
        CodingPlanEntitlement::Unavailable { .. } => CodingPlanAvailabilityResult::Unavailable {
            reason: CodingPlanUnavailableReason::CodingPlanNotEntitled,
        },
        CodingPlanEntitlement::Unknown => CodingPlanAvailabilityResult::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn personal(status: &str, in_period: bool, product_id: &str) -> Value {
        json!({
            "productId": product_id,
            "status": status,
            "inCurrentPeriod": in_period
        })
    }

    #[test]
    fn active_personal_coding_plan_is_available() {
        let payload = json!({
            "success": true,
            "code": 0,
            "data": [personal("VALID", true, "mode-coding-plan-monthly")]
        });
        match parse_personal_coding_plan_entitlement(&payload) {
            CodingPlanEntitlement::Available { subscription } => {
                assert_eq!(subscription.status, "VALID");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn inactive_or_non_coding_entries_are_unavailable() {
        let payload = json!({
            "code": 200,
            "data": [
                personal("EXPIRED", true, "mode-coding-plan"),
                personal("VALID", true, "mode-start-plan")
            ]
        });
        assert_eq!(
            parse_personal_coding_plan_entitlement(&payload),
            CodingPlanEntitlement::Unavailable { reason: None }
        );
    }

    #[test]
    fn malformed_coding_entry_is_unknown_but_unrelated_malformed_is_ignored() {
        let malformed_coding = json!({
            "data": [
                { "productId": "mode-coding-plan", "status": 42 },
                { "productId": "other", "status": "VALID", "inCurrentPeriod": true }
            ]
        });
        assert_eq!(
            parse_personal_coding_plan_entitlement(&malformed_coding),
            CodingPlanEntitlement::Unknown
        );

        let unrelated = json!({
            "data": [
                { "productId": "other", "status": 42 },
                { "productId": "mode-start-plan", "status": "VALID", "inCurrentPeriod": true }
            ]
        });
        assert_eq!(
            parse_personal_coding_plan_entitlement(&unrelated),
            CodingPlanEntitlement::Unavailable { reason: None }
        );
    }

    #[test]
    fn business_failure_or_non_array_is_unknown() {
        assert_eq!(
            parse_personal_coding_plan_entitlement(&json!({ "success": false })),
            CodingPlanEntitlement::Unknown
        );
        assert_eq!(
            parse_personal_coding_plan_entitlement(&json!({ "code": 401, "data": [] })),
            CodingPlanEntitlement::Unknown
        );
        assert_eq!(
            parse_personal_coding_plan_entitlement(&json!({ "data": {} })),
            CodingPlanEntitlement::Unknown
        );
    }

    #[test]
    fn team_states_map_to_entitlement() {
        let available = json!({
            "data": {
                "hasSubscription": true,
                "status": "EFFECTIVE",
                "memberGrantStatus": "VALID"
            }
        });
        assert!(matches!(
            parse_team_coding_plan_entitlement(&available),
            CodingPlanEntitlement::Available { .. }
        ));

        let expired = json!({
            "data": { "hasSubscription": true, "status": "EXPIRED" }
        });
        assert_eq!(
            parse_team_coding_plan_entitlement(&expired),
            CodingPlanEntitlement::Unavailable {
                reason: Some(EntitlementUnavailableReason::Expired)
            }
        );

        let unassigned = json!({
            "data": {
                "hasSubscription": true,
                "status": "EFFECTIVE",
                "memberGrantStatus": "UNASSIGNED"
            }
        });
        assert_eq!(
            parse_team_coding_plan_entitlement(&unassigned),
            CodingPlanEntitlement::Unavailable {
                reason: Some(EntitlementUnavailableReason::Unassigned)
            }
        );

        let unknown = json!({
            "data": { "hasSubscription": true, "status": "PENDING", "memberGrantStatus": "VALID" }
        });
        assert_eq!(
            parse_team_coding_plan_entitlement(&unknown),
            CodingPlanEntitlement::Unknown
        );

        let none = json!({ "data": { "hasSubscription": false } });
        assert_eq!(
            parse_team_coding_plan_entitlement(&none),
            CodingPlanEntitlement::Unavailable { reason: None }
        );
    }

    #[test]
    fn missing_api_key_is_unknown_and_unavailable_maps_to_not_entitled() {
        let payload = json!({
            "data": [personal("VALID", true, "mode-coding-plan")]
        });
        assert_eq!(
            resolve_subscription_list_availability(&payload, false),
            CodingPlanAvailabilityResult::Unknown
        );
        assert_eq!(
            resolve_subscription_list_availability(&json!({ "data": [] }), true),
            CodingPlanAvailabilityResult::Unavailable {
                reason: CodingPlanUnavailableReason::CodingPlanNotEntitled
            }
        );
        assert_eq!(
            resolve_subscription_list_availability(&payload, true),
            CodingPlanAvailabilityResult::Available { models: None }
        );
    }
}
