//! 内网探测：归一化、重试与默认 TCP/HTTP 实现。
//!
//! 行为真源 `packages/services/src/system/systemService.ts`。归一化规则与错误文案见
//! `docs/specs/system-service.md`；错误文本只进诊断字段，不是接口契约。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use url::Url;

use crate::types::{
    IntranetProbeRequest, IntranetProbeResult, IntranetProbeServiceTargetResult,
    IntranetProbeTargetResult, IntranetProbeTcpTargetResult, ProbeTarget,
};

pub const DEFAULT_PROBE_TIMEOUT_MS: u64 = 800;
pub const DEFAULT_PROBE_ATTEMPTS: u32 = 2;
pub const MAX_PROBE_ATTEMPTS: u32 = 3;
pub const DEFAULT_PROBE_PORT: u16 = 22;
const MIN_PROBE_TIMEOUT_MS: u64 = 100;
const MAX_PROBE_TIMEOUT_MS: u64 = 10_000;

pub type BoxProbeFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpProbeParams {
    pub host: String,
    pub port: u16,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceProbeParams {
    pub url: String,
    pub timeout_ms: u64,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceProbeResult {
    pub latency_ms: u64,
    pub marker: Option<String>,
}

/// 返回耗时毫秒；失败时返回诊断文本。
pub type TcpProbe = Arc<dyn Fn(TcpProbeParams) -> BoxProbeFuture<Result<u64, String>> + Send + Sync>;
pub type ServiceProbe =
    Arc<dyn Fn(ServiceProbeParams) -> BoxProbeFuture<Result<ServiceProbeResult, String>> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizedTarget {
    Tcp {
        target_id: String,
        host: String,
        port: u16,
        timeout_ms: u64,
    },
    Service {
        target_id: String,
        url: String,
        expected_marker: Option<String>,
        token: Option<String>,
        timeout_ms: u64,
    },
}

impl NormalizedTarget {
    fn kind(&self) -> &'static str {
        match self {
            Self::Tcp { .. } => "tcp",
            Self::Service { .. } => "service",
        }
    }
}

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite())
}

pub fn normalize_attempts(attempts: Option<f64>) -> u32 {
    match finite(attempts) {
        Some(value) => (value.floor() as i64).clamp(1, MAX_PROBE_ATTEMPTS as i64) as u32,
        None => DEFAULT_PROBE_ATTEMPTS,
    }
}

pub fn normalize_required_success_count(
    required: Option<f64>,
    total_targets: usize,
) -> usize {
    if total_targets == 0 {
        return 1;
    }
    match finite(required) {
        Some(value) => (value.floor() as i64).clamp(1, total_targets as i64) as usize,
        None => 1,
    }
}

pub fn normalize_probe_timeout(timeout_ms: Option<f64>) -> u64 {
    match finite(timeout_ms) {
        Some(value) => (value.floor() as i64)
            .clamp(MIN_PROBE_TIMEOUT_MS as i64, MAX_PROBE_TIMEOUT_MS as i64)
            as u64,
        None => DEFAULT_PROBE_TIMEOUT_MS,
    }
}

pub fn normalize_target(target: &ProbeTarget) -> Option<NormalizedTarget> {
    if target.kind.as_deref() == Some("service") {
        normalize_service_target(target)
    } else {
        normalize_tcp_target(target)
    }
}

fn normalize_tcp_target(target: &ProbeTarget) -> Option<NormalizedTarget> {
    let host = target.host.as_deref()?.trim().to_string();
    if host.is_empty() {
        return None;
    }
    let port = match target.port {
        Some(value)
            if value.is_finite()
                && value.fract() == 0.0
                && (1.0..=65_535.0).contains(&value) =>
        {
            value as u16
        }
        _ => DEFAULT_PROBE_PORT,
    };
    let target_id = target
        .id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("{host}:{port}"));

    Some(NormalizedTarget::Tcp {
        target_id,
        host,
        port,
        timeout_ms: normalize_probe_timeout(target.timeout_ms),
    })
}

