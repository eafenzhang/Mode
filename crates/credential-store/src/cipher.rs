//! 凭据密文：AES-256-GCM + `enc:v1:` 文本格式。
//!
//! 行为真源 `packages/services/src/credential/providers/credentialCipherProvider.ts`。
//! 密钥派生与 fallback 拼接必须与 Node **逐字节一致**：只要差一个字符，升级后旧凭据
//! 全部解不开（见 `docs/specs/credential-store.md`）。

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};

use crate::base64url;

pub const ENCRYPTED_VALUE_PREFIX: &str = "enc:v1:";
pub const CREDENTIAL_SECRET_ENV_KEY: &str = "MODE_CREDENTIAL_SECRET";

const IV_BYTES: usize = 12;
const AUTH_TAG_BYTES: usize = 16;

/// 解密失败原因；`reason` 直接拼进 TS 的错误消息前缀。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherError {
    /// 不以 `enc:v1:` 开头的值会原样返回，因此这里只表示「有前缀但结构不合法」。
    Format,
    IvLength,
    AuthTagLength,
    /// 密钥不匹配或密文被篡改。
    Decrypt,
    /// 系统随机源不可用（正常环境不会发生）。
    Random,
}

impl CipherError {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Format => "密文格式非法",
            Self::IvLength => "IV 长度非法",
            Self::AuthTagLength => "AuthTag 长度非法",
            Self::Decrypt => "密钥不匹配或密文已损坏",
            Self::Random => "随机数生成失败",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Cipher {
    key: [u8; 32],
}

impl Cipher {
    pub fn from_secret(secret: &str) -> Self {
        Self {
            key: derive_key(secret),
        }
    }

    /// 按进程环境解析 secret：`MODE_CREDENTIAL_SECRET` 优先，否则用 fallback 串。
    pub fn from_process_env() -> Self {
        let explicit = std::env::var(CREDENTIAL_SECRET_ENV_KEY).ok();
        Self::from_secret(&resolve_secret(
            explicit.as_deref(),
            node_platform(),
            &node_homedir(),
            &node_username(),
        ))
    }

    pub fn encrypt(&self, value: &str) -> Result<String, CipherError> {
        let mut iv = [0u8; IV_BYTES];
        SystemRandom::new()
            .fill(&mut iv)
            .map_err(|_| CipherError::Random)?;
        Ok(self.encrypt_with_nonce(value, &iv))
    }

    /// 固定 nonce 只供对拍/测试；生产路径必须用 [`Self::encrypt`]。
    #[doc(hidden)]
    pub fn encrypt_with_nonce(&self, value: &str, iv: &[u8; IV_BYTES]) -> String {
        let key = self.less_safe_key();
        let mut payload = value.as_bytes().to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(*iv),
            Aad::empty(),
            &mut payload,
        )
        .expect("AES-256-GCM 密封只在随机数/缓冲区异常时失败");

        let tag_start = payload.len() - AUTH_TAG_BYTES;
        let (cipher_text, auth_tag) = payload.split_at(tag_start);
        format!(
            "{ENCRYPTED_VALUE_PREFIX}{}.{}.{}",
            base64url::encode(iv),
            base64url::encode(auth_tag),
            base64url::encode(cipher_text)
        )
    }

    pub fn decrypt(&self, value: &str) -> Result<String, CipherError> {
        let Some(payload) = value.strip_prefix(ENCRYPTED_VALUE_PREFIX) else {
            // 历史明文条目：TS 同样原样返回。
            return Ok(value.to_string());
        };

        let parts: Vec<&str> = payload.split('.').collect();
        if parts.len() != 3 {
            return Err(CipherError::Format);
        }
        // TS 用 `!ivRaw || !authTagRaw || !cipherRaw` 判空：空明文的密文段是空串，
        // 因此「加密空字符串再读回」在 TS 里同样是格式错误，这里保持同样的行为。
        if parts.iter().any(|part| part.is_empty()) {
            return Err(CipherError::Format);
        }

        let iv = base64url::decode(parts[0]).ok_or(CipherError::Format)?;
        let auth_tag = base64url::decode(parts[1]).ok_or(CipherError::Format)?;
        let cipher_text = base64url::decode(parts[2]).ok_or(CipherError::Format)?;

        if iv.len() != IV_BYTES {
            return Err(CipherError::IvLength);
        }
        if auth_tag.len() != AUTH_TAG_BYTES {
            return Err(CipherError::AuthTagLength);
        }

        let mut iv_bytes = [0u8; IV_BYTES];
        iv_bytes.copy_from_slice(&iv);
        let mut payload = cipher_text;
        payload.extend_from_slice(&auth_tag);

        let key = self.less_safe_key();
        let plain = key
            .open_in_place(
                Nonce::assume_unique_for_key(iv_bytes),
                Aad::empty(),
                &mut payload,
            )
            .map_err(|_| CipherError::Decrypt)?;
        Ok(String::from_utf8_lossy(plain).into_owned())
    }

    fn less_safe_key(&self) -> LessSafeKey {
        let unbound = UnboundKey::new(&AES_256_GCM, &self.key)
            .expect("AES-256-GCM 只接受 32 字节密钥");
        LessSafeKey::new(unbound)
    }
}

