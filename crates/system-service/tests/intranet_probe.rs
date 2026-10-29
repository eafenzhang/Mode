//! 内网探测的归一化、重试与聚合（对应 `docs/specs/system-service.md`）。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::json;
use system_service::{
    BoxProbeFuture, IntranetProbeRequest, ServiceProbeParams, ServiceProbeResult,
    SystemDependencies, SystemService, TcpProbeParams,
};

fn request(value: serde_json::Value) -> IntranetProbeRequest {
    serde_json::from_value(value).expect("请求必须能反序列化")
}

fn base_deps() -> SystemDependencies {
    SystemDependencies {
        env: BTreeMap::new(),
        platform: "linux".into(),
        is_executable: Arc::new(|_| false),
        tcp_probe: Arc::new(|_| Box::pin(async { Ok(1) })),
        service_probe: Arc::new(|_| Box::pin(async { Ok(ServiceProbeResult::default()) })),
        now_ms: Arc::new(|| 1_700_000_000_000),
    }
}

fn queue<T>(results: Vec<Result<T, String>>) -> Arc<Mutex<VecDeque<Result<T, String>>>> {
    Arc::new(Mutex::new(results.into()))
}

fn with_tcp_probe(
    deps: SystemDependencies,
    queue: Arc<Mutex<VecDeque<Result<u64, String>>>>,
    seen: Arc<Mutex<Vec<TcpProbeParams>>>,
) -> SystemService {
    let deps = SystemDependencies {
        tcp_probe: Arc::new(move |params: TcpProbeParams| -> BoxProbeFuture<Result<u64, String>> {
            seen.lock().unwrap().push(params);
            let result = queue
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("exhausted".to_string()));
            Box::pin(async move { result })
        }),
        ..deps
    };
    SystemService::with_dependencies(deps)
}

fn with_service_probe(
    deps: SystemDependencies,
    queue: Arc<Mutex<VecDeque<Result<ServiceProbeResult, String>>>>,
    seen: Arc<Mutex<Vec<ServiceProbeParams>>>,
) -> SystemService {
    let deps = SystemDependencies {
        service_probe: Arc::new(
            move |params: ServiceProbeParams| -> BoxProbeFuture<Result<ServiceProbeResult, String>> {
                seen.lock().unwrap().push(params);
                let result = queue
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| Err("exhausted".to_string()));
                Box::pin(async move { result })
            },
        ),
        ..deps
    };
    SystemService::with_dependencies(deps)
}

#[tokio::test]
async fn normalizes_targets_and_drops_invalid_ones() {
    let seen_tcp = Arc::new(Mutex::new(Vec::new()));
    let service = with_tcp_probe(base_deps(), queue(vec![Ok(5), Ok(6)]), Arc::clone(&seen_tcp));

    let result = service
        .probe_intranet(&request(json!({
            "targets": [
                { "host": " 10.0.0.1 " },
                { "host": "   " },
                { "kind": "service", "url": "ftp://example.test" },
                { "kind": "service", "url": " http://example.test/health " }
            ],
            "attempts": 1
        })))
        .await;

    assert_eq!(result.total_targets, 2, "空 host 与非 http(s) URL 必须被丢弃");
    assert_eq!(result.strategy, "mixed");
    assert_eq!(result.required_success_count, 1);
    assert_eq!(result.checked_at, 1_700_000_000_000);

    let seen = seen_tcp.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].host, "10.0.0.1");
    assert_eq!(seen[0].port, 22, "缺省端口必须是 22");
    assert_eq!(seen[0].timeout_ms, 800, "缺省单次超时必须是 800ms");
}

#[tokio::test]
async fn clamps_attempts_timeout_port_and_ids() {
    let seen_tcp = Arc::new(Mutex::new(Vec::new()));
    let service = with_tcp_probe(
        base_deps(),
        queue(vec![Err("x".into()), Err("y".into()), Err("z".into())]),
        Arc::clone(&seen_tcp),
    );

    let result = service
        .probe_intranet(&request(json!({
            "targets": [
                { "id": "  pinned  ", "host": "h", "port": 70000, "timeoutMs": 5 },
                { "host": "h2", "port": 22.5, "timeoutMs": 50_000 }
            ],
            "attempts": 9
        })))
        .await;

    let seen = seen_tcp.lock().unwrap();
    assert_eq!(seen.len(), 6, "每个目标最多尝试 3 次");
    assert_eq!(seen[0].port, 22, "非法端口回落 22");
    assert_eq!(seen[0].timeout_ms, 100, "超时下限 100ms");
    assert_eq!(seen[3].port, 22, "非整数端口回落 22");
    assert_eq!(seen[3].timeout_ms, 10_000, "超时上限 10s");

    match &result.results[0] {
        system_service::IntranetProbeTargetResult::Tcp(tcp) => {
            assert_eq!(tcp.target_id, "pinned");
            assert_eq!(tcp.attempt_count, 3);
            assert_eq!(tcp.error.as_deref(), Some("z"), "取最后一次失败原因");
        }
        other => panic!("第一个目标应是 tcp: {other:?}"),
    }
}

