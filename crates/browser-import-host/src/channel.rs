//! host ↔ Mode main 的命名管道（P1/T2）。
//!
//! 规格：`docs/specs/chrome-browser-import-extension.md` §4.2、§5。
//! 传输与 Native Messaging 相同（4 字节小端长度 + JSON），但**不是** Chrome 的协议：
//! 先发 `hello` + 一次性 token，main 回 `ready`，再发已校验的 cookie payload，main 回计数。
//!
//! 安全（R1–R10）：
//! - token 由 main 生成、一次性、TTL ≤60s；
//! - channel.json 只含管道名/token/过期时间，不含 cookie；
//! - 本模块不打印 token 与 payload。
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{read_message, write_message, HostError};

/// 默认的握手超时（规格 §6 的 ≤60s TTL 之内）。
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// `%LOCALAPPDATA%\Mode\browser-import\channel.json`（可用
/// `MODE_BROWSER_IMPORT_CHANNEL` 覆盖，供测试与诊断使用）。
pub fn channel_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os("MODE_BROWSER_IMPORT_CHANNEL") {
        if !explicit.is_empty() {
            return PathBuf::from(explicit);
        }
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Mode")
        .join("browser-import")
        .join("channel.json")
}

/// main 写出的握手信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ChannelInfo {
    /// 协议版本（当前 1）。
    pub v: u32,
    /// Windows 命名管道名（`\\.\pipe\...`）。
    pub pipe: String,
    /// 一次性 token。
    pub token: String,
    /// Unix 毫秒；过期后 host 必须拒绝。
    pub expires_at: u64,
}

/// 读取并校验 channel.json。
pub fn load_channel_info(path: &Path) -> Result<ChannelInfo, HostError> {
    let bytes = std::fs::read(path).map_err(|_| HostError::Channel("channel_missing"))?;
    let info: ChannelInfo =
        serde_json::from_slice(&bytes).map_err(|_| HostError::Channel("channel_missing"))?;
    validate_channel_info(&info, now_ms())?;
    Ok(info)
}

fn validate_channel_info(info: &ChannelInfo, now: u64) -> Result<(), HostError> {
    if info.v != 1 {
        return Err(HostError::Channel("channel_missing"));
    }
    if !info.pipe.starts_with("\\\\.\\pipe\\") || info.pipe.len() <= "\\\\.\\pipe\\".len() {
        return Err(HostError::Channel("channel_missing"));
    }
    if info.token.len() < 16 || info.token.len() > 256 {
        return Err(HostError::Channel("channel_missing"));
    }
    if info.expires_at <= now {
        return Err(HostError::Channel("channel_missing"));
    }
    Ok(())
}

/// 把已校验的 payload 交给 Mode main，返回 main 采纳的条数。
pub fn deliver(info: &ChannelInfo, payload: &[u8], timeout: Duration) -> Result<usize, HostError> {
    let stream = connect(&info.pipe)?;
    let owned_info = info.clone();
    let owned_payload = payload.to_vec();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(handshake(stream, &owned_info, &owned_payload));
    });
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err(HostError::Channel("channel_timeout")),
    }
}

#[cfg(windows)]
fn connect(pipe: &str) -> Result<std::fs::File, HostError> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe)
        .map_err(|_| HostError::Channel("channel_missing"))
}

#[cfg(not(windows))]
fn connect(_pipe: &str) -> Result<std::fs::File, HostError> {
    // 浏览器数据导入是 Windows 专有路径；其他平台不提供通道。
    Err(HostError::Channel("unsupported_platform"))
}

/// 握手 + 交付；抽成 `Read + Write` 便于单测用假流。
pub fn handshake<S: Read + Write>(
    mut stream: S,
    info: &ChannelInfo,
    payload: &[u8],
) -> Result<usize, HostError> {
    let hello = serde_json::to_vec(&json!({
        "v": 1,
        "kind": "hello",
        "token": info.token,
    }))?;
    write_message(&mut stream, &hello)?;
    require_ok(read_response(&mut stream)?)?;

    write_message(&mut stream, payload)?;
    let ack = read_response(&mut stream)?;
    require_ok(ack.clone())?;
    ack.get("count")
        .and_then(Value::as_u64)
        .map(|count| count as usize)
        .ok_or(HostError::InvalidPayload("invalid_response"))
}

