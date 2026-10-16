#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Mode 桌面端 Tauri v2 壳（路线图 M1 平台命令面 + M3 Task 1 的 Host 通道）。
//!
//! M1 只做**平台命令面**；M3 Task 1 起，壳还要负责把 Host sidecar 拉起来，并把
//! 「端口 + database-startup 状态流」送进 webview（渲染层的启动闸门据此放行）。
//! 规格：`docs/specs/tauri-platform-commands.md`（命令面）、
//! `docs/specs/tauri-renderer-host.md`（Host 契约）。纪律：`docs/specs/clean-room-policy.md`。
//!
//! ## 组装顺序是有效约束，不是随意排列
//!
//! 1. `deep-link` 插件必须**先于** `platform` 注册：`platform::commands::init_with_extra`
//!    会托管 `AppState`，而深链的路由状态在 `setup` 里托管——插件注册晚于此没有影响，
//!    但深链事件源（插件）必须先存在于 app 上，`deeplink::install` 才拿得到
//!    `deep_link()` 扩展。
//! 2. `DeepLinkRouter` 必须在 `deeplink::install` **之前** `manage`：install 里有两条路
//!    要用它（运行期的 `on_open_url` 回调、冷启动的 argv 解析）。
//! 3. 托盘最后装：它是附加入口，任何一步先失败都不应该连带它。
//! 4. **Host 先起、窗口后建**（M3 Task 1）：窗口的注入桥要拿 `startupId`，而 startupId 是
//!    起 Host 时生成的；先建窗口就会拿到一个没有对应 Host 的 id。
//! 5. 退出时收 Host：`RunEvent::Exit` 里 `shutdown()`（不留孤儿，DR-T1-3）。
//!
//! ## 命令面为什么只有一个 `invoke_handler`
//!
//! `tauri::Builder::invoke_handler` 是**覆盖**语义（`tauri-2.12.4/src/app.rs:1727`），
//! 两个 crate 各调一次会静默丢掉一批命令（M1 的 smoke 吃过「10 条命令全部 Command not found」的亏）。
//! 所以 M1 的命令面与 M3 的 Host 命令经 `platform::commands::init_with_extra` 合成一次注册。
use tauri::Manager;

fn main() {
    let builder = tauri::Builder::default()
        // 更新链只声明端点与插件（M1 不注册更新命令，见 crates/updater 的模块文档）。
        .plugin(updater::plugin())
        .plugin(tauri_plugin_deep_link::init());

    // 切片表里没有的命令名交给壳的 Host 命令（见头部注释的最后一段）。
    let builder = platform::commands::init_with_extra(builder, mode_tauri::host_handler());

    let builder = builder.setup(|app| {
        let handle = app.handle().clone();

        // 深链路由状态：冷启动队列的唯一所有者（见 crates/deeplink/src/router.rs）。
        app.manage(deeplink::DeepLinkRouter::new(deeplink::TauriSink::new(
            handle.clone(),
        )));

        deeplink::install(&handle)?;
        tray::install(&handle)?;

        // Host sidecar：先起再建窗口（注入桥的 startupId 必须与它同代）。
        let host = mode_tauri::spawn_host(&handle);
        let startup_id = host.startup_id().to_string();
        eprintln!("[mode-tauri] host startupId={startup_id} pid={}", host.pid());
        app.manage(host);

        // 窗口必须在 Rust 侧建：`WindowConfig` 没有 `initialization_script` 字段，
        // 配置里的窗口拿不到注入（见 mode_tauri::window 的模块文档）。
        mode_tauri::build_host_window(
            &handle,
            mode_tauri::MAIN_WINDOW_LABEL,
            tauri::WebviewUrl::App(std::path::PathBuf::from("index.html")),
            "Mode",
            &startup_id,
        )?;
        Ok(())
    });

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            // 壳正常退出 → 收掉 sidecar（不留孤儿）。被强杀时靠 Host 自己的 stdin EOF 保护兜底，
            // 见 packages/desktop/src/host-standalone/index.ts。
            if let Some(host) = app_handle.try_state::<sidecar_host::HostHandle>() {
                let _ = host.shutdown();
            }
        }
    });
}