#[tokio::test]
async fn retries_until_success_and_reports_attempt_count() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let service = with_tcp_probe(
        base_deps(),
        queue(vec![Err("refused".into()), Ok(17)]),
        Arc::clone(&seen),
    );

    let result = service
        .probe_intranet(&request(json!({
            "targets": [{ "host": "10.0.0.2", "port": 443 }],
            "attempts": 3
        })))
        .await;

    match &result.results[0] {
        system_service::IntranetProbeTargetResult::Tcp(tcp) => {
            assert!(tcp.reachable);
            assert_eq!(tcp.attempt_count, 2);
            assert_eq!(tcp.latency_ms, Some(17));
            assert_eq!(tcp.error, None);
        }
        other => panic!("应当是 tcp: {other:?}"),
    }
    assert_eq!(seen.lock().unwrap().len(), 2);
    assert!(result.is_intranet);
}

#[tokio::test]
async fn service_marker_mismatch_is_unreachable() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let service = with_service_probe(
        base_deps(),
        queue(vec![Ok(ServiceProbeResult {
            latency_ms: 3,
            marker: Some("other".into()),
        })]),
        Arc::clone(&seen),
    );

    let result = service
        .probe_intranet(&request(json!({
            "targets": [{
                "kind": "service",
                "url": "https://probe.test/health",
                "expectedMarker": "good",
                "token": " t ",
                "timeoutMs": 500
            }],
            "attempts": 1
        })))
        .await;

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].token.as_deref(), Some("t"), "token 必须 trim");
    assert_eq!(seen[0].timeout_ms, 500);

    match &result.results[0] {
        system_service::IntranetProbeTargetResult::Service(svc) => {
            assert!(!svc.reachable);
            assert_eq!(
                svc.error.as_deref(),
                Some("marker mismatch(expected=good, actual=other)")
            );
            assert_eq!(svc.target_id, "https://probe.test/health");
        }
        other => panic!("应当是 service: {other:?}"),
    }
    assert!(!result.is_intranet);
}

#[tokio::test]
async fn service_success_reports_latency_and_marker() {
    let service = with_service_probe(
        base_deps(),
        queue(vec![Ok(ServiceProbeResult {
            latency_ms: 12,
            marker: Some("good".into()),
        })]),
        Arc::new(Mutex::new(Vec::new())),
    );

    let result = service
        .probe_intranet(&request(json!({
            "targets": [{
                "id": " probe ",
                "kind": "service",
                "url": "http://probe.test/health",
                "expectedMarker": "good"
            }]
        })))
        .await;

    assert_eq!(result.strategy, "service-http");
    match &result.results[0] {
        system_service::IntranetProbeTargetResult::Service(svc) => {
            assert!(svc.reachable);
            assert_eq!(svc.target_id, "probe");
            assert_eq!(svc.latency_ms, Some(12));
            assert_eq!(svc.marker.as_deref(), Some("good"));
        }
        other => panic!("应当是 service: {other:?}"),
    }
}

#[tokio::test]
async fn required_success_count_controls_verdict() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let service = with_tcp_probe(
        base_deps(),
        queue(vec![Ok(1), Err("down".into()), Err("down".into())]),
        Arc::clone(&seen),
    );

    let result = service
        .probe_intranet(&request(json!({
            "targets": [
                { "host": "a", "port": 1 },
                { "host": "b", "port": 2 }
            ],
            "attempts": 1,
            "requiredSuccessCount": 10
        })))
        .await;

    assert_eq!(result.required_success_count, 2, "阈值夹到目标总数");
    assert_eq!(result.reached_target_count, 1);
    assert!(!result.is_intranet);

    let zero = service
        .probe_intranet(&request(json!({
            "targets": [{ "host": "a" }],
            "attempts": 1,
            "requiredSuccessCount": 0
        })))
        .await;
    assert_eq!(zero.required_success_count, 1, "阈值下限 1");
}

#[tokio::test]
async fn empty_targets_are_not_intranet() {
    let service = SystemService::with_dependencies(base_deps());
    let result = service
        .probe_intranet(&request(json!({ "targets": [] })))
        .await;

    assert!(!result.is_intranet);
    assert_eq!(result.total_targets, 0);
    assert_eq!(result.required_success_count, 1);
    assert_eq!(result.strategy, "tcp-connect", "空数组按 every 语义落到 tcp");
    assert!(result.results.is_empty());
}
