//! Workspace Hook 配置的纯解析函数。
//!
//! 真源：`packages/shared/src/workspace-hook-config.ts` 的
//! `resolveWorkspaceHookTimeoutMs` / `resolveWorkspaceHookMaxOutputBytes` /
//! `resolveWorkspaceHookConfiguredGates`。摘要与信任判定都以这些结果为输入，
//! 所以数值口径必须逐字一致。
//!
//! 2026-10-30 起补齐发现层：`schema`（严格校验）与 `discovery`（候选路径发现、读取、
//! 运行时根归并），真源同 `packages/shared/src/workspace-hook-config.ts`。

mod discovery;
mod schema;

pub use discovery::{
    create_workspace_hook_source_input, discover_workspace_hook_config_paths, path_to_forward_slashes,
    read_workspace_hook_project_sources, resolve_path, resolve_workspace_hook_runtime_root,
    ConfigPathRef, RuntimeRoot, RuntimeRootInput, SourceInput, SourceReadError,
    DEFAULT_WORKSPACE_HOOK_MAX_OUTPUT_BYTES, DEFAULT_WORKSPACE_HOOK_TIMEOUT_MS,
};
pub use schema::{validate_hooks_config, ValidationError, WORKSPACE_HOOK_EVENT_NAMES};

/// 超时：`timeoutMs` 优先；`command` 类型允许秒级 `timeout` 换算；否则用默认值。
/// 结果 `max(1, round(x))`。
pub fn resolve_timeout_ms(
    hook_type: &str,
    timeout_ms: Option<f64>,
    timeout_seconds: Option<f64>,
    default_timeout_ms: u64,
) -> u64 {
    let raw = match timeout_ms {
        Some(value) => value,
        None => {
            if hook_type == "command" {
                timeout_seconds
                    .map(|seconds| seconds * 1000.0)
                    .unwrap_or(default_timeout_ms as f64)
            } else {
                default_timeout_ms as f64
            }
        }
    };
    raw.round().max(1.0) as u64
}

pub fn resolve_max_output_bytes(max_output_bytes: f64) -> u64 {
    max_output_bytes.round().max(1.0) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredGates {
    pub source_root_enabled: bool,
    pub declaration_enabled: bool,
    pub runtime_hooks_enabled: bool,
    pub configured_enabled: bool,
}

/// 三个开关按「缺省为开」归并；三个都开才算 configured。
pub fn resolve_configured_gates(
    source_enabled: Option<bool>,
    declaration_enabled: Option<bool>,
    runtime_hooks_enabled: bool,
) -> ConfiguredGates {
    let source_root_enabled = source_enabled != Some(false);
    let declaration_enabled = declaration_enabled != Some(false);
    ConfiguredGates {
        source_root_enabled,
        declaration_enabled,
        runtime_hooks_enabled,
        configured_enabled: source_root_enabled && declaration_enabled && runtime_hooks_enabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_resolution_matches_ts_rules() {
        assert_eq!(resolve_timeout_ms("command", Some(5000.0), None, 60_000), 5000);
        assert_eq!(resolve_timeout_ms("command", None, Some(2.0), 60_000), 2000);
        assert_eq!(resolve_timeout_ms("process", None, Some(2.0), 60_000), 60_000);
        assert_eq!(resolve_timeout_ms("command", Some(0.4), None, 60_000), 1);
        assert_eq!(resolve_timeout_ms("command", Some(-5.0), None, 60_000), 1);
        assert_eq!(resolve_max_output_bytes(0.0), 1);
        assert_eq!(resolve_max_output_bytes(1024.4), 1024);
    }

    #[test]
    fn gates_default_to_enabled() {
        let all = resolve_configured_gates(None, None, true);
        assert!(all.configured_enabled);
        assert!(!resolve_configured_gates(Some(false), None, true).configured_enabled);
        assert!(!resolve_configured_gates(None, Some(false), true).configured_enabled);
        assert!(!resolve_configured_gates(None, None, false).configured_enabled);
        assert!(resolve_configured_gates(Some(true), Some(true), true).configured_enabled);
    }
}
