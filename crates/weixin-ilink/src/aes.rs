//! CDN 附件解密（TS `weixinProvider.ts:21,63-99` 的对齐）。
//!
//! 微信 iLink CDN 返回 **AES-128-ECB + PKCS7 padding** 的密文字节；key 支持三种
//! 形态（hex 32 位 / base64 16 字节 / base64 内嵌 hex 文本）。ECB 与 PKCS7 裁剪
//! 手写实现（不引 ecb 依赖）——块大小固定 16，逐块 `decrypt_block`。

use base64::Engine;
use serde_json::Value;

#[derive(Debug)]
pub enum AesError {
    /// key 三形态都不合法（TS :63-84 返回 null → 抛错）。
    InvalidKey,
    /// 密文长度不是 16 的倍数，或 PKCS7 padding 非法。
    InvalidCiphertext(&'static str),
}

impl std::fmt::Display for AesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidKey => write!(formatter, "Weixin attachment AES key is invalid."),
            Self::InvalidCiphertext(reason) => {
                write!(formatter, "Weixin attachment ciphertext invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for AesError {}

/// key 三形态解析（TS :63-84）：hex 32 位 → hex 16 字节；base64 → 16 字节；
/// base64 解出的 UTF-8 文本本身是 hex 32 位 → hex。都不满足 → None。
fn parse_key(value: &str) -> Option<[u8; 16]> {
    let trimmed = value.trim();
    let hex_decode = |hex: &str| -> Option<[u8; 16]> {
        if hex.len() != 32 {
            return None;
        }
        let bytes = hex.as_bytes();
        let mut key = [0u8; 16];
        for (index, pair) in bytes.chunks(2).enumerate() {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            key[index] = ((high << 4) | low) as u8;
        }
        Some(key)
    };
    if let Some(key) = hex_decode(trimmed) {
        return Some(key);
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .ok()?;
    if decoded.len() == 16 {
        let mut key = [0u8; 16];
        key.copy_from_slice(&decoded);
        return Some(key);
    }
    let decoded_text = String::from_utf8(decoded).ok()?.trim().to_string();
    hex_decode(&decoded_text)
}

/// AES-128-ECB + PKCS7 解密（TS :91-99）。
pub fn decrypt_cdn_media(data: &[u8], aes_key: &str) -> Result<Vec<u8>, AesError> {
    use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
    let key = parse_key(aes_key).ok_or(AesError::InvalidKey)?;
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err(AesError::InvalidCiphertext(
            "length must be a positive multiple of 16",
        ));
    }
    let cipher = aes::Aes128::new(&GenericArray::from(key));
    let mut plaintext = data.to_vec();
    for block in plaintext.chunks_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
    }
    // PKCS7：末字节为 padding 长度（1..=16），逐字节校验。
    let padding = *plaintext.last().expect("非空已保证");
    if padding == 0 || padding as usize > 16 {
        return Err(AesError::InvalidCiphertext("invalid PKCS7 padding byte"));
    }
    if plaintext.len() < padding as usize {
        return Err(AesError::InvalidCiphertext("padding exceeds plaintext"));
    }
    let body_len = plaintext.len() - padding as usize;
    if plaintext[body_len..].iter().any(|byte| *byte != padding) {
        return Err(AesError::InvalidCiphertext("inconsistent PKCS7 padding"));
    }
    plaintext.truncate(body_len);
    Ok(plaintext)
}

/// 供调用方从 CDN 附件元数据里取 aes key 的便捷读法
/// （TS downloadUrl/aesKey 挂在 `providerMetadata.weixinAesKey`）。
pub fn aes_key_from_metadata(metadata: &Value) -> Option<String> {
    metadata
        .get("weixinAesKey")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};

    /// 用同一把 key 做 ECB 加密 + PKCS7 填充，构造解密测试的密文（TS 侧由 CDN 产生）。
    fn encrypt_for_test(plaintext: &[u8], key: [u8; 16]) -> Vec<u8> {
        let cipher = aes::Aes128::new(&GenericArray::from(key));
        let padding = 16 - (plaintext.len() % 16);
        let mut data = plaintext.to_vec();
        data.extend(std::iter::repeat_n(padding as u8, padding));
        for block in data.chunks_mut(16) {
            cipher.encrypt_block(GenericArray::from_mut_slice(block));
        }
        data
    }

    fn test_key() -> [u8; 16] {
        [
            0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66,
            0x41, 0x42,
        ]
    }

    #[test]
    fn decrypts_hex_key_form() {
        // hex 32 位形态。
        let key_hex: String = test_key()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let ciphertext = encrypt_for_test(b"hello cdn", test_key());
        let plaintext = decrypt_cdn_media(&ciphertext, &key_hex).expect("解密成功");
        assert_eq!(plaintext, b"hello cdn");
    }

    #[test]
    fn decrypts_base64_key_form_and_base64_hex_text_form() {
        let key = test_key();
        let key_b64 = base64::engine::general_purpose::STANDARD.encode(key);
        let ciphertext = encrypt_for_test(b"body", key);
        assert_eq!(decrypt_cdn_media(&ciphertext, &key_b64).unwrap(), b"body");
        // base64 解出的 UTF-8 文本本身是 hex 32 位。
        let hex_text: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
        let key_b64_of_hex = base64::engine::general_purpose::STANDARD.encode(hex_text.as_bytes());
        assert_eq!(
            decrypt_cdn_media(&ciphertext, &key_b64_of_hex).unwrap(),
            b"body"
        );
    }

    #[test]
    fn rejects_invalid_key_and_ciphertext() {
        assert!(matches!(
            decrypt_cdn_media(&[0u8; 16], "not-a-key"),
            Err(AesError::InvalidKey)
        ));
        let key_hex: String = test_key()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert!(matches!(
            decrypt_cdn_media(&[1u8; 15], &key_hex),
            Err(AesError::InvalidCiphertext(_))
        ));
        assert!(matches!(
            decrypt_cdn_media(&[], &key_hex),
            Err(AesError::InvalidCiphertext(_))
        ));
    }

    #[test]
    fn metadata_reader_reads_weixin_aes_key() {
        let metadata = json!({ "weixinAesKey": "abc" });
        assert_eq!(aes_key_from_metadata(&metadata).as_deref(), Some("abc"));
        assert!(aes_key_from_metadata(&json!({})).is_none());
    }
}
