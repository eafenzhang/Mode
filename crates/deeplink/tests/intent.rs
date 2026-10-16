//! `mode://` 解析的行为契约。
//!
//! 取材：`packages/desktop/tests/deep-link-scheme-compat.test.mjs` 的断言（黑盒行为规格）。
//! 用例名与断言语义逐条对应，便于两边对照；本文件不引用旧实现源码。
use deeplink::intent::{extract_from_args, parse};
use deeplink::{forwardable_second_instance_url, DeepLinkIntent, ACCEPTED_SCHEMES, REGISTERED_SCHEME};

#[test]
fn oauth_callback_is_recognized_for_both_the_new_and_the_pre_rename_scheme() {
    assert_eq!(
        parse("mode://oauth/callback"),
        Some(DeepLinkIntent::OAuthCallback {
            url: "mode://oauth/callback".to_string()
        })
    );
    assert_eq!(
        parse("zcode://oauth/callback"),
        Some(DeepLinkIntent::OAuthCallback {
            url: "zcode://oauth/callback".to_string()
        })
    );
    // 同名路径但 scheme 不是本应用的：不认（旧线断言 https://oauth/callback = false）。
    assert_eq!(parse("https://oauth/callback"), None);
}

#[test]
fn only_the_new_scheme_is_registered_while_both_are_processed() {
    // docs/specs/p2-mode-naming.md：不注册旧 scheme，避免与宿主应用争抢默认 handler。
    assert_eq!(REGISTERED_SCHEME, "mode");
    assert_eq!(ACCEPTED_SCHEMES, &["mode", "zcode"]);
}

#[test]
fn workspace_open_decodes_the_path_query() {
    let intent = parse("zcode://workspace/open?path=C%3A%5Cwork%5Cdemo");
    assert_eq!(
        intent,
        Some(DeepLinkIntent::WorkspaceOpen {
            path: r"C:\work\demo".to_string()
        })
    );
    assert_eq!(
        parse("mode://workspace/open?path=/home/me/demo"),
        Some(DeepLinkIntent::WorkspaceOpen {
            path: "/home/me/demo".to_string()
        })
    );
}

#[test]
fn share_import_decodes_the_code_query() {
    assert_eq!(
        parse("zcode://share/import?code=abc123"),
        Some(DeepLinkIntent::ShareImport {
            code: "abc123".to_string()
        })
    );
}

#[test]
fn payment_callback_is_recognized() {
    assert_eq!(
        parse("mode://payment/callback?state=p1"),
        Some(DeepLinkIntent::PaymentCallback {
            url: "mode://payment/callback?state=p1".to_string()
        })
    );
}

#[test]
fn route_queries_are_required_not_optional() {
    // `workspace/open` 缺 `path`、`share/import` 缺 `code` 都构不成一条可用意图，
    // 与「有规则但规则不成立」同属未识别，而不是拿着一半参数往下走。
    assert_eq!(
        parse("mode://workspace/open"),
        Some(DeepLinkIntent::Unrecognized {
            url: "mode://workspace/open".to_string()
        })
    );
    assert_eq!(
        parse("mode://share/import?code="),
        Some(DeepLinkIntent::Unrecognized {
            url: "mode://share/import?code=".to_string()
        })
    );
}

#[test]
fn unknown_route_under_a_known_scheme_is_reported_as_unrecognized() {
    assert_eq!(
        parse("mode://something/else"),
        Some(DeepLinkIntent::Unrecognized {
            url: "mode://something/else".to_string()
        })
    );
}

#[test]
fn scheme_without_authority_is_not_a_deep_link() {
    // 深链形态固定是 `scheme://host/path`；`mode:oauth/callback` 这种写法旧线上不存在，
    // 这里也不认，避免把两种写法当成同一条路由。
    assert_eq!(parse("mode:oauth/callback"), None);
    assert_eq!(parse(""), None);
    assert_eq!(parse("   "), None);
}

#[test]
fn argv_extraction_matches_whole_token_prefixes_only() {
    let args = [
        r"C:\Program Files\Mode\Mode.exe",
        "mode://oauth/callback?state=state-1",
    ];
    assert_eq!(
        extract_from_args(args),
        Some("mode://oauth/callback?state=state-1".to_string())
    );

    let legacy = [
        r"C:\Program Files\Mode\Mode.exe",
        "zcode://oauth/callback?state=state-2",
    ];
    assert_eq!(
        extract_from_args(legacy),
        Some("zcode://oauth/callback?state=state-2".to_string())
    );

    // 词中间出现 scheme 的普通路径不算链接（旧线断言：`C:\work\asmode.txt`）。
    assert_eq!(extract_from_args([r"C:\work\asmode.txt"]), None);
    assert_eq!(extract_from_args([r"C:\work\zcode.txt"]), None);
    assert_eq!(extract_from_args::<[&str; 0], &str>([]), None);
}

/// 第二实例转发（M3 Task 4 · DR-T4-1）要转交的那条深链。
///
/// 与 [`extract_from_args`] 同源，但这是**另一个调用点**：单实例插件把第二实例的 `argv`
/// **原样**（含 `argv[0]` = 可执行文件路径、以及用户可能带的别的参数）送进壳的回调。
/// 所以这里钉两件事：① 链接在 argv 的**任意位置**都能取到；② **没有链接**的第二次启动
/// （用户又点了一次图标）必须给 `None`——那是「聚焦既有窗口」，不是「转发一条深链」。
#[test]
fn the_second_instance_argv_yields_the_deep_link_to_forward_or_nothing() {
    // ① 操作系统的现场形态：argv[0] 是 exe 路径，链接随后。
    let forwarded = [
        r"C:\Program Files\Mode\Mode.exe".to_string(),
        "mode://oauth/callback?state=second-instance".to_string(),
    ];
    assert_eq!(
        forwardable_second_instance_url(&forwarded),
        Some("mode://oauth/callback?state=second-instance".to_string())
    );

    // 链接不在第二位时也要取到（插件不保证参数顺序，用户也可能带了别的参数）。
    let later = [
        r"C:\Program Files\Mode\Mode.exe".to_string(),
        "--old-instance".to_string(),
        "mode://payment/callback?state=p2".to_string(),
    ];
    assert_eq!(
        forwardable_second_instance_url(&later),
        Some("mode://payment/callback?state=p2".to_string())
    );

    // 旧 scheme 也照收（改名前的链接仍然可用，与处理侧口径一致）。
    assert_eq!(
        forwardable_second_instance_url(vec![
            r"C:\Mode\Mode.exe".to_string(),
            "zcode://share/import?code=abc".to_string()
        ]),
        Some("zcode://share/import?code=abc".to_string())
    );

    // ② 只是又点了一次图标：没有链接 → 没有要转发的深链。
    assert_eq!(
        forwardable_second_instance_url(vec![r"C:\Mode\Mode.exe".to_string()]),
        None
    );
    // 词中间含 scheme 的普通路径不算（同 `extract_from_args` 的口径）。
    assert_eq!(
        forwardable_second_instance_url(vec![
            r"C:\Mode\Mode.exe".to_string(),
            r"C:\work\asmode.txt".to_string()
        ]),
        None
    );
    assert_eq!(forwardable_second_instance_url(Vec::<String>::new()), None);
}
