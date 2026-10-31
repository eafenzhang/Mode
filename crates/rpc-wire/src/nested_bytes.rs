//! 嵌套 `Uint8Array` 标记的共享辅助。
//!
//! TS 的 Object 分支把对象内嵌的 `Uint8Array` 替换为
//! `{"__mode_rpc_nested_uint8array_v1":true,"base64":"…"}`（恰好两个键），
//! 反序列化时按同一判定还原。
//!
//! Rust 的 `Value` 无法承载「对象里嵌套字节」，因此 rpc-wire **不做透明还原**：
//! 适配器对已知二进制字段用 [`from_marker`] / [`to_marker`] 显式编解码。
//! 详见 `docs/specs/rpc-wire-format.md` 的「Object 分支的 JSON 细节」。

use serde_json::Value;

pub const MARKER_KEY: &str = "__mode_rpc_nested_uint8array_v1";
pub const BASE64_KEY: &str = "base64";

/// 与 TS `isRpcEncodedUint8Array` 同口径：对象、恰好两个键、标记为 true、base64 字符串。
pub fn is_marker(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == 2
        && object.get(MARKER_KEY) == Some(&Value::Bool(true))
        && object
            .get(BASE64_KEY)
            .map(Value::is_string)
            .unwrap_or(false)
}

/// 从标记对象取出原始字节；不是标记或 base64 非法时返回 `None`。
pub fn from_marker(value: &Value) -> Option<Vec<u8>> {
    if !is_marker(value) {
        return None;
    }
    let encoded = value.get(BASE64_KEY)?.as_str()?;
    base64_decode(encoded)
}

/// 构造标记对象（编码方向）。
pub fn to_marker(bytes: &[u8]) -> Value {
    serde_json::json!({
        MARKER_KEY: true,
        BASE64_KEY: base64_encode(bytes),
    })
}

pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 标准 base64 解码（忽略 `=` 与空白；非法字符返回 `None`）。
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (index, byte) in ALPHABET.iter().enumerate() {
        lookup[*byte as usize] = index as u8;
    }
    let mut output = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in text.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = lookup[byte as usize];
        if value == 255 {
            return None;
        }
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_round_trips_and_rejects_other_shapes() {
        for bytes in [
            Vec::new(),
            vec![1],
            vec![1, 2],
            vec![1, 2, 3],
            (0..=255).collect(),
        ] {
            let marker = to_marker(&bytes);
            assert!(is_marker(&marker));
            assert_eq!(from_marker(&marker).as_deref(), Some(bytes.as_slice()));
        }
        assert!(!is_marker(&serde_json::json!({})));
        assert!(!is_marker(&serde_json::json!({
            "__mode_rpc_nested_uint8array_v1": true,
            "base64": "AA==",
            "extra": 1
        })));
        assert!(!is_marker(&serde_json::json!({
            "__mode_rpc_nested_uint8array_v1": false,
            "base64": "AA=="
        })));
        assert!(!is_marker(&serde_json::json!({
            "__mode_rpc_nested_uint8array_v1": true,
            "base64": 5
        })));
    }

    #[test]
    fn value_bytes_encode_as_marker() {
        let json = crate::Value::Bytes(vec![0, 1, 2]).to_json();
        assert_eq!(
            json,
            serde_json::json!({
                "__mode_rpc_nested_uint8array_v1": true,
                "base64": "AAEC"
            })
        );
        assert_eq!(from_marker(&json).as_deref(), Some([0u8, 1, 2].as_slice()));
    }

    #[test]
    fn invalid_base64_is_rejected() {
        let marker = serde_json::json!({
            "__mode_rpc_nested_uint8array_v1": true,
            "base64": "!!!"
        });
        assert!(is_marker(&marker));
        assert!(from_marker(&marker).is_none());
    }
}
