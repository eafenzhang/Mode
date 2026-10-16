//! M1 平台命令面的注册表与实现（规格：`docs/specs/tauri-platform-commands.md`）。
//!
//! 切片表是**唯一真源**：命令名（JS 侧 `invoke` 用的名字）与处理函数在同一个宏调用里
//! 成对给出，`init()` 用它注册 handler，[`registered_names()`] 用它对外报名字。
//! 这样「注册的命令」与「适配层会调的名字」不会各写一份而漂移。

pub mod dialog;
pub mod icon;
pub mod notify;
pub mod shell;
pub mod window;

/// 生成切片表、handler 注册与名字上报。
///
/// `$js => $handler` 里的 `$js` 必须等于 `$handler` 函数名的 camelCase 形式，
/// **并且每个处理函数自己也要写 `#[tauri::command(rename = "<同一个 $js>")]`**。
///
/// 后半句不是冗余：Tauri v2 的命令名默认是**函数名的 snake_case 原文**
/// （`tauri-macros` 的 `command/wrapper.rs`：无 rename 时取 `stringify!(ident)`），
/// 不会自动转 camelCase。少了 rename，渲染层的 `invoke("openExternal")`
/// 命不中 `open_external` 分支，运行时才报 `Command openExternal not found`——
/// 2026-10-16 的 smoke 就是被这个静默断链卡住的（10 条命令全部不可达）。
/// `tests/commands.rs` 的 `every_command_declares_a_camel_case_wire_name` 扫源码守住这条。
macro_rules! command_slice {
    ($($js:literal => $handler:path),+ $(,)?) => {
        /// M1 切片表里的命令名，与 `IPlatformService` 逐字一致。
        pub const M1_COMMANDS: &[&str] = &[$($js),+];

        /// 处理函数的 Rust 路径，供守卫用例核对 camelCase 映射。
        pub const M1_HANDLER_PATHS: &[&str] = &[$(stringify!($handler)),+];

        /// 注册插件、托管状态与切片表里的全部命令。
        pub fn init<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
            builder
                .plugin(tauri_plugin_dialog::init())
                .plugin(tauri_plugin_notification::init())
                .plugin(tauri_plugin_opener::init())
                .manage(crate::AppState::default())
                .invoke_handler(tauri::generate_handler![$($handler),+])
        }
    };
}

/// 切片表里的命令名。
pub fn registered_names() -> &'static [&'static str] {
    M1_COMMANDS
}

command_slice! {
    "openExternal" => shell::open_external,
    "selectDirectory" => dialog::select_directory,
    "selectFile" => dialog::select_file,
    "saveFile" => dialog::save_file,
    "openInFileManager" => shell::open_in_file_manager,
    "showTaskNotification" => notify::show_task_notification,
    "notifyRendererReady" => window::notify_renderer_ready,
    "syncWindowTabs" => window::sync_window_tabs,
    "syncWindowUnreadCount" => window::sync_window_unread_count,
    "getApplicationIcon" => icon::get_application_icon,
}
