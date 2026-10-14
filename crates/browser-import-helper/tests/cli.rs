//! 契约 A 的断言。期望值与 `docs/specs/browser-import-helper-contract.md` 一致，
//! 并与黑盒取到的旧产物输出并列核对（见 packages/desktop/tests/browser-import-helper-contract.test.mjs）。
use browser_import_helper::cli::{parse, version_banner, Mode, PROTOCOL, PROTOCOL_VERSION};

#[test]
fn version_banner_has_five_frozen_fields() {
    let banner = version_banner();
    let fields: Vec<&str> = banner.trim().split('\t').collect();
    assert_eq!(fields.len(), 5, "banner={banner:?}");
    assert_eq!(fields[0], PROTOCOL);
    assert_eq!(fields[1], PROTOCOL_VERSION);
    assert!(banner.ends_with('\n'));
}

#[test]
fn version_banner_uses_contract_arch_token() {
    let banner = version_banner();
    let fields: Vec<&str> = banner.trim().split('\t').collect();
    // 契约要求 x64/arm64 这类 token，而不是 std::env::consts::ARCH 的原始值。
    #[cfg(target_arch = "x86_64")]
    assert_eq!(fields[2], "x64");
    #[cfg(target_arch = "aarch64")]
    assert_eq!(fields[2], "arm64");
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    assert!(!fields[2].is_empty());
}

#[test]
fn parses_version() {
    let args = vec!["--version".to_string()];
    assert_eq!(parse(&args), Mode::Version);
}

#[test]
fn parses_broker_with_parent_pid() {
    let args = vec![
        "--broker".to_string(),
        "--parent-pid".to_string(),
        "42".to_string(),
    ];
    assert_eq!(parse(&args), Mode::Broker { parent_pid: 42 });
}

#[test]
fn unknown_or_incomplete_args_are_usage_error() {
    assert_eq!(parse(&[]), Mode::UsageError);
    assert_eq!(parse(&["--nope".to_string()]), Mode::UsageError);
    assert_eq!(parse(&["--broker".to_string()]), Mode::UsageError);
    assert_eq!(
        parse(&[
            "--broker".to_string(),
            "--parent-pid".to_string(),
            "not-a-pid".to_string(),
        ]),
        Mode::UsageError
    );
}