fn derive_key(secret: &str) -> [u8; 32] {
    let digest = Sha256::digest(secret.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

/// `mode-credential-fallback:{platform}:{homedir}:{username}`（与 TS 模板一致）。
pub fn fallback_secret(platform: &str, homedir: &str, username: &str) -> String {
    format!("mode-credential-fallback:{platform}:{homedir}:{username}")
}

/// 显式 secret 为空串时按「未设置」处理（TS 用真值判断）。
pub fn resolve_secret(
    explicit: Option<&str>,
    platform: &str,
    homedir: &str,
    username: &str,
) -> String {
    match explicit {
        Some(secret) if !secret.is_empty() => secret.to_string(),
        _ => fallback_secret(platform, homedir, username),
    }
}

/// `process.platform` 的取值（Rust 的 `windows`/`macos` 必须映射）。
fn node_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

/// `os.homedir()`：Windows 先 `USERPROFILE` 再 `HOMEDRIVE`+`HOMEPATH`；Unix 用 `HOME`。
fn node_homedir() -> String {
    #[cfg(windows)]
    {
        if let Some(profile) = env_non_empty("USERPROFILE") {
            return profile;
        }
        if let (Some(drive), Some(path)) = (env_non_empty("HOMEDRIVE"), env_non_empty("HOMEPATH"))
        {
            return format!("{drive}{path}");
        }
    }
    #[cfg(unix)]
    {
        if let Some(home) = env_non_empty("HOME") {
            return home;
        }
    }
    String::new()
}

/// `os.userInfo().username`：取不到时与 TS 一样回落 `"unknown"`。
fn node_username() -> String {
    system_username().unwrap_or_else(|| "unknown".to_string())
}

#[cfg(windows)]
fn system_username() -> Option<String> {
    use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;

    let mut size: u32 = 256;
    loop {
        let mut buffer = vec![0u16; size as usize];
        let mut used = size;
        let ok = unsafe { GetUserNameW(buffer.as_mut_ptr(), &mut used) };
        if ok != 0 {
            let len = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
            return Some(String::from_utf16_lossy(&buffer[..len]));
        }
        if used <= size {
            return None;
        }
        size = used;
    }
}

#[cfg(unix)]
fn system_username() -> Option<String> {
    use std::ffi::CStr;

    let uid = unsafe { libc::geteuid() };
    let mut buffer_size = 1024usize;
    loop {
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let mut buffer = vec![0 as libc::c_char; buffer_size];
        let code = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut result,
            )
        };
        if code == libc::ERANGE {
            if buffer_size >= 64 * 1024 {
                return None;
            }
            buffer_size *= 2;
            continue;
        }
        if code != 0 || result.is_null() || entry.pw_name.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(entry.pw_name) };
        return name.to_str().ok().map(str::to_string);
    }
}

#[cfg(not(any(windows, unix)))]
fn system_username() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_secret_matches_ts_template() {
        assert_eq!(
            fallback_secret("win32", "C:\\Users\\alice", "alice"),
            "mode-credential-fallback:win32:C:\\Users\\alice:alice"
        );
        assert_eq!(
            fallback_secret("darwin", "/Users/alice", "alice"),
            "mode-credential-fallback:darwin:/Users/alice:alice"
        );
    }

    #[test]
    fn explicit_secret_wins_only_when_non_empty() {
        assert_eq!(resolve_secret(Some("s"), "linux", "/h", "u"), "s");
        assert_eq!(
            resolve_secret(Some(""), "linux", "/h", "u"),
            fallback_secret("linux", "/h", "u")
        );
        assert_eq!(
            resolve_secret(None, "linux", "/h", "u"),
            fallback_secret("linux", "/h", "u")
        );
    }

    #[test]
    fn encrypt_then_decrypt_round_trips() {
        let cipher = Cipher::from_secret("test-secret");
        for value in ["", "hello", "中文凭据", "emoji 🔐"] {
            let encrypted = cipher.encrypt(value).unwrap();
            if value.is_empty() {
                // 与 TS 一致：空明文的密文段为空串，解密会先撞上格式错误。
                assert_eq!(cipher.decrypt(&encrypted), Err(CipherError::Format));
            } else {
                assert_eq!(cipher.decrypt(&encrypted).unwrap(), value);
            }
        }
    }

    #[test]
    fn plaintext_values_pass_through() {
        let cipher = Cipher::from_secret("test-secret");
        assert_eq!(cipher.decrypt("legacy-plain").unwrap(), "legacy-plain");
        assert_eq!(cipher.decrypt("").unwrap(), "");
    }

    #[test]
    fn rejects_malformed_payloads() {
        let cipher = Cipher::from_secret("test-secret");
        assert_eq!(cipher.decrypt("enc:v1:abc"), Err(CipherError::Format));
        assert_eq!(cipher.decrypt("enc:v1:a.b"), Err(CipherError::Format));
        assert_eq!(cipher.decrypt("enc:v1:.b.c"), Err(CipherError::Format));
        // "AAAA" 解出 3 字节 IV。
        assert_eq!(
            cipher.decrypt("enc:v1:AAAA.AAAAAAAAAAAAAAAAAAAAAA.AAAA"),
            Err(CipherError::IvLength)
        );
        assert_eq!(
            cipher.decrypt("enc:v1:AAAAAAAAAAAAAAAA.AAAA.AAAA"),
            Err(CipherError::AuthTagLength)
        );
    }

    #[test]
    fn wrong_key_fails_authentication() {
        let encrypted = Cipher::from_secret("secret-a").encrypt("value").unwrap();
        assert_eq!(
            Cipher::from_secret("secret-b").decrypt(&encrypted),
            Err(CipherError::Decrypt)
        );
    }
}
