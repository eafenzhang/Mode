//! 与 TS `createCredentialCipherProvider` 的密文对拍。
//!
//! fixture 由 `pnpm credential:fixtures` 用真实 TS provider 生成；这里只负责解开，
//! 不允许为了通过测试去改 fixture 或放宽断言。

use credential_store::{Cipher, CipherError};

const FIXTURE: &str = include_str!("../fixtures/credential-cipher.json");

fn fixture() -> serde_json::Value {
    serde_json::from_str(FIXTURE).expect("fixture 必须是合法 JSON")
}

#[test]
fn decrypts_every_ts_ciphertext() {
    let fixture = fixture();
    let cipher = Cipher::from_secret(fixture["secret"].as_str().unwrap());
    let cases = fixture["encrypted"].as_array().unwrap();
    assert!(!cases.is_empty(), "fixture 不能为空");

    for case in cases {
        let plaintext = case["plaintext"].as_str().unwrap();
        let ciphertext = case["ciphertext"].as_str().unwrap();
        assert_eq!(
            cipher.decrypt(ciphertext).unwrap(),
            plaintext,
            "TS 密文必须能用同一 secret 解开"
        );
    }
}

#[test]
fn wrong_secret_fails_like_ts() {
    let fixture = fixture();
    let ciphertext = fixture["encrypted"][0]["ciphertext"].as_str().unwrap();
    assert_eq!(
        Cipher::from_secret("not-the-fixture-secret").decrypt(ciphertext),
        Err(CipherError::Decrypt)
    );
}

#[test]
fn empty_plaintext_matches_ts_format_error() {
    let fixture = fixture();
    let cipher = Cipher::from_secret(fixture["secret"].as_str().unwrap());
    let ciphertext = fixture["emptyPlaintextCiphertext"].as_str().unwrap();
    // TS 用 `!cipherRaw` 判空，空明文的密文段为空串 → 先报格式错误。
    assert_eq!(cipher.decrypt(ciphertext), Err(CipherError::Format));
}

#[test]
fn plaintext_passthrough_matches_ts() {
    let fixture = fixture();
    let cipher = Cipher::from_secret(fixture["secret"].as_str().unwrap());
    for value in fixture["plaintextPassThrough"].as_array().unwrap() {
        let value = value.as_str().unwrap();
        assert_eq!(cipher.decrypt(value).unwrap(), value);
    }
}

#[test]
fn invalid_payloads_match_ts_error_messages() {
    let fixture = fixture();
    let cipher = Cipher::from_secret(fixture["secret"].as_str().unwrap());
    for case in fixture["invalid"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let value = case["value"].as_str().unwrap();
        let expected = case["expectedError"].as_str().unwrap();
        let error = match cipher.decrypt(value) {
            Ok(_) => panic!("{name} 应当解密失败"),
            Err(error) => error,
        };
        assert_eq!(
            format!("凭据解密失败：{}", error.reason()),
            expected,
            "{name} 的错误消息必须与 TS 一致"
        );
    }
}

/// 加密方向不做字节对拍（IV 随机），但结构必须与 TS 完全同形：
/// `enc:v1:<16 字符 IV>.<22 字符 tag>.<base64url 密文>`。
#[test]
fn rust_ciphertext_matches_ts_structure() {
    let fixture = fixture();
    let cipher = Cipher::from_secret(fixture["secret"].as_str().unwrap());
    for plaintext in fixture["encrypted"].as_array().unwrap() {
        let plaintext = plaintext["plaintext"].as_str().unwrap();
        let ciphertext = cipher.encrypt(plaintext).unwrap();
        let payload = ciphertext.strip_prefix("enc:v1:").expect("必须带 enc:v1: 前缀");
        let parts: Vec<&str> = payload.split('.').collect();
        assert_eq!(parts.len(), 3, "必须是三段");
        assert_eq!(parts[0].len(), 16, "12 字节 IV → 16 个 base64url 字符");
        assert_eq!(parts[1].len(), 22, "16 字节 AuthTag → 22 个 base64url 字符");
        assert!(!parts[2].is_empty(), "非空明文必须有密文段");
        assert!(
            !payload.contains('='),
            "base64url 不能带 padding: {ciphertext}"
        );
        assert_eq!(cipher.decrypt(&ciphertext).unwrap(), plaintext);
    }
}
