//! M1 平台命令面的断言。规格：`docs/specs/tauri-platform-commands.md`（命令切片表）。
//!
//! 命令名必须与 `IPlatformService`（`packages/shared/src/platform.ts`）逐字一致 —— 渲染层
//! 的 `invoke()` 适配层按同名映射，改名即断链。
//!
//! 本文件必须留在 `tests/`（集成测试目标）：`crates/platform/build.rs` 只对**测试目标**
//! 注入 comctl32 v6 清单（`rustc-link-arg-tests`），而 cargo 只把 `tests/` 下的目标算作
//! 测试目标；放进 lib 单测则拿不到清单，进程启动即 `STATUS_ENTRYPOINT_NOT_FOUND`。
use platform::commands::shell::external_file_target_error;
use platform::commands::{registered_names, M1_HANDLER_PATHS};

/// 切片表里的命令，顺序与 spec 表格一致。
const M1_SLICE: [&str; 13] = [
    "openExternal",
    "selectDirectory",
    "selectFile",
    "saveFile",
    "createTempTextAttachment",
    "openInFileManager",
    "openExternalFile",
    "showTaskNotification",
    "notifyRendererReady",
    "syncWindowTabs",
    "syncWindowUnreadCount",
    "getApplicationIcon",
    // LR-3：远程桌面剪贴板读取（壳能力）。
    "rdReadClipboardText",
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

/// 切片表内部一致：JS 名 = 处理函数名的 camelCase 形式。
///
/// **注意这条不够**：它只看切片表自己的两个常量，看不出线上真正的命令名。
/// 线上名由 `#[tauri::command(rename = "...")]` 决定（见下一条用例），
/// 而且**默认不带 rename 时 Tauri 用的是 snake_case 函数名**——
/// 这正是 2026-10-16 smoke 里「10 条命令全部 `Command X not found`」的根因：
/// 本用例当时全绿，渲染层却一条都调不通。
#[test]
fn js_names_match_handler_function_names() {
    assert_eq!(M1_HANDLER_PATHS.len(), M1_SLICE.len());
    for (js_name, handler_path) in registered_names().iter().zip(M1_HANDLER_PATHS) {
        let function_name = handler_path
            .rsplit("::")
            .next()
            .unwrap_or(handler_path)
            .trim();
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

/// 命令源文件（编译期读入，路径错了直接编译失败）。
///
/// 本文件是**集成测试**，路径相对本文件所在目录：`tests/` → `../src/commands/`。
const COMMAND_SOURCES: &[&str] = &[
    include_str!("../src/commands/shell.rs"),
    include_str!("../src/commands/dialog.rs"),
    include_str!("../src/commands/notify.rs"),
    include_str!("../src/commands/window.rs"),
    include_str!("../src/commands/icon.rs"),
    include_str!("../src/commands/clipboard.rs"),
    include_str!("../src/commands/temp_attachment.rs"),
];

/// 线上真正的命令名 = `#[tauri::command(rename = "...")]` 的字面量。
///
/// 这一段必须扫源码而不是查常量：Tauri 的 `generate_handler!` 是按
/// `#[tauri::command]` 生成的 `__tauri_command_name_*` 宏的**返回值**做 match 的
/// （`tauri-macros` 的 `command/wrapper.rs`：带 rename 用字面量，不带则用
/// `stringify!(函数名)`）。也就是说 wire 名不存在于任何 Rust 常量里，
/// 只有源码能证明它。少了 rename，渲染层的 `invoke("getApplicationIcon")`
/// 会命不中 `get_application_icon` 这个 match 分支，运行时才报 `Command ... not found`。
#[test]
fn every_command_declares_a_camel_case_wire_name() {
    let mut seen: Vec<String> = Vec::new();

    for source in COMMAND_SOURCES {
        let total = source.matches("#[tauri::command").count();
        let renamed = source.matches("#[tauri::command(rename = \"").count();
        assert_eq!(
            total, renamed,
            "有 {total} 个 #[tauri::command] 但只有 {renamed} 个带 rename：\
             不带 rename 的命令会以 snake_case 注册，渲染层调不到"
        );

        let mut rest = *source;
        while let Some(at) = rest.find("#[tauri::command(rename = \"") {
            let after = &rest[at..];
            let value_start = "#[tauri::command(rename = \"".len();
            let value_end = value_start
                + after[value_start..]
                    .find('"')
                    .expect("rename 的字面量没有闭合引号");
            let wire_name = &after[value_start..value_end];

            // 属性后面的第一个 `fn` 就是被修饰的函数。
            let fn_at = after.find("fn ").expect("rename 属性后面没有 fn");
            let ident_start = fn_at + "fn ".len();
            let ident_end = ident_start
                + after[ident_start..]
                    .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                    .expect("函数名后没有分隔符");
            let function_name = &after[ident_start..ident_end];

            assert_eq!(
                to_camel_case(function_name),
                wire_name,
                "函数 {function_name} 的 rename 是 {wire_name}，\
                 但 IPlatformService 的方法名要求 camelCase"
            );
            seen.push(wire_name.to_string());
            rest = &after[ident_end..];
        }
    }

    seen.sort_unstable();
    let mut expected = M1_SLICE.to_vec();
    expected.sort_unstable();
    assert_eq!(seen, expected, "rename 出来的 wire 名与切片表不一致");
}

#[test]
fn slice_has_no_duplicate_names() {
    let mut names = registered_names().to_vec();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "命令名有重复：{names:?}");
}

#[test]
fn external_file_target_requires_an_existing_file() {
    let dir = std::env::temp_dir().join(format!("mode-open-external-file-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");

    let missing = dir.join("missing.txt");
    let error = external_file_target_error(&missing).expect("不存在路径必须失败");
    assert!(
        error.starts_with("path_unavailable:"),
        "错误应带稳定前缀：{error}"
    );

    let directory = dir.join("sub");
    std::fs::create_dir_all(&directory).expect("建子目录");
    assert_eq!(
        external_file_target_error(&directory).as_deref(),
        Some("not_a_file"),
        "目录不得交给默认应用（目录走 openInFileManager）"
    );

    let file = dir.join("a.txt");
    std::fs::write(&file, b"x").expect("写文件");
    assert_eq!(external_file_target_error(&file), None);

    std::fs::remove_dir_all(&dir).expect("清理");
}
