//! Automation 宿主进壳（G2.5 开关化）：`MODE_RUST_AUTOMATION=1` 时在 Tauri 进程内
//! 启动 Rust `AutomationRuntime`。
//!
//! 所有权约束：Node sidecar 侧 `serviceAccessor` 在同一开关下整个不创建
//! `automationRuntime`（不是只不 start）；两边必须同开关，否则会双派发。
//! 默认关闭——未设置时行为与现状完全一致。

use std::path::Path;
use std::sync::Arc;

use host_channels::automation_host::{create_automation_host, AutomationHostOptions};
use host_channels::model_selection_port::ProviderConfigModelSelectionPort;
use host_channels::ModeTaskChannel;
use rpc_host::ServerChannel;
use task_index_repo::TaskIndexRepo;

use crate::inprocess::InprocessState;

pub const ENV_RUST_AUTOMATION: &str = "MODE_RUST_AUTOMATION";

fn enabled_from(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("1")
}

/// 按开关装配并启动 automation 宿主；未启用/依赖缺失时返回 `None`（留痕）。
pub fn start_if_enabled(
    resources_dir: Option<&Path>,
    data_root: &Path,
    inprocess: &InprocessState,
) -> Option<Arc<host_channels::automation_host::AutomationHost>> {
    if !enabled_from(std::env::var(ENV_RUST_AUTOMATION).ok().as_deref()) {
        return None;
    }
    let cwd = std::env::current_dir().ok()?;
    let Some((bundled, personal)) =
        crate::agent_runtime::resolve_provider_snapshot_paths(resources_dir, data_root, &cwd)
    else {
        crate::host::shell_log(
            "automation host disabled: provider built-in/personal config paths unavailable",
        );
        return None;
    };
    let snapshot = Arc::new(move || {
        provider_config_source::read_snapshot(&bundled, &personal, None)
            .map_err(|error| error.to_string())
    });
    let model_selection = Arc::new(ProviderConfigModelSelectionPort::new(snapshot));

    let db_path = data_root.join("v2").join("tasks-index.sqlite");
    let repo = Arc::new(TaskIndexRepo::new(db_path.clone()));
    let task_events = Arc::clone(inprocess.task_events());
    let mode_task: Arc<dyn ServerChannel> = Arc::new(
        ModeTaskChannel::new(
            repo,
            Some(Arc::clone(inprocess.agent_control())),
            None,
            Arc::clone(&task_events),
        )
        .with_data_root(data_root.to_path_buf()),
    );
    let host = create_automation_host(
        AutomationHostOptions {
            db_path,
            mode_task,
            task_events,
            utc_offset_secs: host_channels::mode_config_options::local_utc_offset_secs(),
            // 与 TS `POLL_INTERVAL_MS` / `automation_runtime::DEFAULT_POLL_INTERVAL_MS` 同值。
            poll_interval_ms: 20_000,
            log: Arc::new(|level, message| {
                crate::host::shell_log(&format!("automation {level}: {message}"))
            }),
        },
        model_selection,
    );
    host.runtime.start();
    crate::host::shell_log("automation host started (MODE_RUST_AUTOMATION=1)");
    Some(Arc::new(host))
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
