//! M1 平台命令面的注册表与实现（规格：`docs/specs/tauri-platform-commands.md`）。
//!
//! 切片表是**唯一真源**：命令名（JS 侧 `invoke` 用的名字）与处理函数在同一个宏调用里
//! 成对给出，`init()` 用它注册 handler，[`registered_names()`] 用它对外报名字。
//! 这样「注册的命令」与「适配层会调的名字」不会各写一份而漂移。

/// LR-3：远程桌面剪贴板读取（壳能力，Host 不碰平台 API）。
pub mod clipboard;
pub mod dialog;
pub mod icon;
pub mod notify;
pub mod shell;
pub mod temp_attachment;
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

        /// 切片表的 handler 值：调用方（壳）用它把额外命令并进**同一个** `invoke_handler`。
        pub fn handler<R: tauri::Runtime>(
        ) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
            tauri::generate_handler![$($handler),+]
        }

        /// 注册插件、托管状态与切片表里的全部命令（= `init_with_extra(builder, |_| false)`）。
        pub fn init<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
            init_with_extra(builder, |_| false)
        }

        /// 同 [`init`]，但把切片表**未命中**的命令名交给 `extra`（M3 的壳侧 Host 命令走这里）。
        ///
        /// 追加命令只能从这个入口进：`tauri::Builder::invoke_handler` 是**覆盖**语义
        /// （`tauri-2.12.4/src/app.rs:1727` 直接赋值），两个 crate 各调一次会静默丢掉先注册的那批；
        /// 而 `generate_handler!` 生成的闭包吃 `Invoke<R>` 的**所有权**，没法把它们串起来调两次。
        /// 所以这里按命令名派发：先认切片表，命中不了再交给 `extra`。
        ///
        /// 注意 `M1_COMMANDS` 仍是切片表的唯一真源——`extra` 里再写一个同名命令会被切片表吃掉。
        pub fn init_with_extra<R, F>(builder: tauri::Builder<R>, extra: F) -> tauri::Builder<R>
        where
            R: tauri::Runtime,
            F: Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
        {
            let slice = handler::<R>();
            builder
                .plugin(tauri_plugin_dialog::init())
                .plugin(tauri_plugin_notification::init())
                .plugin(tauri_plugin_opener::init())
                .manage(crate::AppState::default())
                .invoke_handler(move |invoke| {
                    if M1_COMMANDS.contains(&invoke.message.command()) {
                        slice(invoke)
                    } else {
                        extra(invoke)
                    }
                })
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
    // 长粘贴文本 → 临时附件（Tauri 线真实实现；Electron main IPC 已随旧线 EOL 删除）。
    "createTempTextAttachment" => temp_attachment::create_temp_text_attachment,
    "openInFileManager" => shell::open_in_file_manager,
    // 用系统默认应用打开本地文件（目录仍走 openInFileManager）。
    "openExternalFile" => shell::open_external_file,
    "showTaskNotification" => notify::show_task_notification,
    "notifyRendererReady" => window::notify_renderer_ready,
    "syncWindowTabs" => window::sync_window_tabs,
    "syncWindowUnreadCount" => window::sync_window_unread_count,
    "getApplicationIcon" => icon::get_application_icon,
    // LR-3：远程桌面主控剪贴板同步的读取口（壳能力）。
    "rdReadClipboardText" => clipboard::rd_read_clipboard_text,
}
