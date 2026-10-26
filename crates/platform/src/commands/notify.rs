//! 通知面：`showTaskNotification`。
use protocol::TaskNotificationPayload;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// `IPlatformService.showTaskNotification(payload: TaskNotificationPayload): void`
///
/// 是否真正展示由宿主环境决定（接口注释原文：宿主环境决定是否真正展示）。
/// M1 一律交给系统通知插件；按 `status` 分流（例如权限请求走常驻通知）属产品规则，
/// 需要在 spec 里先定规则再实现，不在骨架期自行发明。
#[tauri::command(rename = "showTaskNotification")]
pub fn show_task_notification<R: tauri::Runtime>(
    app: AppHandle<R>,
    payload: TaskNotificationPayload,
) -> Result<(), String> {
    let title = payload.title.clone();
    let body = payload.body.clone();
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|error| error.to_string())
}

/// 注册「点击通知 → 聚焦」处理器（spec 命令表第 8 行的点击行为，2026-10-26 落地）。
///
/// 必须先于任何通知发出**注册一次**（插件口径：首个 handler 之前展示的通知不上报；
/// handler 进插件的全局表，重复注册会累积）。聚焦动作由调用方注入——窗口标签是壳的
/// 知识，platform 不持有；handler 在后台线程执行，聚焦实现需自行保证线程安全（Tauri
/// 的窗口方法内部走事件循环派发）。
///
/// Windows 口径：toast **在屏时**点击才上报，进了操作中心（Action Center）就不报。
pub fn register_click_to_focus<R: tauri::Runtime>(
    app: &AppHandle<R>,
    focus: impl Fn() + Send + Sync + 'static,
) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .on_action(move |_performed| focus())
        .map_err(|error| error.to_string())
}
