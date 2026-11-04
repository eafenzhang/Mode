//! `process/*` 遥测通知的严格镜像。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的
//! `modeProcessResourceSampleSchema`、`modeMcpTelemetryEventSchema`、
//! `modeMcpResourceSamplesSchema` 与 `modeToolExecResourceSchema`。
//! 仅镜像形状与枚举；zod 的数值上限与正则收紧由生产者保证（本地 CLI）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryPlatform {
    Aix,
    Android,
    Darwin,
    Freebsd,
    Haiku,
    Linux,
    Netbsd,
    Openbsd,
    Sunos,
    Win32,
    Cygwin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryArch {
    Arm,
    Arm64,
    Ia32,
    Loong64,
    Mips,
    Mipsel,
    Ppc,
    Ppc64,
    Riscv64,
    S390,
    S390x,
    X64,
}

/// `modeProcessResourceSampleSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessResourceSample {
    pub platform: TelemetryPlatform,
    pub arch: TelemetryArch,
    pub logical_cpu_count: u64,
    pub interval_ms: f64,
    pub cpu_cores: f64,
    pub cpu_percent: f64,
    pub rss_kb: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heap_used_kb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_minutes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_memory_gb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpSource {
    Builtin,
    Plugin,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpIsolation {
    Session,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpProcessTelemetryBase {
    pub mcp_id: String,
    pub mcp_instance_id: String,
    pub mcp_isolation: McpIsolation,
    pub mcp_source: McpSource,
}

/// `modeMcpTelemetryEventSchema`：按 `kind` 的判别联合。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum McpTelemetryEvent {
    #[serde(rename = "process_start", rename_all = "camelCase")]
    ProcessStart {
        arch: TelemetryArch,
        occurred_at: u64,
        platform: TelemetryPlatform,
        #[serde(flatten)]
        process: McpProcessTelemetryBase,
    },
    #[serde(rename = "process_crash", rename_all = "camelCase")]
    ProcessCrash {
        arch: TelemetryArch,
        occurred_at: u64,
        platform: TelemetryPlatform,
        #[serde(flatten)]
        process: McpProcessTelemetryBase,
        affected_session_count: u64,
        exit_code: Option<i64>,
        signal: Option<String>,
        uptime_ms: f64,
    },
    #[serde(rename = "session_startup", rename_all = "camelCase")]
    SessionStartup {
        arch: TelemetryArch,
        occurred_at: u64,
        platform: TelemetryPlatform,
        configured_count: u64,
        connected_count: u64,
        failed_count: u64,
        process_count: u64,
        session_id: String,
    },
    #[serde(rename = "memory", rename_all = "camelCase")]
    Memory {
        arch: TelemetryArch,
        occurred_at: u64,
        platform: TelemetryPlatform,
        #[serde(flatten)]
        process: McpProcessTelemetryBase,
        memory_kb: f64,
        memory_scope: McpMemoryScope,
        orphan_suspected: bool,
        owner_session_count: u64,
        unowned_seconds: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpMemoryScope {
    ProcessTree,
    DirectProcess,
}

/// `modeMcpResourceSampleSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpResourceSample {
    pub mcp_id: String,
    pub instance_token: String,
    pub sampled_at: u64,
    pub interval_ms: f64,
    pub process_count: u64,
    pub rss_kb_total: f64,
    pub rss_kb_max_process: f64,
    pub cpu_time_ms_delta: f64,
    pub uptime_minutes: u64,
    pub platform: TelemetryPlatform,
    pub arch: TelemetryArch,
    pub logical_cpu_count: u64,
    pub total_memory_gb: u64,
}

/// `modeToolExecResourceSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolExecResource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_token: Option<String>,
    pub platform: TelemetryPlatform,
    pub tool_name: BashToolName,
    pub duration_ms: f64,
    pub exit_kind: ToolExitKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_rss_kb_peak: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_cpu_time_ms: Option<f64>,
    pub sample_count: u64,
    pub cli_rss_kb: f64,
    pub system_free_memory_kb: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BashToolName {
    Bash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExitKind {
    Completed,
    Timeout,
    Killed,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_resource_sample_accepts_optional_telemetry_fields() {
        let sample: ProcessResourceSample = serde_json::from_value(serde_json::json!({
            "platform": "win32",
            "arch": "x64",
            "logicalCpuCount": 8,
            "intervalMs": 15000,
            "cpuCores": 0.5,
            "cpuPercent": 12.5,
            "rssKb": 1024,
            "heapUsedKb": 512,
            "uptimeMinutes": 3,
            "totalMemoryGb": 16,
            "instanceToken": "abcdefgh"
        }))
        .unwrap();
        assert_eq!(sample.platform, TelemetryPlatform::Win32);
        assert_eq!(sample.instance_token.as_deref(), Some("abcdefgh"));
    }

    #[test]
    fn mcp_telemetry_is_tagged_and_strict() {
        let event: McpTelemetryEvent = serde_json::from_value(serde_json::json!({
            "kind": "process_crash",
            "platform": "linux",
            "arch": "arm64",
            "occurredAt": 1,
            "mcpId": "builtin:files",
            "mcpInstanceId": "i1",
            "mcpIsolation": "session",
            "mcpSource": "builtin",
            "affectedSessionCount": 1,
            "exitCode": null,
            "signal": null,
            "uptimeMs": 1000
        }))
        .unwrap();
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["kind"], "process_crash");
        assert!(value["exitCode"].is_null());
        let error = serde_json::from_value::<McpTelemetryEvent>(serde_json::json!({
            "kind": "process_start",
            "platform": "linux",
            "arch": "arm64",
            "occurredAt": 1,
            "mcpId": "builtin:files",
            "mcpInstanceId": "i1",
            "mcpIsolation": "session",
            "mcpSource": "builtin",
            "extra": true
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"), "{error}");
    }

    #[test]
    fn tool_exec_resource_rejects_unknown_exit_kind() {
        let error = serde_json::from_value::<ToolExecResource>(serde_json::json!({
            "platform": "win32",
            "toolName": "bash",
            "durationMs": 15000,
            "exitKind": "exploded",
            "sampleCount": 1,
            "cliRssKb": 10,
            "systemFreeMemoryKb": 100
        }))
        .unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}
