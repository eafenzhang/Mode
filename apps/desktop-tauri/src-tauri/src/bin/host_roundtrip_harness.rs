//! M3 Task 1 的 **opt-in 真 webview 集成 harness**（**测试资产**，不是产品入口）。
//!
//! 它只做测试需要的三件事，产品壳（`src/main.rs`）与它共用同一套部件：
//!
//! 1. 用**产品接线**起 Host（`mode_tauri::spawn_host`：真 sidecar 入口、真桥脚本、真 startupId）；
//! 2. 用**产品窗口装配**（`mode_tauri::build_host_window`）加载集成测试页
//!    （`harness/host-sidecar/index.html`，由 `test/harness/host-sidecar/vite.config.ts` 产出）；
//! 3. 看门狗到点后 `app.exit(0)`，让 `RunEvent::Exit` 的 Host 收摊跑一遍——测试据此断言
//!    「壳正常退出后不留孤儿」。
//!
//! 什么时候跑：**发版前人工跑**（CI 没有真 webview，见 `docs/specs/tauri-renderer-host.md`）。
//! ```text
//! node scripts/rust-cargo.ts build -p mode-tauri --features host-roundtrip --bin HostRoundtripHarness
//! node --test apps/desktop-tauri/test/webviewRoundTrip.test.mts   # 需要 MODE_TAURI_WEBVIEW_ROUNDTRIP=1
//! ```
//! 通常不用手敲：`apps/desktop-tauri/test/webviewRoundTrip.test.mts` 会自己构建页面与 bin、
//! 起日志 sink、拉起本 bin 并断言结果。

use std::time::Duration;

use tauri::Manager;

fn watchdog_ms() -> u64 {
    std::env::var("MODE_HOST_ROUNDTRIP_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(25_000)
}

fn main() {
    let builder = tauri::Builder::default();
    let builder = platform::commands::init_with_extra(builder, mode_tauri::host_handler());

    let builder = builder.setup(|app| {
        let handle = app.handle().clone();

        let host = mode_tauri::spawn_host(&handle);
        let startup_id = host.startup_id().to_string();
        println!("HARNESS host startupId={startup_id} pid={} dataRootHint=见 host.describe", host.pid());
        app.manage(host);

        mode_tauri::build_host_window(
            &handle,
            // label 用产品的 `main`：capabilities/default.json 只覆盖这个 label。
            mode_tauri::MAIN_WINDOW_LABEL,
            tauri::WebviewUrl::App(std::path::PathBuf::from("harness/host-sidecar/index.html")),
            "M3 Task 1 · host round trip",
            &startup_id,
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
