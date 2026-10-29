//! base64url（无 padding），与 Node `Buffer.toString("base64url")` 同形。

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// 编码结果**不带** `=`：Node 的 base64url 同样省略 padding。
pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        }
    }
    out
}

/// 解码同时接受 base64url 与标准 base64 字母表（Node 的解码器同样宽容），
/// `=` 之后的内容一律忽略。
pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;

    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        } as u32;

        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
            buffer &= (1 << bits) - 1;
        }
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_without_padding() {
        for input in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            let encoded = encode(input);
            assert!(!encoded.contains('='), "base64url 不应带 padding: {encoded}");
            assert_eq!(decode(&encoded).unwrap(), input);
        }
    }

    #[test]
    fn matches_node_base64url_for_binary_payloads() {
        // Node: Buffer.from([0xfb, 0xff, 0xbf]).toString("base64url") === "-_-_"
        assert_eq!(encode(&[0xfb, 0xff, 0xbf]), "-_-_");
        assert_eq!(decode("-_-_").unwrap(), vec![0xfb, 0xff, 0xbf]);
    }

    #[test]
    fn rejects_non_alphabet_characters() {
        assert!(decode("ab*").is_none());
    }

    #[test]
    fn accepts_padding_and_standard_alphabet() {
        assert_eq!(decode("--__==").unwrap(), vec![0xfb, 0xef, 0xff]);
    }
}
