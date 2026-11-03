use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use usage_stats::{
    is_coding_plan_model_provider_id, AppUsageStatsSource, CodingPlanQuotaSource,
    ProviderBalanceSource, UsageStatsService, NO_BIGMODEL_API_KEY,
};

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<String>>,
    last_request: Mutex<Option<Value>>,
}

impl Recorder {
    fn record(&self, method: &str, request: &Value) {
        self.calls.lock().unwrap().push(method.to_string());
        *self.last_request.lock().unwrap() = Some(request.clone());
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn last_request(&self) -> Option<Value> {
        self.last_request.lock().unwrap().clone()
    }
}

struct FakeAppUsage(Arc<Recorder>);
impl AppUsageStatsSource for FakeAppUsage {
    fn get_app_usage_stats(&self, request: &Value) -> Result<Value, String> {
        self.0.record("app", request);
        Ok(json!({ "source": "app" }))
    }
}

struct FakeQuota(Arc<Recorder>);
impl CodingPlanQuotaSource for FakeQuota {
    fn get_coding_plan_usage_snapshot(&self, request: &Value) -> Result<Value, String> {
        self.0.record("coding-plan-usage", request);
        Ok(json!({ "source": "coding-plan-usage" }))
    }

    fn get_coding_plan_reset_status(&self, request: &Value) -> Result<Value, String> {
        self.0.record("reset-status", request);
        Ok(json!({ "source": "reset-status" }))
    }

    fn request_coding_plan_reset_opportunity(&self, request: &Value) -> Result<Value, String> {
        self.0.record("reset-opportunity", request);
        Ok(json!({ "source": "reset-opportunity" }))
    }

    fn use_coding_plan_reset(&self, request: &Value) -> Result<Value, String> {
        self.0.record("reset-use", request);
        Ok(json!({ "source": "reset-use" }))
    }

    fn mark_coding_plan_reset_history_read(&self, request: &Value) -> Result<(), String> {
        self.0.record("reset-history-read", request);
        Ok(())
    }

    fn get_usage_stats_snapshot(&self, request: &Value) -> Result<Value, String> {
        self.0.record("usage-snapshot", request);
        Ok(json!({ "source": "usage-snapshot" }))
    }

