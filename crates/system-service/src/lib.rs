//! `system` 频道的服务实现（W3-2）。
//!
//! 行为真源：`packages/services/src/system/*`；规范见 `docs/specs/system-service.md`。
//! 三个方法都没有跨域状态：`info` 读环境，shell 列表读环境 + 可执行文件，内网探测只做
//! TCP/HTTP 尝试。

mod probe;
mod service;
mod shells;
mod types;

pub use probe::{
    aggregate, default_service_probe, default_tcp_probe, normalize_attempts, normalize_probe_timeout,
    normalize_request, normalize_required_success_count, normalize_target, resolve_probe_strategy,
    run_probes, BoxProbeFuture, NormalizedTarget, ServiceProbe, ServiceProbeParams,
    ServiceProbeResult, TcpProbe, TcpProbeParams, DEFAULT_PROBE_ATTEMPTS, DEFAULT_PROBE_PORT,
    DEFAULT_PROBE_TIMEOUT_MS, MAX_PROBE_ATTEMPTS,
};
pub use service::{Clock, ExecutableCheck, SystemDependencies, SystemService};
pub use shells::list_integrated_terminal_shells;
pub use types::{
    IntegratedTerminalShellOption, IntranetProbeRequest, IntranetProbeResult,
    IntranetProbeServiceTargetResult, IntranetProbeTargetResult, IntranetProbeTcpTargetResult,
    ProbeTarget, SystemInfo,
};
