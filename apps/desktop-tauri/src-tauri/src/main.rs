#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Mode 桌面端 Tauri v2 壳（路线图 M1）。
//!
//! 本阶段只做**平台命令面**：业务逻辑仍在既有 Node/TS 侧，经 sidecar（M3）接入。
//! 规格与验收：`docs/specs/tauri-platform-commands.md`。
//! 纪律：`docs/specs/clean-room-policy.md`（本目录全部适用）。
//!
//! ## 组装顺序是有效约束，不是随意排列
//!
//! 1. `deep-link` 插件必须**先于** `platform` 注册：`platform::commands::init` 会
//!    托管 `AppState`，而深链的路由状态在 `setup` 里托管——插件注册晚于此没有影响，
//!    但深链事件源（插件）必须先存在于 app 上，`deeplink::install` 才拿得到
//!    `deep_link()` 扩展。
//! 2. `DeeplinkState` 必须在 `deeplink::install` **之前** `manage`：install 里有两条路
//!    要用它（运行期的 `on_open_url` 回调、冷启动的 argv 解析）。
//! 3. 托盘最后装：它是附加入口，任何一步先失败都不应该连带它。
use tauri::Manager;

fn main() {
    let builder = tauri::Builder::default()
        // 更新链只声明端点与插件（M1 不注册更新命令，见 crates/updater 的模块文档）。
        .plugin(updater::plugin())
        .plugin(tauri_plugin_deep_link::init());

    let builder = platform::commands::init(builder);

    builder
        .setup(|app| {
            let handle = app.handle().clone();

            // 深链路由状态：冷启动队列的唯一所有者（见 crates/deeplink/src/router.rs）。
            app.manage(deeplink::DeepLinkRouter::new(deeplink::TauriSink::new(
                handle.clone(),
            )));

            deeplink::install(&handle)?;
            tray::install(&handle)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