    fn get_entitlement_snapshot(&self, request: &Value) -> Result<Value, String> {
        self.0.record("entitlement", request);
        Ok(json!({ "source": "entitlement" }))
    }
}

struct FakeBalance(Arc<Recorder>);
impl ProviderBalanceSource for FakeBalance {
    fn get_provider_balance_snapshot(&self, request: &Value) -> Result<Value, String> {
        self.0.record("balance", request);
        Ok(json!({ "source": "balance" }))
    }
}

fn service() -> (
    UsageStatsService,
    Arc<Recorder>,
    Arc<Recorder>,
    Arc<Recorder>,
) {
    let app = Arc::new(Recorder::default());
    let quota = Arc::new(Recorder::default());
    let balance = Arc::new(Recorder::default());
    let service = UsageStatsService::new(
        Arc::new(FakeAppUsage(Arc::clone(&app))),
        Arc::new(FakeQuota(Arc::clone(&quota))),
        Arc::new(FakeBalance(Arc::clone(&balance))),
    );
    (service, app, quota, balance)
}

fn coding_request(provider: &str) -> Value {
    json!({ "preferredProviderId": provider })
}

#[test]
fn coding_plan_provider_ids_match_shared_contract() {
    for id in [
        "account:zai-individual-coding-plan",
        "account:zai-team-coding-plan",
        "account:zai-start-plan",
        "account:bigmodel-individual-coding-plan",
        "account:bigmodel-team-coding-plan",
        "account:bigmodel-start-plan",
    ] {
        assert!(is_coding_plan_model_provider_id(id), "{id}");
    }
    for id in [
        "account:zai-api",
        "account:bigmodel-api",
        "account:other",
        "",
    ] {
        assert!(!is_coding_plan_model_provider_id(id), "{id}");
    }
}

#[test]
fn coding_plan_gate_rejects_non_coding_provider() {
    let (service, _, quota, _) = service();
    for request in [
        coding_request("account:other"),
        json!({}),
        json!({ "preferredProviderId": 42 }),
        Value::Null,
    ] {
        assert_eq!(
            service
                .get_coding_plan_usage_snapshot(&request)
                .unwrap_err(),
            NO_BIGMODEL_API_KEY
        );
    }
    assert!(quota.calls().is_empty());
}

#[test]
fn all_gated_methods_reject_before_delegating() {
    let (service, _, quota, _) = service();
    let request = coding_request("account:other");
    assert!(service.get_coding_plan_reset_status(&request).is_err());
    assert!(service
        .request_coding_plan_reset_opportunity(&request)
        .is_err());
    assert!(service.use_coding_plan_reset(&request).is_err());
    assert!(service
        .mark_coding_plan_reset_history_read(&request)
        .is_err());
    assert!(quota.calls().is_empty());
}

#[test]
fn coding_plan_methods_delegate_when_allowed() {
    let (service, _, quota, _) = service();
    let request = coding_request("account:zai-start-plan");
    assert_eq!(
        service.get_coding_plan_usage_snapshot(&request).unwrap(),
        json!({ "source": "coding-plan-usage" })
    );
    assert_eq!(
        service.get_coding_plan_reset_status(&request).unwrap(),
        json!({ "source": "reset-status" })
    );
    assert_eq!(
        service
            .request_coding_plan_reset_opportunity(&request)
            .unwrap(),
        json!({ "source": "reset-opportunity" })
    );
    assert_eq!(
        service.use_coding_plan_reset(&request).unwrap(),
        json!({ "source": "reset-use" })
    );
    service
        .mark_coding_plan_reset_history_read(&request)
        .unwrap();
    assert_eq!(
        quota.calls(),
        vec![
            "coding-plan-usage",
            "reset-status",
            "reset-opportunity",
            "reset-use",
            "reset-history-read"
        ]
    );
}

#[test]
fn ungated_methods_delegate_and_default_entitlement_request() {
    let (service, app, quota, balance) = service();
    assert_eq!(
        service
            .get_app_usage_snapshot(&json!({ "range": "7d" }))
            .unwrap(),
        json!({ "source": "app" })
    );
    assert_eq!(app.last_request(), Some(json!({ "range": "7d" })));

    assert_eq!(
        service.get_snapshot(&json!({})).unwrap(),
        json!({ "source": "usage-snapshot" })
    );
    assert_eq!(
        service.get_provider_balance_snapshot(&json!({})).unwrap(),
        json!({ "source": "balance" })
    );
    assert_eq!(balance.calls(), vec!["balance"]);

    service.get_entitlement_snapshot(None).unwrap();
    assert_eq!(quota.last_request(), Some(json!({})));
    service
        .get_entitlement_snapshot(Some(&Value::Null))
        .unwrap();
    assert_eq!(quota.last_request(), Some(json!({})));
    service
        .get_entitlement_snapshot(Some(&json!({ "providerId": "x" })))
        .unwrap();
    assert_eq!(quota.last_request(), Some(json!({ "providerId": "x" })));
}

#[test]
fn source_errors_propagate() {
    struct Failing;
    impl CodingPlanQuotaSource for Failing {
        fn get_coding_plan_usage_snapshot(&self, _: &Value) -> Result<Value, String> {
            Err("quota failed".to_string())
        }
        fn get_coding_plan_reset_status(&self, _: &Value) -> Result<Value, String> {
            Ok(json!({}))
        }
        fn request_coding_plan_reset_opportunity(&self, _: &Value) -> Result<Value, String> {
            Ok(json!({}))
        }
        fn use_coding_plan_reset(&self, _: &Value) -> Result<Value, String> {
            Ok(json!({}))
        }
        fn mark_coding_plan_reset_history_read(&self, _: &Value) -> Result<(), String> {
            Ok(())
        }
        fn get_usage_stats_snapshot(&self, _: &Value) -> Result<Value, String> {
            Ok(json!({}))
        }
        fn get_entitlement_snapshot(&self, _: &Value) -> Result<Value, String> {
            Ok(json!({}))
        }
    }
    struct NoopApp;
    impl AppUsageStatsSource for NoopApp {
        fn get_app_usage_stats(&self, _: &Value) -> Result<Value, String> {
            Ok(json!({}))
        }
    }
    struct NoopBalance;
    impl ProviderBalanceSource for NoopBalance {
        fn get_provider_balance_snapshot(&self, _: &Value) -> Result<Value, String> {
            Ok(json!({}))
        }
    }
    let service =
        UsageStatsService::new(Arc::new(NoopApp), Arc::new(Failing), Arc::new(NoopBalance));
    assert_eq!(
        service
            .get_coding_plan_usage_snapshot(&coding_request("account:zai-start-plan"))
            .unwrap_err(),
        "quota failed"
    );
}
