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
