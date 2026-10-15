//! M1 平台命令面的断言。规格：`docs/specs/tauri-platform-commands.md`（命令切片表）。
//!
//! 命令名必须与 `IPlatformService`（`packages/shared/src/platform.ts`）逐字一致 —— 渲染层
//! 的 `invoke()` 适配层按同名映射，改名即断链。
//!
//! 本文件必须留在 `tests/`（集成测试目标）：`crates/platform/build.rs` 只对**测试目标**
//! 注入 comctl32 v6 清单（`rustc-link-arg-tests`），而 cargo 只把 `tests/` 下的目标算作
//! 测试目标；放进 lib 单测则拿不到清单，进程启动即 `STATUS_ENTRYPOINT_NOT_FOUND`。
use platform::commands::{registered_names, M1_HANDLER_PATHS};

/// 切片表里的 10 个命令，顺序与 spec 表格一致。
const M1_SLICE: [&str; 10] = [
    "openExternal",
    "selectDirectory",
    "selectFile",
    "saveFile",
    "openInFileManager",
    "showTaskNotification",
    "notifyRendererReady",
    "syncWindowTabs",
    "syncWindowUnreadCount",
    "getApplicationIcon",
];

#[test]
fn registers_exactly_the_m1_slice() {
    let names = registered_names();
    assert_eq!(
        names.len(),
        M1_SLICE.len(),
        "注册的命令数应为 {}，实际 {names:?}",
        M1_SLICE.len()
    );
    for name in M1_SLICE {
        assert!(names.contains(&name), "缺少命令 {name}；实际 {names:?}");
    }
}

/// Tauri 默认按 camelCase 暴露命令名。切片表里的 JS 名若与处理函数名的 camelCase 形式
/// 不一致，渲染层就会 `invoke` 到一个不存在的命令 —— 这里把这个隐式约定钉成断言。
#[test]
fn js_names_match_handler_function_names() {
    assert_eq!(M1_HANDLER_PATHS.len(), M1_SLICE.len());
    for (js_name, handler_path) in registered_names().iter().zip(M1_HANDLER_PATHS) {
        let function_name = handler_path.rsplit("::").next().unwrap_or(handler_path).trim();
        assert_eq!(
            to_camel_case(function_name),
            *js_name,
            "处理函数 {handler_path} 的 camelCase 名与切片表里的 {js_name} 不一致"
        );
    }
}

/// 与 `heck` 的 lowerCamelCase 同口径：下划线后的字母升格，其余原样。
fn to_camel_case(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut uppercase_next = false;
    for ch in snake.chars() {
        if ch == '_' {
            uppercase_next = true;
            continue;
        }
        if uppercase_next {
            out.extend(ch.to_uppercase());
            uppercase_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

#[test]
fn slice_has_no_duplicate_names() {
    let mut names = registered_names().to_vec();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "命令名有重复：{names:?}");
}
