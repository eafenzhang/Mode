//! Chrome Native Messaging host（P1）的协议层。
//!
//! 规格：`docs/specs/chrome-browser-import-extension.md` §4.1。
//! 安全要求：`docs/specs/chrome-abe-import-security.md` R1–R10。
//!
//! 传输是 Chrome 规定的 **4 字节小端长度 + UTF-8 JSON**；单条消息上限 1 MiB。
//! 本层只做「读写 + 形状校验 + 稳定错误码」，不碰 cookie 的落盘/写入（那是 Mode main 的事）。
//!
//! # 边界
//!
//! - 不信任任何字段：`v`/`kind`/cookie 字段逐项校验，失败返回稳定码；
//! - 不把 payload 写日志；
//! - 不实现 host ↔ app 的命名管道（T2）。
use std::fmt;
use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Chrome Native Messaging 的单条消息上限（1 MiB）。
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// 单次导入的 cookie 条数上限（防御性上限；正常 profile 远小于此）。
pub const MAX_COOKIES: usize = 100_000;

/// 协议错误。`code()` 是给扩展/用户看的稳定码。
#[derive(Debug)]
pub enum HostError {
    Io(io::Error),
    /// 长度前缀非法（0、截断或非 4 字节）。
    InvalidLength,
    /// 超过 [`MAX_MESSAGE_BYTES`]。
    TooLarge,
    InvalidJson(serde_json::Error),
    /// JSON 合法但形状/取值不符合本协议。
    InvalidPayload(&'static str),
}

impl HostError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "io_error",
            Self::InvalidLength => "invalid_length",
            Self::TooLarge => "payload_too_large",
            Self::InvalidJson(_) => "invalid_json",
            Self::InvalidPayload(reason) => reason,
        }
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "io error: {error}"),
            Self::InvalidLength => write!(formatter, "invalid message length"),
            Self::TooLarge => write!(formatter, "message exceeds {MAX_MESSAGE_BYTES} bytes"),
            Self::InvalidJson(error) => write!(formatter, "invalid json: {error}"),
            Self::InvalidPayload(reason) => write!(formatter, "invalid payload: {reason}"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<io::Error> for HostError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for HostError {
    fn from(error: serde_json::Error) -> Self {
        Self::InvalidJson(error)
    }
}

/// 读一条长度前缀消息的正文。
pub fn read_message<R: Read>(reader: &mut R) -> Result<Vec<u8>, HostError> {
    let mut length_bytes = [0_u8; 4];
    reader.read_exact(&mut length_bytes)?;
    let length = u32::from_le_bytes(length_bytes) as usize;
    if length == 0 || length > MAX_MESSAGE_BYTES {
        return Err(if length > MAX_MESSAGE_BYTES {
            HostError::TooLarge
        } else {
            HostError::InvalidLength
        });
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

/// 写一条长度前缀消息。
pub fn write_message<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), HostError> {
    if payload.is_empty() || payload.len() > MAX_MESSAGE_BYTES {
        return Err(if payload.is_empty() {
            HostError::InvalidLength
        } else {
            HostError::TooLarge
        });
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}

/// 导出请求（扩展 → host）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CookieImportRequest {
    pub v: u32,
    pub kind: String,
    pub cookies: Vec<Cookie>,
}

/// 一条 cookie。字段名与 `chrome.cookies.Cookie` 的对外形状一致；只保留导入需要的字段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, rename = "httpOnly")]
    pub http_only: bool,
    #[serde(default)]
    pub same_site: Option<String>,
    #[serde(default)]
    pub expiration_date: Option<f64>,
    #[serde(default)]
    pub host_only: bool,
}

const SAME_SITE_VALUES: [&str; 4] = ["no_restriction", "lax", "strict", "unspecified"];

/// 校验并解析一条请求。
pub fn parse_request(payload: &[u8]) -> Result<CookieImportRequest, HostError> {
    let request: CookieImportRequest = serde_json::from_slice(payload)?;
    if request.v != 1 {
        return Err(HostError::InvalidPayload("unsupported_version"));
    }
    if request.kind != "cookies" {
        return Err(HostError::InvalidPayload("unsupported_kind"));
    }
    if request.cookies.len() > MAX_COOKIES {
        return Err(HostError::InvalidPayload("too_many_cookies"));
    }
    for cookie in &request.cookies {
        validate_cookie(cookie)?;
    }
    Ok(request)
}

fn validate_cookie(cookie: &Cookie) -> Result<(), HostError> {
    if cookie.name.is_empty() || cookie.name.chars().any(char::is_control) {
        return Err(HostError::InvalidPayload("invalid_cookie_name"));
    }
    if cookie.domain.is_empty()
        || cookie.domain.chars().any(char::is_control)
        || cookie.domain.chars().any(char::is_whitespace)
    {
        return Err(HostError::InvalidPayload("invalid_cookie_domain"));
    }
    if !cookie.path.starts_with('/') || cookie.path.chars().any(char::is_control) {
        return Err(HostError::InvalidPayload("invalid_cookie_path"));
    }
    if let Some(same_site) = cookie.same_site.as_deref() {
        if !SAME_SITE_VALUES.contains(&same_site) {
            return Err(HostError::InvalidPayload("invalid_same_site"));
        }
    }
    if let Some(expiration) = cookie.expiration_date {
        if !expiration.is_finite() {
            return Err(HostError::InvalidPayload("invalid_expiration_date"));
        }
    }
    Ok(())
}

/// 成功响应：只回计数，不回显 cookie。
pub fn success_response(count: usize) -> Value {
    serde_json::json!({ "ok": true, "count": count })
}

/// 失败响应：只回稳定码。
pub fn error_response(code: &str) -> Value {
    serde_json::json!({ "ok": false, "error": code })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn encoded(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).expect("json")
    }

    fn framed(value: &Value) -> Vec<u8> {
        let payload = encoded(value);
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&payload);
        frame
    }

    fn sample() -> Value {
        json!({
            "v": 1,
            "kind": "cookies",
            "cookies": [{
                "name": "mode_test",
                "value": "secret",
                "domain": ".example.com",
                "path": "/",
                "secure": true,
                "httpOnly": true,
                "sameSite": "lax",
                "expirationDate": 1_900_000_000.0,
                "hostOnly": false
            }]
        })
    }

    #[test]
    fn read_write_roundtrip() {
        let frame = framed(&sample());
        let mut reader = frame.as_slice();
        let payload = read_message(&mut reader).expect("read");
        let request = parse_request(&payload).expect("parse");
        assert_eq!(request.cookies.len(), 1);
        assert_eq!(request.cookies[0].name, "mode_test");
        assert_eq!(request.cookies[0].same_site.as_deref(), Some("lax"));
    }

    #[test]
    fn zero_length_is_rejected() {
        let bytes = 0_u32.to_le_bytes();
        let mut reader = bytes.as_slice();
        assert!(matches!(
            read_message(&mut reader),
            Err(HostError::InvalidLength)
        ));
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let bytes = ((MAX_MESSAGE_BYTES + 1) as u32).to_le_bytes();
        let mut reader = bytes.as_slice();
        assert!(matches!(
            read_message(&mut reader),
            Err(HostError::TooLarge)
        ));
    }

    #[test]
    fn invalid_json_is_rejected() {
        assert!(matches!(
            parse_request(b"{"),
            Err(HostError::InvalidJson(_))
        ));
    }

    #[test]
    fn missing_and_unknown_fields_are_rejected() {
        let missing = json!({ "v": 1, "kind": "cookies" });
        assert!(matches!(
            parse_request(&encoded(&missing)),
            Err(HostError::InvalidJson(_))
        ));
        let mut unknown = sample();
        unknown["extra"] = json!(true);
        assert!(matches!(
            parse_request(&encoded(&unknown)),
            Err(HostError::InvalidJson(_))
        ));
    }

    #[test]
    fn unsupported_version_and_kind_are_rejected() {
        let mut version = sample();
        version["v"] = json!(2);
        assert!(matches!(
            parse_request(&encoded(&version)),
            Err(HostError::InvalidPayload("unsupported_version"))
        ));
        let mut kind = sample();
        kind["kind"] = json!("history");
        assert!(matches!(
            parse_request(&encoded(&kind)),
            Err(HostError::InvalidPayload("unsupported_kind"))
        ));
    }

    #[test]
    fn invalid_cookie_fields_are_rejected() {
        for (field, value, code) in [
            ("name", json!(""), "invalid_cookie_name"),
            ("domain", json!("bad domain"), "invalid_cookie_domain"),
            ("path", json!("no-slash"), "invalid_cookie_path"),
            ("sameSite", json!("sometimes"), "invalid_same_site"),
        ] {
            let mut request = sample();
            request["cookies"][0][field] = value;
            assert!(
                matches!(
                    parse_request(&encoded(&request)),
                    Err(HostError::InvalidPayload(actual)) if actual == code
                ),
                "{field} 应该被拒绝"
            );
        }
    }

    #[test]
    fn responses_do_not_echo_values() {
        assert_eq!(success_response(34), json!({ "ok": true, "count": 34 }));
        assert_eq!(
            error_response("payload_too_large"),
            json!({ "ok": false, "error": "payload_too_large" })
        );
    }

    #[test]
    fn write_rejects_empty_and_oversized_payloads() {
        let mut output = Vec::new();
        assert!(matches!(
            write_message(&mut output, b""),
            Err(HostError::InvalidLength)
        ));
        let oversized = vec![0_u8; MAX_MESSAGE_BYTES + 1];
        assert!(matches!(
            write_message(&mut output, &oversized),
            Err(HostError::TooLarge)
        ));
    }
}
