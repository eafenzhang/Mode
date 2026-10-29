//! `system` 服务的装配层：依赖注入（环境、平台、可执行判定、探测实现、时钟）。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::probe::{
    aggregate, normalize_attempts, normalize_request, normalize_required_success_count, run_probes,
    ServiceProbe, TcpProbe,
};
use crate::shells::{env_get_ci, list_integrated_terminal_shells};
use crate::types::{
    IntegratedTerminalShellOption, IntranetProbeRequest, IntranetProbeResult, SystemInfo,
};

pub type ExecutableCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub struct SystemDependencies {
    pub env: BTreeMap<String, String>,
    pub platform: String,
    pub is_executable: ExecutableCheck,
    pub tcp_probe: TcpProbe,
    pub service_probe: ServiceProbe,
    pub now_ms: Clock,
}

impl SystemDependencies {
    /// 进程环境 + 平台默认实现（与 TS `createSystemService()` 无参调用一致）。
    pub fn from_process() -> Self {
        Self {
            env: std::env::vars().collect(),
            platform: node_platform().to_string(),
            is_executable: Arc::new(|path| {
                std::fs::metadata(path)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
            }),
            tcp_probe: Arc::new(crate::probe::default_tcp_probe),
            service_probe: Arc::new(crate::probe::default_service_probe),
            now_ms: Arc::new(current_time_ms),
        }
    }
}

pub struct SystemService {
    deps: SystemDependencies,
}

impl SystemService {
    pub fn new() -> Self {
        Self {
            deps: SystemDependencies::from_process(),
        }
    }

    pub fn with_dependencies(deps: SystemDependencies) -> Self {
        Self { deps }
    }

    /// `os.homedir()` + `process.platform`。
    pub fn info(&self) -> SystemInfo {
        SystemInfo {
            homedir: node_homedir(&self.deps.env),
            platform: self.deps.platform.clone(),
        }
    }

    pub async fn list_integrated_terminal_shells(&self) -> Vec<IntegratedTerminalShellOption> {
        list_integrated_terminal_shells(
            &self.deps.env,
            &self.deps.platform,
            &*self.deps.is_executable,
        )
    }

    pub async fn probe_intranet(&self, request: &IntranetProbeRequest) -> IntranetProbeResult {
        let targets = normalize_request(request);
        let attempts = normalize_attempts(request.attempts);
        let required_success_count =
            normalize_required_success_count(request.required_success_count, targets.len());
        let results = run_probes(
            targets.clone(),
            attempts,
            &self.deps.tcp_probe,
            &self.deps.service_probe,
        )
        .await;
        let checked_at = (self.deps.now_ms)();
        aggregate(targets, results, required_success_count, checked_at)
    }
}

impl Default for SystemService {
    fn default() -> Self {
        Self::new()
    }
}

/// `process.platform`：Windows/macOS 的 Rust 名字必须映射。
fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

/// Node `os.homedir()` 的优先级：Windows 先 `USERPROFILE`，再 `HOMEDRIVE`+`HOMEPATH`；
/// Unix 用 `HOME`。空值等同于未设置（与 libuv 一致）。
fn node_homedir(env: &BTreeMap<String, String>) -> String {
    if let Some(profile) = non_empty(env_get_ci(env, "USERPROFILE")) {
        return profile.to_string();
    }
    if let (Some(drive), Some(path)) = (
        non_empty(env_get_ci(env, "HOMEDRIVE")),
        non_empty(env_get_ci(env, "HOMEPATH")),
    ) {
        return format!("{drive}{path}");
    }
    non_empty(env_get_ci(env, "HOME"))
        .map(str::to_string)
        .unwrap_or_default()
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn homedir_prefers_profile_then_drive_pair_then_home() {
        assert_eq!(
            node_homedir(&env(&[
                ("USERPROFILE", "C:\\Users\\a"),
                ("HOME", "/home/a")
            ])),
            "C:\\Users\\a"
        );
        assert_eq!(
            node_homedir(&env(&[
                ("HOMEDRIVE", "D:"),
                ("HOMEPATH", "\\Users\\a"),
                ("HOME", "/home/a")
            ])),
            "D:\\Users\\a"
        );
        assert_eq!(node_homedir(&env(&[("HOME", "/home/a")])), "/home/a");
    }

    #[test]
    fn homedir_ignores_empty_values() {
        assert_eq!(
            node_homedir(&env(&[("USERPROFILE", ""), ("HOME", "/home/a")])),
            "/home/a"
        );
        assert_eq!(node_homedir(&env(&[])), "");
    }

    #[test]
    fn info_uses_injected_environment_and_platform() {
        let service = SystemService::with_dependencies(SystemDependencies {
            env: env(&[("HOME", "/home/a")]),
            platform: "linux".into(),
            is_executable: Arc::new(|_| false),
            tcp_probe: Arc::new(|_| Box::pin(async { Ok(1) })),
            service_probe: Arc::new(|_| Box::pin(async { Ok(Default::default()) })),
            now_ms: Arc::new(|| 42),
        });
        let info = service.info();
        assert_eq!(info.homedir, "/home/a");
        assert_eq!(info.platform, "linux");
    }
}
