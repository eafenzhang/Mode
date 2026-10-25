//! 窗口面：`notifyRendererReady` / `syncWindowTabs` / `syncWindowUnreadCount`。
//!
//! 三者都是「渲染层单向告知宿主」的命令：TS 侧返回 `void`，没有回值。
//! `notifyRendererReady` 除了记录就绪位，还要**回放冷启动深链**（见下）；
//! `syncWindowTabs` / `syncWindowUnreadCount` M1 只落到 [`crate::AppState`] 的占位快照
//! （所有者声明见该模块文档），M3 接上 sidecar 后由真正的所有者消费。
use tauri::{AppHandle, Manager, Runtime, State};

use crate::AppState;

/// `IPlatformService.notifyRendererReady(): void`
///
/// 原语义：通知 main process renderer 已就绪，触发缓存的冷启动 deep link 转发。
///
/// 本命令就是那条握手的宿主端：渲染层把订阅装好之后才调它，所以**冷启动缓存的深链
/// 在这里回放**（缓存与回放的时序在 `crates/deeplink` 的 router 里，有单元测试钉住）。
/// 深链状态尚未托管时不报错——「没有深链要补发」是正常情形，不是失败。
#[tauri::command(rename = "notifyRendererReady")]
pub fn notify_renderer_ready<R: Runtime>(app: AppHandle<R>, state: State<'_, AppState>) {
    let _ = state.mark_renderer_ready();
    if let Some(router) = app.try_state::<deeplink::DeeplinkState<R>>() {
        let flushed = router.mark_renderer_ready();
        if flushed > 0 {
            // 冷启动深链补发的条数是有用的可观测点：0 与非 0 对应完全不同的启动路径。
            eprintln!("[deeplink] renderer 就绪，补发积压深链 {flushed} 条");
        }
    }
}

/// `IPlatformService.syncWindowTabs(paths: string[]): void`
///
/// 原语义：把当前窗口所有 tab 的 workspace 路径同步给 main，用于跨窗口去重。
#[tauri::command(rename = "syncWindowTabs")]
pub fn sync_window_tabs(state: State<'_, AppState>, paths: Vec<String>) {
    let _ = state.set_tabs(paths);
}

/// `IPlatformService.syncWindowUnreadCount(count: number): void`
///
/// 原语义：把当前窗口未读 task 数同步给宿主，用于 Dock / 任务栏徽标聚合。
#[tauri::command(rename = "syncWindowUnreadCount")]
pub fn sync_window_unread_count(state: State<'_, AppState>, count: u32) {
    let _ = state.set_unread_count(count);
}

/// 窗口行为类设置的壳侧事实（渲染层经 `syncAppSettings` 推送，本状态是唯一落点）。
///
/// 为什么独立于 [`crate::AppState`]：关闭事件钩子（壳 `main.rs` 的 `on_window_event`）
/// 要**同步读**这个开关，AtomicBool 无锁取值；AppState 是 M1 的占位快照面，语义不同。
/// 缺省 `false` = 直接退出（现网行为 fail-safe）：渲染层还没同步过设置时，
/// 不因补丁迟到而擅自改变关闭语义。
pub struct WindowCloseBehavior(pub std::sync::atomic::AtomicBool);

impl Default for WindowCloseBehavior {
    fn default() -> Self {
        Self(std::sync::atomic::AtomicBool::new(false))
    }
}

/// `syncAppSettings` 的补丁入参：壳只消费它执行的窗口行为字段，
/// 其余设置键（provider/终端/通知…）归业务所有者（Host settings 服务），壳不解释。
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncAppSettingsPatch {
    /// 「关闭窗口时隐藏到托盘」（设置页仅在 Windows 呈现）；补丁缺键时保持现值。
    close_to_tray_on_windows: Option<bool>,
    /// 「保持电脑运行」（阻止空闲休眠）；补丁缺键时保持现值。
    keep_awake_while_running: Option<bool>,
}

/// `IPlatformService.syncAppSettings(patch: Partial<AppSettings>): void`
///
/// 渲染层单向告知壳窗口行为类设置的当前事实；壳不读业务存储
/// （AGENTS.md「进程、协议与远程控制」：窗口与进程调度属壳）。
/// 「保持电脑运行」的壳侧事实（同 WindowCloseBehavior，AtomicBool 供无锁读写）。
pub struct KeepAwakeBehavior(pub std::sync::atomic::AtomicBool);

impl Default for KeepAwakeBehavior {
    fn default() -> Self {
        Self(std::sync::atomic::AtomicBool::new(false))
    }
}

/// 把「保持电脑运行」落到 Windows 电源管理：`SetThreadExecutionState(ES_CONTINUOUS |
/// ES_SYSTEM_REQUIRED)` 阻止系统因空闲进入休眠（手动睡眠/合盖不受影响，与设置描述一致）。
/// 关闭时只保留 ES_CONTINUOUS（= 清除阻止）。非 Windows：no-op（设置页同门控只在 Windows 呈现）。
#[cfg(windows)]
fn apply_keep_awake(enabled: bool) {
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
    let flags = if enabled {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    // 返回值是上一次的执行状态（诊断用途）；失败返回 0，此处无事可做——
    // 下一次设置变更会再次尝试，不缓存失败。
    let _previous = unsafe { windows_sys::Win32::System::Power::SetThreadExecutionState(flags) };
}

#[cfg(not(windows))]
fn apply_keep_awake(_enabled: bool) {}

/// `IPlatformService.syncAppSettings(patch: Partial<AppSettings>): void`
///
/// 渲染层单向告知壳窗口行为类设置的当前事实；壳不读业务存储
/// （AGENTS.md「进程、协议与远程控制」：窗口与进程调度属壳）。
#[tauri::command(rename = "syncAppSettings")]
pub fn sync_app_settings(
    close_behavior: State<'_, WindowCloseBehavior>,
    keep_awake: State<'_, KeepAwakeBehavior>,
    patch: SyncAppSettingsPatch,
) {
    if let Some(enabled) = patch.close_to_tray_on_windows {
        close_behavior
            .0
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
    }
    if let Some(enabled) = patch.keep_awake_while_running {
        keep_awake
            .0
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
        apply_keep_awake(enabled);
    }
}
