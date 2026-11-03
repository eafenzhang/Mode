use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cua_raw_input::{
    BrokerCallError, CuaHelperError, RawInputBrokerResult, RawInputInjector,
    RawInputInjectorOptions,
};
use serde_json::{json, Value};

fn injector(
    socket: Result<Option<String>, BrokerCallError>,
    result: Result<RawInputBrokerResult, BrokerCallError>,
    calls: Arc<AtomicUsize>,
) -> RawInputInjector {
    RawInputInjector::new(RawInputInjectorOptions {
        resolve_socket_path: Arc::new(move || socket.clone()),
        call_broker: Arc::new(move |path, events, timeout| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(path, "helper.sock");
            assert_eq!(events, vec![json!({"k": "move"})]);
            assert_eq!(timeout, 5_000);
            result.clone()
        }),
        timeout_ms: 5_000,
    })
}

#[test]
fn rejects_non_array_events() {
    let calls = Arc::new(AtomicUsize::new(0));
    let injector = injector(
        Ok(Some("helper.sock".to_string())),
        Ok(RawInputBrokerResult {
            dispatched: Some(1.0),
        }),
        Arc::clone(&calls),
    );
    let error = injector.inject(json!({"k": "move"})).unwrap_err();
    assert_eq!(error.code, "invalid_request");
    assert_eq!(error.message, "raw_input events must be an array");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn missing_socket_is_helper_unavailable() {
    let calls = Arc::new(AtomicUsize::new(0));
    let injector = injector(
        Ok(None),
        Ok(RawInputBrokerResult {
            dispatched: Some(1.0),
        }),
        Arc::clone(&calls),
    );
    let error = injector.inject(json!([{"k": "move"}])).unwrap_err();
    assert_eq!(error.code, "helper_unavailable");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn success_returns_dispatched_count() {
    let calls = Arc::new(AtomicUsize::new(0));
    let injector = injector(
        Ok(Some("helper.sock".to_string())),
        Ok(RawInputBrokerResult {
            dispatched: Some(3.0),
        }),
        Arc::clone(&calls),
    );
    let dispatch = injector.inject(json!([{"k": "move"}])).expect("inject");
    assert_eq!(dispatch.dispatched, 3.0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn invalid_result_is_helper_unavailable() {
    let calls = Arc::new(AtomicUsize::new(0));
    let injector = injector(
        Ok(Some("helper.sock".to_string())),
        Ok(RawInputBrokerResult { dispatched: None }),
        calls,
    );
    let error = injector.inject(json!([{"k": "move"}])).unwrap_err();
    assert_eq!(error.code, "helper_unavailable");
    assert_eq!(
        error.message,
        "Computer Use Helper returned an invalid raw_input result"
    );
}

#[test]
fn broker_errors_map_unreachable_codes_to_helper_unavailable() {
    for code in [
        "stale_socket",
        "timeout",
        "unavailable",
        "broker_unavailable",
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let injector = injector(
            Ok(Some("helper.sock".to_string())),
            Err(BrokerCallError::Broker {
                code: code.to_string(),
                message: "broker failed".to_string(),
            }),
            calls,
        );
        let error = injector.inject(json!([{"k": "move"}])).unwrap_err();
        assert_eq!(error.code, "helper_unavailable", "{code}");
        assert_eq!(error.message, "broker failed");
    }
}

#[test]
fn broker_protocol_error_keeps_code() {
    let calls = Arc::new(AtomicUsize::new(0));
    let injector = injector(
        Ok(Some("helper.sock".to_string())),
        Err(BrokerCallError::Broker {
            code: "method_not_found".to_string(),
            message: "no method".to_string(),
        }),
        calls,
    );
    let error = injector.inject(json!([{"k": "move"}])).unwrap_err();
    assert_eq!(error.code, "method_not_found");
}

#[test]
fn helper_error_passes_through_and_other_errors_become_unavailable() {
    let calls = Arc::new(AtomicUsize::new(0));
    let helper = injector(
        Err(BrokerCallError::Helper(CuaHelperError::new(
            "invalid_request",
            "bad request",
        ))),
        Ok(RawInputBrokerResult { dispatched: None }),
        Arc::clone(&calls),
    );
    let error = helper.inject(json!([{"k": "move"}])).unwrap_err();
    assert_eq!(error.code, "invalid_request");
    assert_eq!(error.message, "bad request");

    let other = injector(
        Ok(Some("helper.sock".to_string())),
        Err(BrokerCallError::Other("unexpected".to_string())),
        calls,
    );
    let error = other.inject(json!([{"k": "move"}])).unwrap_err();
    assert_eq!(error.code, "helper_unavailable");
    assert_eq!(error.message, "unexpected");
}

#[test]
fn empty_events_array_is_forwarded() {
    let calls = Arc::new(AtomicUsize::new(0));
    let injector = RawInputInjector::new(RawInputInjectorOptions {
        resolve_socket_path: Arc::new(|| Ok(Some("helper.sock".to_string()))),
        call_broker: {
            let calls = Arc::clone(&calls);
            Arc::new(move |_path, events, _timeout| {
                calls.fetch_add(1, Ordering::SeqCst);
                assert!(events.is_empty());
                Ok(RawInputBrokerResult {
                    dispatched: Some(0.0),
                })
            })
        },
        timeout_ms: 5_000,
    });
    let dispatch: cua_raw_input::RawInputDispatch =
        injector.inject(Value::Array(Vec::new())).expect("inject");
    assert_eq!(dispatch.dispatched, 0.0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
