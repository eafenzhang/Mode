//! `system` 频道适配器：方法名、参数与返回 JSON 形状。

use std::collections::BTreeMap;
use std::sync::Arc;

use host_channels::system::{INFO, LIST_INTEGRATED_TERMINAL_SHELLS, PROBE_INTRANET};
use host_channels::SystemChannel;
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value;
use system_service::{
    BoxProbeFuture, ServiceProbeParams, ServiceProbeResult, SystemDependencies, SystemService,
    TcpProbeParams,
};

fn channel() -> SystemChannel {
    let deps = SystemDependencies {
        env: BTreeMap::from([("HOME".to_string(), "/home/test".to_string())]),
        platform: "linux".into(),
        is_executable: Arc::new(|_| false),
        tcp_probe: Arc::new(|_params: TcpProbeParams| -> BoxProbeFuture<Result<u64, String>> {
            Box::pin(async { Ok(9) })
        }),
        service_probe: Arc::new(|_params: ServiceProbeParams| -> BoxProbeFuture<
            Result<ServiceProbeResult, String>,
        > { Box::pin(async { Ok(ServiceProbeResult::default()) }) }),
        now_ms: Arc::new(|| 7),
    };
    SystemChannel::new(Arc::new(SystemService::with_dependencies(deps)))
}

async fn call(channel: &SystemChannel, method: &str, arg: Value) -> Result<Value, rpc_host::ChannelError> {
    channel
        .call("ctx".into(), method.into(), arg, CancellationToken::default())
        .await
}

fn json(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn info_returns_homedir_and_platform() {
    let body = json(&call(&channel(), INFO, Value::Array(Vec::new())).await.unwrap());
    assert_eq!(
        body,
        serde_json::json!({ "homedir": "/home/test", "platform": "linux" })
    );
}

#[tokio::test]
async fn shells_are_empty_on_non_windows() {
    let body = json(
        &call(
            &channel(),
            LIST_INTEGRATED_TERMINAL_SHELLS,
            Value::Array(Vec::new()),
        )
        .await
        .unwrap(),
    );
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test]
async fn probe_intranet_returns_camel_case_result() {
    let request = Value::Array(vec![Value::Json(
        r#"{"targets":[{"host":"10.0.0.9","port":443}],"attempts":1}"#.into(),
    )]);
    let body = json(&call(&channel(), PROBE_INTRANET, request).await.unwrap());

    assert_eq!(body["isIntranet"], true);
    assert_eq!(body["reachedTargetCount"], 1);
    assert_eq!(body["totalTargets"], 1);
    assert_eq!(body["requiredSuccessCount"], 1);
    assert_eq!(body["checkedAt"], 7);
    assert_eq!(body["strategy"], "tcp-connect");

    let first = &body["results"][0];
    assert_eq!(first["kind"], "tcp");
    assert_eq!(first["targetId"], "10.0.0.9:443");
    assert_eq!(first["host"], "10.0.0.9");
    assert_eq!(first["port"], 443);
    assert_eq!(first["reachable"], true);
    assert_eq!(first["attemptCount"], 1);
    assert_eq!(first["latencyMs"], 9);
}

#[tokio::test]
async fn probe_intranet_empty_targets_is_not_intranet() {
    let request = Value::Array(vec![Value::Json(r#"{"targets":[]}"#.into())]);
    let body = json(&call(&channel(), PROBE_INTRANET, request).await.unwrap());
    assert_eq!(body["isIntranet"], false);
    assert_eq!(body["strategy"], "tcp-connect");
    assert_eq!(body["requiredSuccessCount"], 1);
}

#[tokio::test]
async fn probe_intranet_rejects_malformed_request() {
    let request = Value::Array(vec![Value::Json(r#"{"targets":"nope"}"#.into())]);
    assert!(call(&channel(), PROBE_INTRANET, request).await.is_err());
    assert!(call(&channel(), PROBE_INTRANET, Value::Array(Vec::new()))
        .await
        .is_err());
}

#[tokio::test]
async fn unknown_command_is_an_error() {
    assert!(call(&channel(), "reboot", Value::Array(Vec::new()))
        .await
        .is_err());
}
