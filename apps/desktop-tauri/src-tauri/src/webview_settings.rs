//! 设置项里需要在 **WebView 创建之前**生效的部分（当前：硬件加速开关）。
//!
//! WebView2 的浏览器参数（如 `--disable-gpu`）只能在 WebView2 环境**创建时**读取——
//! 窗口一旦建出来再改就晚了。壳不读业务存储（AGENTS.md 的进程分工），但「默认根
//! setting.json」（`~/.mode/v2/setting.json`，恒在用户主目录、不随数据目录迁移，
//! 见 `docs/specs/tauri-renderer-host.md` §23）是 Host 与壳共知的引导文件，
//! 这里在 `setup` 最早期同步读它，只取自己要执行的那一个键。
//!
//! 判定纯函数在 `sidecar_host::settings_probe`（那边测试目标本地可跑）；本模块只做
//! 「读文件 → 按需设环境变量」。传输：`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 是
//! WebView2 官方指定的额外参数注入点。**已显式设置时尊重现状不覆盖**——开发/测试
//! 环境用它注入远程调试等参数，静默覆盖会让那套流程失灵。

use std::path::Path;

/// 在首个 WebView 创建之前应用硬件加速设置。任何失败都静默保持默认（硬件加速开启）。
pub fn apply_webview_settings(home: &Path) {
    let Ok(raw) = std::fs::read_to_string(
        sidecar_host::settings_probe::default_root_settings_file(home),
    ) else {
        return;
    };
    if !sidecar_host::settings_probe::gpu_disabled_from_settings(&raw) {
        return;
    }
    if std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_some() {
        // 显式 env 优先（开发/测试注入远程调试等参数时不被覆盖）。
        eprintln!("[mode-tauri] WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS 已设置，硬件加速设置不覆盖");
        return;
    }
    const ARGS: &str = "--disable-gpu";
    std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", ARGS);
    eprintln!("[mode-tauri] 按设置关闭 WebView2 硬件加速（{ARGS}）");
}
