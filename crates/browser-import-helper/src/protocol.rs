//! 契约 B 的 wire 层：broker 模式的请求/响应编解码。
//!
//! 契约：`docs/specs/browser-import-helper-contract.md`。
//! 请求（stdin 一行、制表符分隔 3 字段）：`<协议名>\t<base64(密文)>\t<base64(chrome.exe 路径)>\n`
//! 响应（stdout 一行、制表符分隔 3 字段）：`<协议名>\tOK\t<base64(32 字节密钥)>`
//! 或 `<协议名>\tERR\t<reason>`。
use base64::Engine;

use crate::cli::WIRE_PROTOCOL;

/// 宿主 reason 白名单（`APP_BOUND_HELPER_FAILURE_REASONS`），重写必须覆盖这 13 项。
pub const HOST_WHITELIST_REASONS: [&str; 13] = [
    "helper_failed",
    "broker_initialization_failed",
    "controller_handshake_failed",
    "service_channel_failed",
    "elevation_failed",
    "timeout",
    "peer_verification_failed",
    "service_failed",
    "validation_failed",
    "unsupported_key",
    "cng_failed",
    "decryption_failed",
    "service_cleanup_failed",
];

/// 宿主在 `parseHelperResponse` 里**先于白名单**单独识别的两项。
/// 它们不在上面 13 项里，但 helper 必须能发出——否则 UAC 取消路径无法映射。
pub const HOST_SPECIAL_REASONS: [&str; 2] = ["elevation_cancelled", "helper_verification_failed"];

/// helper 可对外发出的 reason 全集 = 白名单 13 项 + 宿主单独识别的 2 项。
pub const REASONS: [&str; 15] = [
    "helper_failed",
    "broker_initialization_failed",
    "controller_handshake_failed",
    "service_channel_failed",
    "elevation_failed",
    "timeout",
    "peer_verification_failed",
    "service_failed",
    "validation_failed",
    "unsupported_key",
    "cng_failed",
    "decryption_failed",
    "service_cleanup_failed",
    // 以下两项属 HOST_SPECIAL_REASONS，契约要求可发出。
    "elevation_cancelled",
    "helper_verification_failed",
];

/// 密钥长度是契约的一部分：宿主拿到的必须是 32 字节。
pub const KEY_LEN: usize = 32;

#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub encrypted_key: Vec<u8>,
    pub chrome_exe_path: String,
}

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

pub fn parse_request(line: &str) -> Result<Request, &'static str> {
    let trimmed = line.strip_suffix('\n').unwrap_or(line);
    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
    let fields: Vec<&str> = trimmed.split('\t').collect();
    if fields.len() != 3 || fields[0] != WIRE_PROTOCOL {
        return Err("validation_failed");
    }
    let encrypted_key = b64()
        .decode(fields[1])
        .map_err(|_| "validation_failed")?;
    if encrypted_key.is_empty() {
        return Err("validation_failed");
    }
    let path_bytes = b64().decode(fields[2]).map_err(|_| "validation_failed")?;
    let chrome_exe_path = String::from_utf8(path_bytes).map_err(|_| "validation_failed")?;
    if chrome_exe_path.is_empty() {
        return Err("validation_failed");
    }
    Ok(Request {
        encrypted_key,
        chrome_exe_path,
    })
}

pub fn ok_response(key: &[u8; KEY_LEN]) -> String {
    format!("{}\tOK\t{}\n", WIRE_PROTOCOL, b64().encode(key))
}

pub fn err_response(reason: &str) -> String {
    let reason = if REASONS.contains(&reason) {
        reason
    } else {
        // 未知 reason 不得泄给宿主：归并成通用失败（与宿主侧归并口径一致）。
        "helper_failed"
    };
    format!("{}\tERR\t{}\n", WIRE_PROTOCOL, reason)
}
