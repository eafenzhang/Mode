//! 契约 B wire 层的断言（`docs/specs/browser-import-helper-contract.md`）。
//!
//! 令牌：请求/响应帧都用宿主常量 `HELPER_PROTOCOL = MODE_BROWSER_IMPORT_V1`；
//! `--version` 横幅用的是另一个常量 `MODE_BROWSER_IMPORT_HELPER`。旧产物把两者混用，
//! 这里用 ZCODE_ 前缀作为「必须被拒绝」的反例，锁死这个缺陷不会回来。
use base64::Engine;
use browser_import_helper::cli::WIRE_PROTOCOL;
use browser_import_helper::protocol::{
    err_response, ok_response, parse_request, HOST_SPECIAL_REASONS, HOST_WHITELIST_REASONS,
    KEY_LEN, REASONS,
};

const WIRE: &str = "MODE_BROWSER_IMPORT_V1";

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn frame(token: &str, blob: &[u8], path: &str) -> String {
    format!("{token}\t{}\t{}\n", b64(blob), b64(path.as_bytes()))
}

#[test]
fn wire_protocol_constant_matches_host() {
    assert_eq!(WIRE_PROTOCOL, WIRE);
}

#[test]
fn parses_contract_b_request() {
    let request = parse_request(&frame(WIRE, &[7u8; 12], "C:\\chrome.exe")).expect("应当解析成功");
    assert_eq!(request.encrypted_key, vec![7u8; 12]);
    assert_eq!(request.chrome_exe_path, "C:\\chrome.exe");
}

#[test]
fn rejects_legacy_zcode_token() {
    // 旧产物回帧/发帧用的是 ZCODE_BROWSER_IMPORT_V1（与自家宿主断言不符的历史缺陷）。
    // 本实现只认宿主契约里的令牌，不接受历史品牌串。
    assert_eq!(
        parse_request(&frame("ZCODE_BROWSER_IMPORT_V1", &[1u8; 4], "C:\\c.exe")),
        Err("validation_failed")
    );
}

#[test]
fn accepts_crlf_and_missing_trailing_newline() {
    let line = frame(WIRE, &[1u8; 4], "C:\\c.exe");
    assert!(parse_request(line.trim_end()).is_ok());
    assert!(parse_request(&line.replace('\n', "\r\n")).is_ok());
}

#[test]
fn rejects_malformed_frames() {
    let bad = [
        String::new(),
        format!("{WIRE}\tonly\ttwo"),
        "WRONG_PROTOCOL\tAA==\tAA==".to_string(),
        format!("{WIRE}\t!!!\t{}", b64(b"C:\\c.exe")), // 非 base64 密文
        format!("{WIRE}\t{}\t!!!", b64(&[1u8; 4])),    // 非 base64 路径
        format!("{WIRE}\t\t{}", b64(b"C:\\c.exe")),    // 空密文
        format!("{WIRE}\t{}\t", b64(&[1u8; 4])),       // 空路径
    ];
    for line in bad {
        assert_eq!(parse_request(&line), Err("validation_failed"), "line={line:?}");
    }
}

#[test]
fn rejects_non_utf8_path() {
    let line = format!("{WIRE}\t{}\t{}", b64(&[1u8; 4]), b64(&[0xff, 0xfe]));
    assert_eq!(parse_request(&line), Err("validation_failed"));
}

#[test]
fn ok_response_uses_wire_token_and_32_byte_key() {
    let response = ok_response(&[1u8; KEY_LEN]);
    let fields: Vec<&str> = response.trim().split('\t').collect();
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[0], WIRE);
    assert_eq!(fields[1], "OK");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(fields[2])
        .expect("base64");
    assert_eq!(decoded.len(), KEY_LEN);
    assert!(response.ends_with('\n'));
}

#[test]
fn responses_never_use_the_version_token() {
    // 反向锁：回帧绝不能出现横幅那个品牌串，否则宿主 391 行断言必然失败。
    assert!(!ok_response(&[0u8; KEY_LEN]).starts_with("MODE_BROWSER_IMPORT_HELPER"));
    assert!(!err_response("helper_failed").starts_with("MODE_BROWSER_IMPORT_HELPER"));
}

#[test]
fn err_response_carries_known_reason() {
    assert_eq!(
        err_response("unsupported_key").trim(),
        format!("{WIRE}\tERR\tunsupported_key")
    );
    assert_eq!(
        err_response("elevation_cancelled").trim(),
        format!("{WIRE}\tERR\televation_cancelled")
    );
}

#[test]
fn unknown_reason_is_folded_to_generic_failure() {
    assert_eq!(
        err_response("some_internal_detail").trim(),
        format!("{WIRE}\tERR\thelper_failed")
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