fn normalize_service_target(target: &ProbeTarget) -> Option<NormalizedTarget> {
    let url_text = target.url.as_deref()?.trim();
    if url_text.is_empty() {
        return None;
    }
    let parsed = Url::parse(url_text).ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    let url = parsed.to_string();

    let expected_marker = trimmed_non_empty(target.expected_marker.as_deref());
    let token = trimmed_non_empty(target.token.as_deref());
    let target_id = trimmed_non_empty(target.id.as_deref()).unwrap_or_else(|| url.clone());

    Some(NormalizedTarget::Service {
        target_id,
        url,
        expected_marker,
        token,
        timeout_ms: normalize_probe_timeout(target.timeout_ms),
    })
}

fn trimmed_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 目标之间并发、每个目标内部串行重试（`Promise.all` + `for` 语义）。
pub async fn run_probes(
    targets: Vec<NormalizedTarget>,
    attempts: u32,
    tcp_probe: &TcpProbe,
    service_probe: &ServiceProbe,
) -> Vec<IntranetProbeTargetResult> {
    let futures = targets.into_iter().map(|target| {
        let tcp_probe = Arc::clone(tcp_probe);
        let service_probe = Arc::clone(service_probe);
        async move {
            match target {
                NormalizedTarget::Tcp {
                    target_id,
                    host,
                    port,
                    timeout_ms,
                } => IntranetProbeTargetResult::Tcp(
                    probe_tcp_with_retry(target_id, host, port, timeout_ms, attempts, &tcp_probe)
                        .await,
                ),
                NormalizedTarget::Service {
                    target_id,
                    url,
                    expected_marker,
                    token,
                    timeout_ms,
                } => IntranetProbeTargetResult::Service(
                    probe_service_with_retry(
                        target_id,
                        url,
                        expected_marker,
                        token,
                        timeout_ms,
                        attempts,
                        &service_probe,
                    )
                    .await,
                ),
            }
        }
    });
    futures_util::future::join_all(futures).await
}

async fn probe_tcp_with_retry(
    target_id: String,
    host: String,
    port: u16,
    timeout_ms: u64,
    attempts: u32,
    tcp_probe: &TcpProbe,
) -> IntranetProbeTcpTargetResult {
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        let params = TcpProbeParams {
            host: host.clone(),
            port,
            timeout_ms,
        };
        match tcp_probe(params).await {
            Ok(latency_ms) => {
                return IntranetProbeTcpTargetResult {
                    target_id,
                    kind: "tcp",
                    host,
                    port,
                    reachable: true,
                    attempt_count: attempt,
                    latency_ms: Some(latency_ms),
                    error: None,
                }
            }
            Err(error) => last_error = error,
        }
    }
    IntranetProbeTcpTargetResult {
        target_id,
        kind: "tcp",
        host,
        port,
        reachable: false,
        attempt_count: attempts,
        latency_ms: None,
        error: Some(if last_error.is_empty() {
            "probe failed".to_string()
        } else {
            last_error
        }),
    }
}

#[allow(clippy::too_many_arguments)]
async fn probe_service_with_retry(
    target_id: String,
    url: String,
    expected_marker: Option<String>,
    token: Option<String>,
    timeout_ms: u64,
    attempts: u32,
    service_probe: &ServiceProbe,
) -> IntranetProbeServiceTargetResult {
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        let params = ServiceProbeParams {
            url: url.clone(),
            timeout_ms,
            token: token.clone(),
        };
        match service_probe(params).await {
            Ok(result) => {
                if let Some(expected) = expected_marker.as_deref() {
                    if result.marker.as_deref() != Some(expected) {
                        last_error = format!(
                            "marker mismatch(expected={expected}, actual={})",
                            result.marker.as_deref().unwrap_or("<empty>")
                        );
                        continue;
                    }
                }
                return IntranetProbeServiceTargetResult {
                    target_id,
                    kind: "service",
                    url,
                    reachable: true,
                    attempt_count: attempt,
                    latency_ms: Some(result.latency_ms),
                    marker: result.marker,
                    error: None,
                };
            }
            Err(error) => last_error = error,
        }
    }
    IntranetProbeServiceTargetResult {
        target_id,
        kind: "service",
        url,
        reachable: false,
        attempt_count: attempts,
        latency_ms: None,
        marker: None,
        error: Some(if last_error.is_empty() {
            "probe failed".to_string()
        } else {
            last_error
        }),
    }
}

