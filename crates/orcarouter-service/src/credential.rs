//! OrcaRouter 凭据存储（阶段 1）：项目加密凭据库中的单条记录。
//!
//! 规范见 `docs/specs/orcarouter-service.md`。连接目录/overlay/频道在后续阶段。

use std::sync::Arc;

use credential_store::CredentialStore;
use serde::{Deserialize, Serialize};

/// 与 TS `ORCAROUTER_CREDENTIAL_KEY` 同值。
pub const ORCAROUTER_CREDENTIAL_KEY: &str = "provider:orcarouter:api-key";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrcaCredentialSource {
    #[serde(rename = "api-key")]
    ApiKey,
    #[serde(rename = "pkce")]
    Pkce,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrcaCredential {
    pub api_key: String,
    pub source: OrcaCredentialSource,
    pub granted_scope: Option<String>,
    pub account_id: Option<String>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrcaCredentialStatus {
    pub connected: bool,
    pub masked: String,
    pub source: Option<OrcaCredentialSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub generation: u64,
    pub needs_reauth: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedOrcaCredential {
    api_key: String,
    #[serde(default)]
    source: Option<OrcaCredentialSource>,
    #[serde(default)]
    granted_scope: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    needs_reauth: bool,
    #[serde(default)]
    needs_reauth_generation: Option<u64>,
}

pub struct OrcaCredentialStore {
    credentials: Arc<CredentialStore>,
}

impl OrcaCredentialStore {
    pub fn new(credentials: Arc<CredentialStore>) -> Self {
        Self { credentials }
    }

    fn read_record(&self) -> Result<Option<PersistedOrcaCredential>, String> {
        let raw = self
            .credentials
            .load(ORCAROUTER_CREDENTIAL_KEY)
            .map_err(|error| error.to_string())?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let Ok(record) = serde_json::from_str::<PersistedOrcaCredential>(&raw) else {
            // 损坏记录按未连接处理；保留原值直到显式覆盖。
            return Ok(None);
        };
        let api_key = record.api_key.trim().to_string();
        if api_key.is_empty() {
            return Ok(None);
        }
        Ok(Some(PersistedOrcaCredential {
            api_key,
            source: record.source,
            granted_scope: record.granted_scope,
            account_id: record.account_id,
            generation: record.generation,
            needs_reauth: record.needs_reauth,
            needs_reauth_generation: record.needs_reauth_generation,
        }))
    }

    fn write_record(&self, record: &PersistedOrcaCredential) -> Result<(), String> {
        let text = serde_json::to_string(record).map_err(|error| error.to_string())?;
        self.credentials
            .save(ORCAROUTER_CREDENTIAL_KEY, &text)
            .map_err(|error| error.to_string())
    }

    pub fn read(&self) -> Result<Option<OrcaCredential>, String> {
        let record = self.read_record()?;
        Ok(record.map(|record| OrcaCredential {
            api_key: record.api_key,
            source: record.source.unwrap_or(OrcaCredentialSource::ApiKey),
            granted_scope: record.granted_scope,
            account_id: record.account_id,
            generation: record.generation,
        }))
    }

    /// `needsReauth` 的记录不返回，避免继续使用死凭据。
    pub fn load_usable(&self) -> Result<Option<OrcaCredential>, String> {
        let Some(record) = self.read_record()? else {
            return Ok(None);
        };
        if record.needs_reauth {
            return Ok(None);
        }
        Ok(Some(OrcaCredential {
            api_key: record.api_key,
            source: record.source.unwrap_or(OrcaCredentialSource::ApiKey),
            granted_scope: record.granted_scope,
            account_id: record.account_id,
            generation: record.generation,
        }))
    }

    /// 保存凭据：generation 单调递增，旧异步结果不得覆盖新登录。
    pub fn save(
        &self,
        api_key: &str,
        source: OrcaCredentialSource,
        granted_scope: Option<String>,
        account_id: Option<String>,
    ) -> Result<OrcaCredential, String> {
        let current = self.read_record()?;
        let generation = current.map(|record| record.generation).unwrap_or(0) + 1;
        let record = PersistedOrcaCredential {
            api_key: api_key.trim().to_string(),
            source: Some(source),
            granted_scope: granted_scope.clone(),
            account_id: account_id.clone(),
            generation,
            needs_reauth: false,
            needs_reauth_generation: None,
        };
        self.write_record(&record)?;
        Ok(OrcaCredential {
            api_key: record.api_key,
            source,
            granted_scope,
            account_id,
            generation,
        })
    }

    pub fn clear(&self) -> Result<(), String> {
        self.credentials
            .delete(ORCAROUTER_CREDENTIAL_KEY)
            .map_err(|error| error.to_string())
    }

    pub fn status(&self) -> Result<OrcaCredentialStatus, String> {
        let Some(record) = self.read_record()? else {
            return Ok(OrcaCredentialStatus {
                connected: false,
                masked: String::new(),
                source: None,
                account_id: None,
                generation: 0,
                needs_reauth: false,
            });
        };
        Ok(OrcaCredentialStatus {
            connected: !record.needs_reauth,
            masked: mask_orca_secret(Some(&record.api_key)),
            source: Some(record.source.unwrap_or(OrcaCredentialSource::ApiKey)),
            account_id: record.account_id,
            generation: record.generation,
            needs_reauth: record.needs_reauth,
        })
    }

    /// 只匹配发出被拒请求的精确账号 + generation；已标记同 generation 幂等。
    pub fn mark_needs_reauth(
        &self,
        generation: Option<u64>,
        account_id: Option<&str>,
    ) -> Result<bool, String> {
        let Some(current) = self.read_record()? else {
            return Ok(false);
        };
        if generation.is_some_and(|value| current.generation != value) {
            return Ok(false);
        }
        if account_id.is_some_and(|value| current.account_id.as_deref() != Some(value)) {
            return Ok(false);
        }
        if current.needs_reauth && current.needs_reauth_generation == Some(current.generation) {
            return Ok(true);
        }
        let record = PersistedOrcaCredential {
            needs_reauth: true,
            needs_reauth_generation: Some(current.generation),
            ..current
        };
        self.write_record(&record)?;
        Ok(true)
    }
}

/// 只保留前 6 + 末 4；≤8 全遮；空值空串。
pub fn mask_orca_secret(secret: Option<&str>) -> String {
    let value = secret.unwrap_or("").trim();
    if value.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "••••".to_string();
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// 只做轻量格式检查；前缀不是有效性证明。
pub fn looks_like_orca_api_key(value: &str) -> bool {
    let value = value.trim();
    let Some(rest) = value.strip_prefix("sk-orca-") else {
        return false;
    };
    rest.len() >= 6
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "orcarouter-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos() as u64)
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("dir");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store(dir: &TempDir) -> OrcaCredentialStore {
        OrcaCredentialStore::new(Arc::new(CredentialStore::with_secret(
            &dir.0,
            "orcarouter-test-secret",
        )))
    }

    #[test]
    fn save_increments_generation_and_clears_reauth() {
        let dir = TempDir::new("save");
        let store = store(&dir);
        let first = store
            .save(
                "sk-orca-abcdef123456",
                OrcaCredentialSource::ApiKey,
                None,
                None,
            )
            .expect("save");
        assert_eq!(first.generation, 1);
        assert!(store.mark_needs_reauth(Some(1), None).expect("mark"));
        assert!(store.load_usable().expect("load").is_none());
        let second = store
            .save(
                "sk-orca-zzzzzz999999",
                OrcaCredentialSource::Pkce,
                None,
                None,
            )
            .expect("save");
        assert_eq!(second.generation, 2);
        assert_eq!(
            store
                .load_usable()
                .expect("load")
                .expect("credential")
                .api_key,
            "sk-orca-zzzzzz999999"
        );
    }

    #[test]
    fn mark_needs_reauth_matches_exact_generation() {
        let dir = TempDir::new("reauth");
        let store = store(&dir);
        store
            .save(
                "sk-orca-abcdef123456",
                OrcaCredentialSource::Pkce,
                None,
                Some("acct-1".to_string()),
            )
            .expect("save");
        assert!(!store
            .mark_needs_reauth(Some(9), Some("acct-1"))
            .expect("mismatch generation"));
        assert!(!store
            .mark_needs_reauth(Some(1), Some("acct-2"))
            .expect("mismatch account"));
        assert!(store
            .mark_needs_reauth(Some(1), Some("acct-1"))
            .expect("match"));
        // 同 generation 重复标记幂等。
        assert!(store
            .mark_needs_reauth(Some(1), Some("acct-1"))
            .expect("idempotent"));
        let status = store.status().expect("status");
        assert!(!status.connected);
        assert!(status.needs_reauth);
        assert_eq!(status.account_id.as_deref(), Some("acct-1"));
    }

    #[test]
    fn status_masks_and_clear_resets() {
        let dir = TempDir::new("status");
        let store = store(&dir);
        assert_eq!(store.status().expect("status").generation, 0);
        store
            .save(
                "sk-orca-abcdef123456",
                OrcaCredentialSource::ApiKey,
                None,
                None,
            )
            .expect("save");
        let status = store.status().expect("status");
        assert!(status.connected);
        assert_eq!(status.masked, "sk-orc…3456");
        store.clear().expect("clear");
        let status = store.status().expect("status");
        assert!(!status.connected);
        assert_eq!(status.masked, "");
    }

    #[test]
    fn mask_and_format_helpers() {
        assert_eq!(mask_orca_secret(None), "");
        assert_eq!(mask_orca_secret(Some("   ")), "");
        assert_eq!(mask_orca_secret(Some("short")), "••••");
        assert_eq!(mask_orca_secret(Some("123456789")), "123456…6789");
        assert!(looks_like_orca_api_key("sk-orca-abc123"));
        assert!(!looks_like_orca_api_key("sk-orca-"));
        assert!(!looks_like_orca_api_key("sk-orca-abc 123"));
        assert!(!looks_like_orca_api_key("abc123"));
    }
}
