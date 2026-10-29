//! 与 `packages/shared/src/protocol.ts`、`intranetProbe.ts` 对齐的线格式类型。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SystemInfo {
    pub homedir: String,
    pub platform: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegratedTerminalShellOption {
    /// `cmd` | `git-bash`
    pub dialect: String,
    pub id: String,
    pub label: String,
    pub path: String,
    /// `system` | `path`
    pub source: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntranetProbeRequest {
    #[serde(default)]
    pub targets: Vec<ProbeTarget>,
    pub attempts: Option<f64>,
    pub required_success_count: Option<f64>,
}

/// 请求里的目标：tcp 与 service 共用一张表，按 `kind` 分流。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeTarget {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub host: Option<String>,
    pub port: Option<f64>,
    pub timeout_ms: Option<f64>,
    pub url: Option<String>,
    pub expected_marker: Option<String>,
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntranetProbeTcpTargetResult {
    pub target_id: String,
    pub kind: &'static str,
    pub host: String,
    pub port: u16,
    pub reachable: bool,
    pub attempt_count: u32,
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntranetProbeServiceTargetResult {
    pub target_id: String,
    pub kind: &'static str,
    pub url: String,
    pub reachable: bool,
    pub attempt_count: u32,
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum IntranetProbeTargetResult {
    Tcp(IntranetProbeTcpTargetResult),
    Service(IntranetProbeServiceTargetResult),
}

impl IntranetProbeTargetResult {
    pub fn reachable(&self) -> bool {
        match self {
            Self::Tcp(result) => result.reachable,
            Self::Service(result) => result.reachable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntranetProbeResult {
    pub is_intranet: bool,
    pub reached_target_count: usize,
    pub required_success_count: usize,
    pub total_targets: usize,
    pub checked_at: u64,
    /// `tcp-connect` | `service-http` | `mixed`
    pub strategy: &'static str,
    pub results: Vec<IntranetProbeTargetResult>,
}
