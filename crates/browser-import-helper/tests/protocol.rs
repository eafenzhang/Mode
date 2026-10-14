//! 契约 B wire 层的断言（`docs/specs/browser-import-helper-contract.md`）。
use base64::Engine;
use browser_import_helper::protocol::{
    err_response, ok_response, parse_request, HOST_SPECIAL_REASONS, HOST_WHITELIST_REASONS,
    KEY_LEN, REASONS,
};

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[test]
fn parses_contract_b_request() {
    let line = format!(
        "ZCODE_BROWSER_IMPORT_HELPER\t{}\t{}\n",
        b64(&[7u8; 12]),
        b64(b"C:\\chrome.exe")
    );
    let request = parse_request(&line).expect("应当解析成功");
    assert_eq!(request.encrypted_key, vec![7u8; 12]);
    assert_eq!(request.chrome_exe_path, "C:\\chrome.exe");
}

#[test]
fn accepts_crlf_and_missing_trailing_newline() {
    let line = format!(
        "ZCODE_BROWSER_IMPORT_HELPER\t{}\t{}",
        b64(&[1u8; 4]),
        b64(b"C:\\c.exe")
    );
    assert!(parse_request(&line).is_ok());
    assert!(parse_request(&format!("{line}\r\n")).is_ok());
}

#[test]
fn rejects_malformed_frames() {
    for line in [
        "",
        "ZCODE_BROWSER_IMPORT_HELPER\tonly\ttwo",   // 字段数不足
        "WRONG_PROTOCOL\tAA==\tAA==",               // 协议名不符
        "ZCODE_BROWSER_IMPORT_HELPER\t!!!\tAA==",   // 非 base64 密文
        "ZCODE_BROWSER_IMPORT_HELPER\tAA==\t!!!",   // 非 base64 路径
        "ZCODE_BROWSER_IMPORT_HELPER\t\tAA==",      // 空密文
        "ZCODE_BROWSER_IMPORT_HELPER\tAA==\t",      // 空路径
    ] {
        assert_eq!(
            parse_request(line),
            Err("validation_failed"),
            "line={line:?}"
        );
    }
}

#[test]
fn rejects_non_utf8_path() {
    let line = format!(
        "ZCODE_BROWSER_IMPORT_HELPER\t{}\t{}",
        b64(&[1u8; 4]),
        b64(&[0xff, 0xfe])
    );
    assert_eq!(parse_request(&line), Err("validation_failed"));
}

#[test]
fn ok_response_is_three_fields_with_32_byte_key() {
    let response = ok_response(&[1u8; KEY_LEN]);
    let fields: Vec<&str> = response.trim().split('\t').collect();
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0], "ZCODE_BROWSER_IMPORT_HELPER");
    assert_eq!(fields[1], "OK");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(fields[2])
        .expect("base64");
    assert_eq!(decoded.len(), KEY_LEN);
    assert!(response.ends_with('\n'));
}

#[test]
fn err_response_carries_known_reason() {
    assert_eq!(
        err_response("unsupported_key").trim(),
        "ZCODE_BROWSER_IMPORT_HELPER\tERR\tunsupported_key"
    );
    assert_eq!(
        err_response("elevation_cancelled").trim(),
        "ZCODE_BROWSER_IMPORT_HELPER\tERR\televation_cancelled"
    );
}

#[test]
fn unknown_reason_is_folded_to_generic_failure() {
    // 契约只允许白名单内的 reason 外露；其余归并，避免泄漏内部判定。
    assert_eq!(
        err_response("some_internal_detail").trim(),
        "ZCODE_BROWSER_IMPORT_HELPER\tERR\thelper_failed"
    );
}

#[test]
fn reason_set_matches_host_contract() {
    // 契约分两处列举：白名单 13 项 + 宿主单独识别的 2 项。
    assert_eq!(HOST_WHITELIST_REASONS.len(), 13);
    assert_eq!(HOST_SPECIAL_REASONS.len(), 2);
    assert_eq!(REASONS.len(), 15);
    for reason in HOST_WHITELIST_REASONS.iter().chain(HOST_SPECIAL_REASONS.iter()) {
        assert!(REASONS.contains(reason), "REASONS 缺少 {reason}");
    }
    assert!(HOST_SPECIAL_REASONS.contains(&"elevation_cancelled"));
    assert!(HOST_SPECIAL_REASONS.contains(&"helper_verification_failed"));
}
