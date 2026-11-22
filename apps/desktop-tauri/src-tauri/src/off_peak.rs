//! 闲时任务调度进壳（G2.6 执行侧）：`MODE_RUST_OFF_PEAK=1` 时在 Tauri 进程内
//! 启动 Rust `OffPeakRuntime`（spec `docs/specs/off-peak-runtime.md`）。
//!
//! 所有权：取号/批量轮询仍由 Node Host `OffPeakTaskService.startSync()` 提供；
//! Rust 只认领 `schedulable=1` 的 queued 任务并派发会话。Node 侧没有 scheduler，
//! 因此不会双派发。默认关闭——未设置时行为与现状完全一致（任务不会被执行）。

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use host_channels::off_peak_host::create_off_peak_dispatch;
use host_channels::ModeTaskChannel;
use off_peak_repo::OffPeakTaskRepo;
use off_peak_runtime::{
    OffPeakRuntime, OffPeakRuntimeOptions, DEFAULT_MAX_DISPATCH_ATTEMPTS, DEFAULT_POLL_INTERVAL_MS,
};
use rpc_host::ServerChannel;
use task_index_repo::TaskIndexRepo;

use crate::inprocess::InprocessState;

pub const ENV_RUST_OFF_PEAK: &str = "MODE_RUST_OFF_PEAK";

fn enabled_from(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("1")
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// 按开关装配并启动闲时调度；未启用时返回 `None`。
pub fn start_if_enabled(
    data_root: &Path,
    inprocess: &InprocessState,
) -> Option<Arc<OffPeakRuntime>> {
    if !enabled_from(std::env::var(ENV_RUST_OFF_PEAK).ok().as_deref()) {
        return None;
    }
    let db_path = data_root.join("v2").join("tasks-index.sqlite");
    let off_peak_repo = Arc::new(OffPeakTaskRepo::new(db_path.clone()));
    let task_index = Arc::new(TaskIndexRepo::new(db_path));
    let task_events = Arc::clone(inprocess.task_events());
    let mode_task: Arc<dyn ServerChannel> = Arc::new(
        ModeTaskChannel::new(
            Arc::clone(&task_index),
            Some(Arc::clone(inprocess.agent_control())),
            None,
            Arc::clone(&task_events),
        )
        .with_data_root(data_root.to_path_buf()),
    );
    let dispatch =
        create_off_peak_dispatch(mode_task, Arc::clone(&task_events), Arc::clone(&task_index));
    let runtime = OffPeakRuntime::new(OffPeakRuntimeOptions {
        repo: off_peak_repo,
        dispatch,
        // 与 TS `POLL_INTERVAL_MS` / `DEFAULT_POLL_INTERVAL_MS` 同值。
        poll_interval_ms: DEFAULT_POLL_INTERVAL_MS,
        max_dispatch_attempts: DEFAULT_MAX_DISPATCH_ATTEMPTS,
        clock: Arc::new(now_millis),
        log: Arc::new(|level, message| {
            crate::host::shell_log(&format!("off-peak {level}: {message}"))
        }),
    });
    runtime.start();
    crate::host::shell_log("off-peak runtime started (MODE_RUST_OFF_PEAK=1)");
    Some(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_only_accepts_one() {
        assert!(enabled_from(Some("1")));
        assert!(enabled_from(Some(" 1 ")));
        assert!(!enabled_from(Some("true")));
        assert!(!enabled_from(Some("0")));
        assert!(!enabled_from(None));
    }
}
