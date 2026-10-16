//! 注入到 webview 的桥脚本：模板（`bridge.ts`）+ 占位符替换。
//!
//! 为什么脚本是 `.ts` 而不是 `.js`：语言政策只允许 Rust / TS / HTML / CSS 新增（2026-10-16 有过
//! 一次提交因新增 `.mjs` 被判红）。这个文件被 `include_str!` 当**文本**注入，里面的语法必须
//! 是纯 JS（不能有类型标注），所以它虽然叫 `.ts` 也不参与 `tsc` 的工程（不在任何 tsconfig 的 include 里）。
//!
//! 命令名从 Rust 常量注入而不是写死在脚本里：wire 名是 `#[tauri::command(rename = …)]` 的语义，
//! 只能有一处真源（`crate::commands::COMMAND_NAMES` 与它逐字一致，由 `commands.rs` 的守卫用例扫源码）。

/// 页面 → 壳（raw body）的命令名。
pub const CMD_SEND: &str = "host_send";
/// 壳 → 页面（长轮询 raw 响应）的命令名。
pub const CMD_RECV: &str = "host_recv";
/// 启动状态长轮询的命令名。
pub const CMD_STATE: &str = "host_startup_state";

/// 三个命令名（守卫用例与 CI 检查的清单）。
pub const COMMAND_NAMES: &[&str] = &[CMD_SEND, CMD_RECV, CMD_STATE];

/// 注入脚本模板（占位符见 `bridge.ts` 顶部注释）。
const TEMPLATE: &str = include_str!("bridge.ts");

/// `window.mode` 的注入内容（**拼在桥脚本之前**，见 `bridge_script`）。
///
/// 为什么单独一个文件：它与桥脚本是两件事（一个是平台面适配层，一个是端口/状态传输），
/// 而且 `oxlint` 的 `max-lines` 上限是 400——合在一起会顶到线。
/// **顺序不是排版问题**：renderer 入口模块求值期就读 `window.mode`（DR-T3-1），
/// 所以它必须排在前面，且两者同属**一个** `initialization_script`（document-start 求值）。
const WINDOW_MODE_TEMPLATE: &str = include_str!("window_mode.ts");

/// 把 startupId 与命令名填进模板，得到可直接交给 `initialization_script` 的脚本。
///
/// startupId 只允许 `[A-Za-z0-9_-]`：它来自壳生成的实例 id，但注入脚本是**字符串拼接**，
/// 不设防的话一个引号就能改脚本语义，所以这里做一次白名单化（不合法字符替换为 `-`）。
pub fn bridge_script(startup_id: &str) -> String {
    let safe_id: String = startup_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    // `window.mode` 在前：renderer 的**入口模块求值期**就会读它（见 `window_mode.ts` 的文件头）。
    // 两段拼成一段脚本而不是两次 `initialization_script`：同一段脚本文本的求值顺序是确定的，
    // 不依赖 Tauri 对多次注册的拼接顺序。
    format!("{WINDOW_MODE_TEMPLATE}\n{TEMPLATE}")
        .replace("__MODE_HOST_STARTUP_ID__", &safe_id)
        .replace("__MODE_HOST_CMD_SEND__", CMD_SEND)
        .replace("__MODE_HOST_CMD_RECV__", CMD_RECV)
        .replace("__MODE_HOST_CMD_STATE__", CMD_STATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_every_placeholder() {
        let script = bridge_script("host-1234-1");
        for placeholder in [
            "__MODE_HOST_STARTUP_ID__",
            "__MODE_HOST_CMD_SEND__",
            "__MODE_HOST_CMD_RECV__",
            "__MODE_HOST_CMD_STATE__",
        ] {
            assert!(
                !script.contains(placeholder),
                "注入脚本里不得残留占位符 {placeholder}（残留会让页面拿到字面量命令名，invoke 必然 not found）"
            );
        }
        for name in COMMAND_NAMES {
            assert!(script.contains(&format!("\"{name}\"")), "脚本里应带上命令名 {name}");
        }
        assert!(script.contains("\"host-1234-1\""));
        // 频道名是字面量：跨语言一致性由 packages/desktop/tests/host-standalone.test.mts 守住。
        assert!(script.contains("\"mode:service-port\""));
        assert!(script.contains("\"mode:database-startup-state\""));
        assert!(
            script.contains("new Uint8Array(buffer)"),
            "入方向必须把 raw 响应的 ArrayBuffer 包成 Uint8Array（spike §4.3 的静默丢帧陷阱）"
        );
        assert!(
            script.contains("DOMContentLoaded"),
            "投递必须推迟到渲染层的 message 监听注册之后（spike §4.5）"
        );
    }

    #[test]
    fn sanitizes_startup_id_before_injection() {
        let script = bridge_script("bad\"; window.__pwned = 1; var x = \"");
        assert!(
            !script.contains("window.__pwned"),
            "注入脚本是字符串拼接：startupId 必须白名单化，否则一个引号就能改脚本语义"
        );
        // 精确断言：`var STARTUP_ID = "…"` 里那段字面量只剩白名单字符。
        let prefix = "var STARTUP_ID = \"";
        let start = script.find(prefix).expect("模板里有 STARTUP_ID 赋值") + prefix.len();
        let end = script[start..].find('"').expect("字面量有结束引号") + start;
        let injected = &script[start..end];
        assert!(
            injected
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_'),
            "注入的 id 只允许 [A-Za-z0-9_-]，实际：{injected}"
        );
    }

    /// `window.mode` 的注入必须排在端口桥之前：renderer 的**入口模块求值期**就会读它
    /// （DR-T3-1 的顺序性质）。这条断言只守「脚本文本内部的顺序」，端到端的行为断言在
    /// `apps/desktop-tauri/test/windowModeInjection.test.mts`（真跑脚本、真读探针）。
    #[test]
    fn window_mode_is_installed_before_the_rest_of_the_bridge() {
        let script = bridge_script("host-sequencing");
        let mode_assignment = script
            .find("window.mode = mode")
            .expect("注入脚本必须赋值 window.mode（renderer 入口模块求值期就读它）");
        let deferred_delivery = script
            .find("DOMContentLoaded")
            .expect("桥脚本把投递推迟到 DOMContentLoaded（spike §4.5）");
        assert!(
            mode_assignment < deferred_delivery,
            "window.mode 必须在任何延迟点之前就位：赋值在下标 {mode_assignment}，延迟点在下标 {deferred_delivery}"
        );
        // 拼接顺序的负向守卫：只把桥脚本注入进去（没有 window_mode 模板）必须被这条断言抓住。
        let bridge_only = TEMPLATE.to_string();
        assert!(
            !bridge_only.contains("window.mode = mode"),
            "桥脚本自己不得提供 window.mode（否则这条顺序断言是空的）"
        );
    }
}
