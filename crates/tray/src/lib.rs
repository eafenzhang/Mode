//! 托盘（路线图 M1 任务 6 的最小可用面）。
//!
//! ## 本阶段只做「能看见、能点」
//!
//! 两个菜单项：显示主窗口、退出。不做未读徽标、不做动态菜单、不做点击折叠——
//! 那些都依赖 M2/M3 的任务状态真源（`AGENTS.md`：托盘不是业务状态的所有者）。
//!
//! ## 失败不致命
//!
//! 托盘在部分 Linux 桌面环境需要 `libayatana-appindicator`，缺失时 `build` 失败。
//! 托盘是**附加**入口（主窗口才是唯一必需入口），所以这里把失败降级成一条 stderr，
//! 不阻断应用启动。
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

/// 菜单项 id：显示主窗口。
pub const MENU_ID_SHOW: &str = "mode-tray-show";
/// 菜单项 id：退出应用。
pub const MENU_ID_QUIT: &str = "mode-tray-quit";

/// 主窗口 label（与 `tauri.conf.json` 的 `app.windows[].label` 一致）。
const MAIN_WINDOW_LABEL: &str = "main";

/// 安装托盘。调用点在 `setup`。
pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_ID_SHOW, "显示主窗口", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, MENU_ID_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &separator, &quit])?;

    let mut builder = TrayIconBuilder::new()
        .tooltip("Mode")
        .menu(&menu)
        // 左键点击图标时**不**弹菜单：主窗口才是默认动作，弹菜单会让单击变成两步。
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_ID_SHOW => show_main_window(app),
            MENU_ID_QUIT => app.exit(0),
            _ => {}
        });

    // 用应用自己的窗口图标，避免托盘出现一个与产品无关的默认图标。
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    if let Err(error) = builder.build(app) {
        eprintln!("[tray] 托盘不可用，已降级为仅主窗口入口：{error}");
    }
    Ok(())
}

/// 显示并聚焦主窗口。窗口被关闭（`close` 而非销毁）时用 `show` 恢复。
fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_are_distinct_and_stable() {
        // id 是事件分派的**唯一**依据（`on_menu_event` 里按字符串匹配），
        // 写错或写重会让某个菜单项变成静默无效。这里把两个 id 钉住。
        assert_ne!(MENU_ID_SHOW, MENU_ID_QUIT);
        assert_eq!(MENU_ID_SHOW, "mode-tray-show");
        assert_eq!(MENU_ID_QUIT, "mode-tray-quit");
        // 主窗口 label 必须与 tauri.conf.json 一致，否则「显示主窗口」找不到窗口。
        assert_eq!(MAIN_WINDOW_LABEL, "main");
    }
}
