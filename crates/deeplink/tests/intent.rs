//! `mode://` 解析的行为契约。
//!
//! 取材：`packages/desktop/tests/deep-link-scheme-compat.test.mjs` 的断言（黑盒行为规格）。
//! 用例名与断言语义逐条对应，便于两边对照；本文件不引用旧实现源码。
use deeplink::intent::{extract_from_args, parse};
use deeplink::{DeepLinkIntent, ACCEPTED_SCHEMES, REGISTERED_SCHEME};

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
