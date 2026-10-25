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
//! 0. **单实例插件最先注册**（M3 Task 4）：插件 setup 钩子按注册顺序执行，而第二个进程的早退
//!    就在钩子里。注册在窗口/Host 之后等于没做（那时它们已经建出来了）。见
//!    `src/single_instance.rs`。
//! 1. `deep-link` 插件必须**先于** `platform` 注册：`platform::commands::init_with_extra`
//!    会托管 `AppState`，而深链的路由状态在 `setup` 里托管——插件注册晚于此没有影响，
//!    但深链事件源（插件）必须先存在于 app 上，`deeplink::install` 才拿得到
//!    `deep_link()` 扩展。
//! 2. 深链路由状态由 `deeplink::install` **自己**托管（它第一件事就是幂等的
//!    `deeplink::manage_state`），因为 install 里有两条路要用它（运行期的 `on_open_url` 回调、
//!    冷启动的 argv 解析）。**不再由本文件 `app.manage`**：两处都管会让「谁拥有它」变成两说
//!    （M3 Task 4 fix round 把早先那句「必须在 install 之前 manage」改掉了——那时确实是
//!    main.rs 自己管的，现在不是了）。
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
    // **单实例必须是第一个插件**（M3 Task 4 · DR-T4-1）：插件按注册顺序跑 setup 钩子，而第二个
    // 进程的早退就在钩子里——注册晚了它就会走完 app setup，建出第二个窗口与第二个 Host
    // （见 src/single_instance.rs 的模块文档）。
    let builder = mode_tauri::single_instance::install(tauri::Builder::default())
        // 更新链只声明端点与插件（M1 不注册更新命令，见 crates/updater 的模块文档）。
        .plugin(updater::plugin())
        .plugin(tauri_plugin_deep_link::init());

    // 切片表里没有的命令名交给壳侧的追加命令（Host 通道 + 窗口控制面，见头部注释的最后一段）。
    let builder = platform::commands::init_with_extra(builder, mode_tauri::extra_handler());

    let builder = builder.setup(|app| {
        let handle = app.handle().clone();

        // 深链面：托管路由状态（冷启动队列的唯一所有者，见 crates/deeplink/src/router.rs）+
        // 注册 scheme + 接线运行期回调与冷启动 argv。第二实例转发进来的 argv 也落到这一份
        // 路由上（`mode_tauri::single_instance`）。
        // 硬件加速设置必须在**首个 WebView 创建之前**生效（窗口建完再改就晚了）：
        // 读默认根 setting.json，desktopChromiumHardwareAccelerationEnabled === false 时
        // 注入 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--disable-gpu（显式 env 不覆盖）。
        if let Ok(home) = handle.path().home_dir() {
            mode_tauri::webview_settings::apply_webview_settings(&home);
        }
        deeplink::install(&handle)?;
        tray::install(&handle)?;
        // 更新状态机：检查/下载/安装分开，状态经 mode:update-state 推给渲染层的原版更新弹窗。
        app.manage(updater::state::UpdateManager::default());

        // Host sidecar：先起再建窗口（注入桥的 startupId 必须与它同代）。
        let host = mode_tauri::spawn_host(&handle);
        let startup_id = host.startup_id().to_string();
        eprintln!(
            "[mode-tauri] host startupId={startup_id} pid={}",
            host.pid()
        );
        app.manage(host);

        // 窗口必须在 Rust 侧建：`WindowConfig` 没有 `initialization_script` 字段，
        // 配置里的窗口拿不到注入（见 mode_tauri::window 的模块文档）。
        //
        // 入口页默认是**真产品 UI**（renderer 的构建产物 `dist/index.html`，M3 Task 3 接线）；
        // M1 的命令面验收台按 DR-T3-2 保留为开发/诊断入口，用
        // `MODE_TAURI_SHELL_ENTRY=dev/index.html` 进（挑选规则与白名单在
        // `sidecar_host::pick_shell_entry`，那边有用例覆盖）。
        // 入口页与**窗口形态**用的是同一个输入（`MODE_TAURI_SHELL_ENTRY`），所以两处判据
        // 不可能分叉：产品页（未覆写）→ 无边框（产品 UI 自绘标题栏）；开发/诊断入口与集成
        // 测试页 → 保留系统栏（那两页没有自绘标题栏，去掉会既移不动也关不掉）。
        //
        // 判据本身住在 `sidecar_host::decorations_for_shell_entry`（**纯函数、有用例**）：
        // 它在调用点写反过一次（`!entry_overridden`），症状就是用户报的「系统标题栏还在」。
        // 理由与拖动为什么仍然可用见 `mode_tauri::window` 的模块文档。
        let entry_override = std::env::var_os("MODE_TAURI_SHELL_ENTRY");
        let main_window = mode_tauri::build_host_window(
            &handle,
            mode_tauri::MAIN_WINDOW_LABEL,
            tauri::WebviewUrl::App(sidecar_host::pick_shell_entry(entry_override.as_deref())),
            "Mode",
            // 产品注入：`window.mode`（平台面适配层）+ Host 桥，一段脚本文本、document-start 求值。
            &sidecar_host::bridge::bridge_script(&startup_id),
            None,
            sidecar_host::decorations_for_shell_entry(entry_override.as_deref()),
        )?;

        // 关闭窗口时隐藏到托盘（Windows 设置 `closeToTrayOnWindows`，spec 命令表 `syncAppSettings`）。
        // 三条关闭路径——自绘标题栏关闭按钮（`closeWindow` → `window.close()`）、Alt+F4、
        // 任务栏/系统菜单——全部汇聚在 CloseRequested，在这一处统一拦截，不逐路分发。
        // 开关事实由渲染层经 `syncAppSettings` 推送（壳不读业务存储），缺省 false = 直接退出
        // （与补丁同步前的现网行为一致）。fail-safe：托盘不可用（部分 Linux 桌面降级）时
        // 即使开关为开也放行关闭——「隐藏到不存在的托盘」等于应用凭空消失。
        let close_window = main_window.clone();
        let event_handle = handle.clone();
        main_window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let hide_to_tray = event_handle
                    .state::<platform::commands::window::WindowCloseBehavior>()
                    .0
                    .load(std::sync::atomic::Ordering::SeqCst)
                    && event_handle.tray_by_id(tray::TRAY_ID).is_some();
                if hide_to_tray {
                    api.prevent_close();
                    let _ = close_window.hide();
                }
            }
        });

        // 更新演练用的确定性触发（M5 Task 1）：产品里的入口是「帮助 → 检查更新」，那需要人点；
        // 安装后的自动化演练没有可靠的点按面，所以提供一个显式环境变量，走**同一条**
        // `updater::command::check_and_install`。不设该变量时行为与之前完全一致。
        //
        // 注意：更新成功后 NSIS 以默认参数重启应用，环境变量会随安装器被继承；新版本会再做
        // 一次检查并得到 up-to-date，不会形成循环（见 RELEASE.md 的演练步骤）。
        if std::env::var_os("MODE_TAURI_CHECK_FOR_UPDATES").is_some() {
            eprintln!("[mode-tauri] updater startup trigger enabled");
            let updater_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                let outcome = updater::command::check_and_install(&updater_handle).await;
                eprintln!("[mode-tauri] updater startup trigger result={outcome}");
            });
        }
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