fn read_response<S: Read>(stream: &mut S) -> Result<Value, HostError> {
    let payload = read_message(stream)?;
    serde_json::from_slice(&payload).map_err(|_| HostError::InvalidPayload("invalid_response"))
}

fn require_ok(response: Value) -> Result<(), HostError> {
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let code = response
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("invalid_response");
    Err(match code {
        "channel_token_mismatch" => HostError::Channel("channel_token_mismatch"),
        "channel_timeout" => HostError::Channel("channel_timeout"),
        "payload_too_large" => HostError::Channel("payload_too_large"),
        "invalid_payload" => HostError::Channel("invalid_payload"),
        _ => HostError::InvalidPayload("invalid_response"),
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn framed(value: Value) -> Vec<u8> {
        let payload = serde_json::to_vec(&value).expect("json");
        let mut frame = (payload.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(&payload);
        frame
    }

    fn info() -> ChannelInfo {
        ChannelInfo {
            v: 1,
            pipe: "\\\\.\\pipe\\mode-browser-import-test".to_string(),
            token: "0123456789abcdef0123456789abcdef".to_string(),
            expires_at: now_ms() + 60_000,
        }
    }

    #[test]
    fn channel_info_is_validated() {
        assert!(validate_channel_info(&info(), now_ms()).is_ok());
        let mut expired = info();
        expired.expires_at = 1;
        assert!(matches!(
            validate_channel_info(&expired, now_ms()),
            Err(HostError::Channel("channel_missing"))
        ));
        let mut bad_pipe = info();
        bad_pipe.pipe = "C:\\temp\\pipe".to_string();
        assert!(matches!(
            validate_channel_info(&bad_pipe, now_ms()),
            Err(HostError::Channel("channel_missing"))
        ));
        let mut short = info();
        short.token = "short".to_string();
        assert!(matches!(
            validate_channel_info(&short, now_ms()),
            Err(HostError::Channel("channel_missing"))
        ));
    }

    #[test]
    fn handshake_sends_hello_and_payload() {
        let mut input = framed(json!({ "ok": true })).clone();
        input.extend(framed(json!({ "ok": true, "count": 3 })));
        let mut duplex = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        let payload = br#"{"v":1,"kind":"cookies","cookies":[]}"#.to_vec();
        let count = handshake(&mut duplex, &info(), &payload).expect("handshake");
        assert_eq!(count, 3);

        let mut requests = Cursor::new(duplex.output);
        let hello: Value =
            serde_json::from_slice(&read_message(&mut requests).expect("hello")).expect("json");
        assert_eq!(hello["kind"], "hello");
        assert_eq!(hello["token"], info().token);
        let sent = read_message(&mut requests).expect("payload");
        assert_eq!(sent, payload);
    }

    #[test]
    fn token_mismatch_maps_to_channel_error() {
        let input = framed(json!({ "ok": false, "error": "channel_token_mismatch" }));
        let mut duplex = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        assert!(matches!(
            handshake(&mut duplex, &info(), b"{}"),
            Err(HostError::Channel("channel_token_mismatch"))
        ));
    }

    #[test]
    fn malformed_ready_is_invalid_response() {
        let input = framed(json!({ "ok": false, "error": "something_else" }));
        let mut duplex = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        assert!(matches!(
            handshake(&mut duplex, &info(), b"{}"),
            Err(HostError::InvalidPayload("invalid_response"))
        ));
    }

    #[test]
    fn missing_count_is_invalid_response() {
        let mut input = framed(json!({ "ok": true })).clone();
        input.extend(framed(json!({ "ok": true })));
        let mut duplex = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        assert!(matches!(
            handshake(&mut duplex, &info(), b"{}"),
            Err(HostError::InvalidPayload("invalid_response"))
        ));
    }
}
