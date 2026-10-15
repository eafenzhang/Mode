#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Mode 桌面端 Tauri v2 壳（路线图 M1）。
//!
//! 本阶段只做**平台命令面**：业务逻辑仍在既有 Node/TS 侧，经 sidecar（M3）接入。
//! 规格与验收：`docs/specs/tauri-platform-commands.md`。
//! 纪律：`docs/specs/clean-room-policy.md`（本目录全部适用）。

fn main() {
    let builder = tauri::Builder::default();
    let builder = platform::commands::init(builder);
    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
