//! 窗口面：`notifyRendererReady` / `syncWindowTabs` / `syncWindowUnreadCount`。
//!
//! 这三个都是「渲染层单向告知宿主」的命令：TS 侧返回 `void`，没有回值。
//! M1 只落到 [`crate::AppState`] 的占位快照（所有者声明见该模块文档）；
//! M3 接上 sidecar 后由真正的所有者消费。
use tauri::State;

use crate::AppState;

/// `IPlatformService.notifyRendererReady(): void`
///
/// 原语义：通知 main process renderer 已就绪，触发缓存的冷启动 deep link 转发。
/// M1 的 deep link 转发在 Task 6 接线，这里先记录就绪位。
#[tauri::command]
pub fn notify_renderer_ready(state: State<'_, AppState>) {
    let _ = state.mark_renderer_ready();
}

/// `IPlatformService.syncWindowTabs(paths: string[]): void`
///
/// 原语义：把当前窗口所有 tab 的 workspace 路径同步给 main，用于跨窗口去重。
#[tauri::command]
pub fn sync_window_tabs(state: State<'_, AppState>, paths: Vec<String>) {
    let _ = state.set_tabs(paths);
}

/// `IPlatformService.syncWindowUnreadCount(count: number): void`
///
/// 原语义：把当前窗口未读 task 数同步给宿主，用于 Dock / 任务栏徽标聚合。
#[tauri::command]
pub fn sync_window_unread_count(state: State<'_, AppState>, count: u32) {
    let _ = state.set_unread_count(count);
}
