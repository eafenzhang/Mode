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
