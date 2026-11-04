//! keep-awake（`keepAwakeWhileRunning`）：存在执行中的闲时任务时阻止系统空闲休眠。
//!
//! spec 口径（`packages/shared/src/protocol.ts`）：手动开关、只防闲置休眠、防不了合盖；
//! Electron 线的 `powerSaveBlocker` 在 Tauri 线没有对应实现，这里补上壳侧唯一所有者：
//!
//! - 设置来源：`<dataBaseDir>/.mode/v2/setting.json` 顶层 `keepAwakeWhileRunning`；
//! - 运行中判据：任务库 `off_peak_tasks` 里 `status='running'` 的行数 > 0
//!   （与 TS `OffPeakTaskRepo` 执行中计数同口径）；
//! - 5 秒轮询（电源开关不是高频状态，轮询比引入 watcher 依赖更省）；
//! - Windows 用 `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`，
//!   其余平台为 no-op（保持跨平台可编译）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// 决策：只有开关打开且确有执行中的闲时任务时才阻止休眠。
fn should_keep_awake(setting_enabled: bool, running_tasks: i64) -> bool {
    setting_enabled && running_tasks > 0
}

#[cfg(windows)]
fn set_execution_state(keep_awake: bool) {
    #[link(name = "kernel32")]
    extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    let flags = if keep_awake {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    // 失败不影响业务：keep-awake 是尽力而为的电源提示。
    unsafe {
        SetThreadExecutionState(flags);
    }
}

#[cfg(not(windows))]
fn set_execution_state(_keep_awake: bool) {}

/// 幂等的电源状态持有者：值不变不重复调用系统 API。
#[derive(Default)]
pub struct KeepAwake {
    active: AtomicBool,
}

impl KeepAwake {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn apply(&self, keep_awake: bool) {
        if self.active.swap(keep_awake, Ordering::SeqCst) == keep_awake {
            return;
        }
        set_execution_state(keep_awake);
    }
}

fn setting_path(data_base_dir: &Path) -> PathBuf {
    data_base_dir.join(".mode").join("v2").join("setting.json")
}

fn tasks_db_path(data_base_dir: &Path) -> PathBuf {
    data_base_dir
        .join(".mode")
        .join("v2")
        .join("tasks-index.sqlite")
}

fn read_setting_enabled(data_base_dir: &Path) -> bool {
    let path = setting_path(data_base_dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value
        .get("keepAwakeWhileRunning")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn read_running_off_peak_tasks(data_base_dir: &Path) -> i64 {
    let repo =
        off_peak_repo::OffPeakTaskRepo::with_busy_timeout(tasks_db_path(data_base_dir), 2_000);
    repo.count_active().unwrap_or(0)
}

/// 启动轮询线程（调用一次；`data_base_dir` 与 Host 的 dataBaseDir 同口径）。
pub fn start_watcher(data_base_dir: PathBuf, state: Arc<KeepAwake>) {
    std::thread::spawn(move || loop {
        let enabled = read_setting_enabled(&data_base_dir);
        let running = if enabled {
            read_running_off_peak_tasks(&data_base_dir)
        } else {
            0
        };
        state.apply(should_keep_awake(enabled, running));
        std::thread::sleep(POLL_INTERVAL);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_requires_setting_and_running_tasks() {
        assert!(!should_keep_awake(false, 3));
        assert!(!should_keep_awake(true, 0));
        assert!(should_keep_awake(true, 1));
    }

    #[test]
    fn setting_read_defaults_to_false_on_missing_or_bad_json() {
        let dir = std::env::temp_dir().join(format!(
            "mode-power-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join(".mode").join("v2")).expect("mkdir");
        assert!(!read_setting_enabled(&dir));
        std::fs::write(setting_path(&dir), "{bad json").expect("write");
        assert!(!read_setting_enabled(&dir));
        std::fs::write(setting_path(&dir), "{\"keepAwakeWhileRunning\":true}").expect("write");
        assert!(read_setting_enabled(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
