//! CUA raw input socket 解析策略。
//!
//! 真源：`packages/services/src/cua-permission-broker/cuaRawInput.ts` 的
//! `createCuaRawInputSocketResolver`。该层只发现/触发创建，不 fork、不写 helper 状态；
//! broker 调用与真实 socket 解析留在宿主装配层。
//!
//! 与 TS 的差异：Rust 同步 `&mut self` 串行化调用，不共享 async in-flight promise；
//! 并发调用方由宿主用锁串行化并复用同一结果。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub type SocketResolver = Arc<dyn Fn() -> Option<String> + Send + Sync>;
pub type EnabledCheck = Arc<dyn Fn() -> bool + Send + Sync>;
pub type HelperEnsurer = Arc<dyn Fn() -> Option<String> + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub struct RawInputSocketResolverOptions {
    /// 托管 helper 的 live socket peek；`None` 表示无托管 helper 或未运行。
    pub resolve_live_socket_path: SocketResolver,
    /// 稳定 socket 探测（probe-only，不拉起）。
    pub probe_stable_socket_path: SocketResolver,
    /// 被控总开关；缺省关闭，不产生创建副作用。
    pub is_remote_desktop_server_enabled: EnabledCheck,
    /// 创建入口；`None` 表示宿主未接线，保持 probe-only。
    pub ensure_remote_desktop_helper: Option<HelperEnsurer>,
    /// 创建失败后的退避窗口；0 = 每次调用都重试。
    pub ensure_failure_backoff_ms: u64,
    pub now_ms: Clock,
}

impl Default for RawInputSocketResolverOptions {
    fn default() -> Self {
        Self {
            resolve_live_socket_path: Arc::new(|| None),
            probe_stable_socket_path: Arc::new(|| None),
            is_remote_desktop_server_enabled: Arc::new(|| false),
            ensure_remote_desktop_helper: None,
            ensure_failure_backoff_ms: 0,
            now_ms: Arc::new(system_now_ms),
        }
    }
}

pub struct RawInputSocketResolver {
    options: RawInputSocketResolverOptions,
    ensure_deferred_until: u64,
}

impl RawInputSocketResolver {
    pub fn new(options: RawInputSocketResolverOptions) -> Self {
        Self {
            options,
            ensure_deferred_until: 0,
        }
    }

    /// live peek → 稳定探测 → 开关打开时触发一次创建；失败后按退避窗口返回 `None`。
    pub fn resolve(&mut self) -> Option<String> {
        if let Some(path) = (self.options.resolve_live_socket_path)() {
            return Some(path);
        }
        if let Some(path) = (self.options.probe_stable_socket_path)() {
            return Some(path);
        }
        let Some(ensure) = &self.options.ensure_remote_desktop_helper else {
            return None;
        };
        if !(self.options.is_remote_desktop_server_enabled)() {
            return None;
        }
        let now = (self.options.now_ms)();
        if now < self.ensure_deferred_until {
            return None;
        }
        match ensure() {
            Some(path) => Some(path),
            None => {
                self.ensure_deferred_until =
                    now.saturating_add(self.options.ensure_failure_backoff_ms);
                None
            }
        }
    }
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
