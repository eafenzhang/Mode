//! 文件下载与 AES-256-CBC 解密（spec §6.6；SDK :141-171, :841-878 的对齐）。

use std::time::Duration;

use aes::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
use base64::Engine;

pub type BoxFuture<T> = futures_util::future::BoxFuture<'static, T>;

/// 下载结果：原始字节 + `Content-Disposition` 文件名。
pub type DownloadResult = Result<(Vec<u8>, Option<String>), String>;

/// 下载抽象：测试可注入假实现。
pub trait FileDownloader: Send + Sync {
    fn download(&self, url: String) -> BoxFuture<DownloadResult>;
}

/// reqwest 生产实现（10s 超时，SDK :129）。
#[derive(Clone)]
pub struct ReqwestFileDownloader {
    client: reqwest::Client,
}

impl ReqwestFileDownloader {
    pub fn new() -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self { client })
    }
}

impl FileDownloader for ReqwestFileDownloader {
    fn download(&self, url: String) -> BoxFuture<Result<(Vec<u8>, Option<String>), String>> {
        let client = self.client.clone();
        Box::pin(async move {
            let response = client
                .get(url)
                .send()
                .await
                .map_err(|error| error.to_string())?;
            if !response.status().is_success() {
                return Err(format!("HTTP {}", response.status()));
            }
            let filename = response
                .headers()
                .get("content-disposition")
                .and_then(|value| value.to_str().ok())
                .and_then(parse_filename);
            let data = response.bytes().await.map_err(|error| error.to_string())?;
            Ok((data.to_vec(), filename))
        })
    }
}

/// `Content-Disposition` 文件名解析（SDK :148-163）：优先 `filename*=UTF-8''x`，
/// 其次 `filename="x"` / `filename=x`；均做百分号解码。
pub fn parse_filename(header: &str) -> Option<String> {
    let lower = header.to_ascii_lowercase();
    let utf8_marker = "filename*=utf-8''";
    if let Some(index) = lower.find(utf8_marker) {
        let rest = &header[index + utf8_marker.len()..];
        let value = rest.split([';', ' ']).next().unwrap_or_default();
        if !value.is_empty() {
            return Some(percent_decode(value));
        }
    }
    let marker = "filename=";
    if let Some(index) = lower.find(marker) {
        let rest = &header[index + marker.len()..];
        let value = rest.trim_start().trim_start_matches('"');
        let value = value.split(['"', ';', ' ']).next().unwrap_or_default();
        if !value.is_empty() {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// AES-256-CBC 解密（SDK :841-878）：key = Base64 32 字节，IV = key 前 16 字节；
/// 关闭自动 padding，手动去 PKCS#7（块大小 **32**）。
pub fn decrypt_file(data: &[u8], aes_key_base64: &str) -> Result<Vec<u8>, String> {
    let key = base64::engine::general_purpose::STANDARD
        .decode(aes_key_base64.trim())
        .map_err(|error| format!("invalid aesKey: {error}"))?;
    if key.len() != 32 {
        return Err(format!(
            "invalid aesKey (expected 32 bytes, got {})",
            key.len()
        ));
    }
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return Err("invalid ciphertext length (expected multiple of 16)".into());
    }
    let cipher = aes::Aes256::new(GenericArray::from_slice(&key));
    let iv = &key[..16];
    let mut previous: [u8; 16] = iv.try_into().expect("16 字节 IV");
    let mut plaintext = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        let ciphertext_block = block;
        cipher.decrypt_block(GenericArray::from_mut_slice(&mut block));
        for (index, byte) in block.iter_mut().enumerate() {
            *byte ^= previous[index];
        }
        previous = ciphertext_block;
        plaintext.extend_from_slice(&block);
    }
    // PKCS#7：块大小 32（SDK :855-873 的 manual unpad）。
    let pad = *plaintext.last().ok_or("invalid pkcs7 payload")?;
    if pad == 0 || pad > 32 || pad as usize > plaintext.len() {
        return Err(format!("Invalid PKCS#7 padding value: {pad}"));
    }
    let body_len = plaintext.len() - pad as usize;
    if plaintext[body_len..].iter().any(|byte| *byte != pad) {
        return Err("Invalid PKCS#7 padding: padding bytes mismatch".into());
    }
    plaintext.truncate(body_len);
    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncrypt;

    #[test]
    fn parses_content_disposition_variants() {
        assert_eq!(
            parse_filename("attachment; filename*=UTF-8''%E4%B8%AD%E6%96%87.txt"),
            Some("中文.txt".into())
        );
        assert_eq!(
            parse_filename("attachment; filename=\"report.pdf\""),
            Some("report.pdf".into())
        );
        assert_eq!(
            parse_filename("attachment; filename=plain.bin"),
            Some("plain.bin".into())
        );
        assert_eq!(parse_filename("inline"), None);
    }

    fn encrypt_for_test(plaintext: &[u8]) -> (Vec<u8>, String) {
        // 构造 32 字节块的 PKCS#7 密文（测试侧实现加密，生产只解密）。
        let key: Vec<u8> = (0u8..32).collect();
        let iv = &key[..16];
        let pad = 32 - (plaintext.len() % 32);
        let mut padded = plaintext.to_vec();
        padded.extend(std::iter::repeat_n(pad as u8, pad));
        let cipher = aes::Aes256::new(GenericArray::from_slice(&key));
        let mut previous: [u8; 16] = iv.try_into().unwrap();
        let mut ciphertext = Vec::with_capacity(padded.len());
        for chunk in padded.chunks(16) {
            let mut block = [0u8; 16];
            block.copy_from_slice(chunk);
            for (index, byte) in block.iter_mut().enumerate() {
                *byte ^= previous[index];
            }
            cipher.encrypt_block(GenericArray::from_mut_slice(&mut block));
            previous = block;
            ciphertext.extend_from_slice(&block);
        }
        (
            ciphertext,
            base64::engine::general_purpose::STANDARD.encode(&key),
        )
    }

    #[test]
    fn decrypts_round_trip_with_32_byte_padding() {
        let (ciphertext, key) = encrypt_for_test(b"hello encrypted file");
        assert_eq!(
            decrypt_file(&ciphertext, &key).unwrap(),
            b"hello encrypted file"
        );
        // 32 的整数倍也要有整块 padding。
        let block = [7u8; 32];
        let (ciphertext, key) = encrypt_for_test(&block);
        assert_eq!(decrypt_file(&ciphertext, &key).unwrap(), block);
    }

    #[test]
    fn rejects_invalid_key_and_padding() {
        let (ciphertext, _key) = encrypt_for_test(b"x");
        assert!(decrypt_file(&ciphertext, "not-base64!!").is_err());
        let short_key = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert!(decrypt_file(&ciphertext, &short_key).is_err());
        assert!(decrypt_file(
            b"short",
            &base64::engine::general_purpose::STANDARD.encode([0u8; 32])
        )
        .is_err());
    }
}
