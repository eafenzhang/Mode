//! M3 Task 3 · Step 4 的 **opt-in 真 webview harness**（**测试资产**，不是产品入口）。
//!
//! 与 Task 1 的 `HostRoundtripHarness` 是**同一套产品部件**（`mode_tauri::spawn_host` +
//! `mode_tauri::build_host_window`），差别只有两处，而这两处正是本任务要验的东西：
//!
//! 1. 加载的是 **`index.html`**——真 renderer 的构建产物（`frontendDist` 装配后的产品页），
//!    不是集成测试页；
//! 2. **追加一段探针**注入脚本（`test/harness/product-ui/probe.ts`）。页面不能改
//!    （renderer 源码零改动是硬约束），所以观测只能从注入做。
//!
//! ```text
//! node scripts/rust-cargo.ts build -p mode-tauri --features product-ui-harness --bin ProductUiHarness
//! MODE_TAURI_PRODUCT_UI=1 node --import tsx --test apps/desktop-tauri/test/productUiMount.test.mts
//! ```
//! 通常不用手敲：`productUiMount.test.mts` 会自己装配前端、构建本 bin、起 sink 并断言结果。

use std::time::Duration;

use tauri::Manager;

/// 探针脚本（测试资产；**注入**而不是改页面，见文件头）。
const PROBE_SCRIPT: &str = include_str!("../../../test/harness/product-ui/probe.ts");

fn watchdog_ms() -> u64 {
    std::env::var("MODE_PRODUCT_UI_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60_000)
}

fn main() {
    let builder = tauri::Builder::default();
    let builder = platform::commands::init_with_extra(builder, mode_tauri::host_handler());

    let builder = builder.setup(|app| {
        let handle = app.handle().clone();

        let host = mode_tauri::spawn_host(&handle);
        let startup_id = host.startup_id().to_string();
        println!("HARNESS host startupId={startup_id} pid={}", host.pid());
        app.manage(host);

        // 与产品壳**同一段注入**（`bridge_script` 内含 `window.mode`），入口页也取产品默认值
        // （`pick_shell_entry` 在未覆写时返回 `index.html`）。
        mode_tauri::build_host_window(
            &handle,
            mode_tauri::MAIN_WINDOW_LABEL,
            tauri::WebviewUrl::App(sidecar_host::pick_shell_entry(None)),
            "M3 Task 3 · product UI",
            &startup_id,
            Some(PROBE_SCRIPT),
        )?;

        let exit_handle = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(watchdog_ms()));
            println!("HARNESS watchdog_exit");
            let _ = exit_handle.exit(0);
        });
        Ok(())
    });

    let app = builder
        .build(tauri::generate_context!())
        .expect("harness 壳构建失败");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(host) = app_handle.try_state::<sidecar_host::HostHandle>() {
                let _ = host.shutdown();
                println!("HARNESS host_shutdown pid={}", host.pid());
            }
        }
    });
}