pub fn resolve_probe_strategy(targets: &[NormalizedTarget]) -> &'static str {
    if targets.iter().all(|target| target.kind() == "tcp") {
        return "tcp-connect";
    }
    if targets.iter().all(|target| target.kind() == "service") {
        return "service-http";
    }
    "mixed"
}

pub fn aggregate(
    targets: Vec<NormalizedTarget>,
    results: Vec<IntranetProbeTargetResult>,
    required_success_count: usize,
    checked_at: u64,
) -> IntranetProbeResult {
    let reached_target_count = results.iter().filter(|result| result.reachable()).count();
    let total_targets = targets.len();
    let strategy = resolve_probe_strategy(&targets);
    IntranetProbeResult {
        is_intranet: total_targets > 0 && reached_target_count >= required_success_count,
        reached_target_count,
        required_success_count,
        total_targets,
        checked_at,
        strategy,
        results,
    }
}

/// 从请求归一化出要探测的目标（丢弃无效目标）。
pub fn normalize_request(request: &IntranetProbeRequest) -> Vec<NormalizedTarget> {
    request
        .targets
        .iter()
        .filter_map(normalize_target)
        .collect()
}

/// 默认 TCP 探测：只等连接建立，超时文案与 TS 对齐。
pub fn default_tcp_probe(params: TcpProbeParams) -> BoxProbeFuture<Result<u64, String>> {
    Box::pin(async move {
        let started = Instant::now();
        let connect = TcpStream::connect((params.host.as_str(), params.port));
        match tokio::time::timeout(Duration::from_millis(params.timeout_ms), connect).await {
            Ok(Ok(_stream)) => Ok(started.elapsed().as_millis() as u64),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err(format!("timeout({}ms)", params.timeout_ms)),
        }
    })
}

/// 默认 service 探测：GET + 可选 token 头 + `{ok, marker}` JSON。
pub fn default_service_probe(params: ServiceProbeParams) -> BoxProbeFuture<Result<ServiceProbeResult, String>> {
    Box::pin(async move {
        let started = Instant::now();
        let client = http_client()?;
        let mut request = client.get(&params.url);
        if let Some(token) = params.token.as_deref() {
            request = request.header("x-mode-intranet-token", token);
        }

        let timeout = Duration::from_millis(params.timeout_ms);
        let body = tokio::time::timeout(timeout, async {
            let response = request.send().await.map_err(|error| error.to_string())?;
            let status = response.status();
            if !status.is_success() {
                return Err(format!("HTTP {}", status.as_u16()));
            }
            response
                .bytes()
                .await
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|_| format!("timeout({}ms)", params.timeout_ms))??;

        let value: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| "invalid service response".to_string())?;
        let (ok, marker) = parse_service_response(&value)?;
        if !ok {
            return Err("service returned ok=false".to_string());
        }
        Ok(ServiceProbeResult {
            latency_ms: started.elapsed().as_millis() as u64,
            marker,
        })
    })
}

fn parse_service_response(value: &serde_json::Value) -> Result<(bool, Option<String>), String> {
    let Some(object) = value.as_object() else {
        return Err("invalid service response".to_string());
    };
    let Some(ok) = object.get("ok") else {
        return Err("invalid service response: missing ok".to_string());
    };
    let Some(ok) = ok.as_bool() else {
        return Err("invalid service response: missing ok".to_string());
    };
    // TS 用 `"marker" in response && marker !== undefined && typeof marker !== "string"`
    // 判错：显式 `null` 也会命中（`typeof null === "object"`），只有「键不存在」才是缺省。
    let marker = match object.get("marker") {
        None => None,
        Some(serde_json::Value::String(marker)) => Some(marker.clone()),
        Some(_) => {
            return Err("invalid service response: marker must be string".to_string())
        }
    };
    Ok((ok, marker))
}

fn http_client() -> Result<reqwest::Client, String> {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            // 与 crates/platform 一致：reqwest 以 rustls-no-provider 编译，进程级 provider 由
            // 使用者安装；重复安装返回 Err，忽略即可。
            let _ = rustls::crypto::ring::default_provider().install_default();
            reqwest::Client::builder()
                .build()
                .map_err(|error| error.to_string())
        })
        .clone()
}
