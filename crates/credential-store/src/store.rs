//! `credentials.json` 的读写：整文件 read-modify-write + 跨进程锁 + 损坏隔离。
//!
//! 行为真源 `packages/services/src/credential/credentialService.ts`：
//! - 读取时把坏 JSON **备份后报错**，禁止当空表继续写（会清空其它登录凭据）；
//! - `save`/`delete` 的锁必须覆盖「读 → 改 → 原子替换」全过程；
//! - `list` 只看键名，不解密，避免单条旧密文连坐展示性枚举。

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atomic_file::{acquire_file_lock, write_private_text_atomic, AtomicError, WriteOptions};
use sha2::{Digest, Sha256};

use crate::cipher::{Cipher, CipherError};

pub const CREDENTIALS_FILE_NAME: &str = "credentials.json";
pub const CREDENTIAL_DECRYPT_ERROR_CODE: &str = "MODE_CREDENTIAL_DECRYPT_FAILED";
pub const CREDENTIAL_DECRYPT_ERROR_PREFIX: &str = "凭据解密失败：";

/// 坏文件告警的接收方（壳侧接 `shell_log`；库本身不决定日志落点）。
pub type CorruptListener = Arc<dyn Fn(&Path, Option<&Path>) + Send + Sync>;

#[derive(Debug)]
pub enum CredentialError {
    /// TS 的 `credentialKeySchema = nonEmptyStringSchema`：trim 后仍为空。
    KeyInvalid,
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Corrupt {
        path: PathBuf,
        backup: Option<PathBuf>,
        detail: String,
    },
    Decrypt {
        reason: &'static str,
    },
    Write {
        path: PathBuf,
        source: AtomicError,
    },
}

impl CredentialError {
    /// 只有解密失败带 TS 的 `code`；其余错误在 TS 里也是普通 `Error`。
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Decrypt { .. } => Some(CREDENTIAL_DECRYPT_ERROR_CODE),
            _ => None,
        }
    }
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyInvalid => write!(formatter, "credential key must be a non-empty string"),
            Self::Read { path, source } => {
                write!(formatter, "Unable to read Mode credentials: {} ({source})", path.display())
            }
            Self::Corrupt { path, .. } => {
                write!(formatter, "Mode credentials are corrupt: {}", path.display())
            }
            Self::Decrypt { reason } => write!(formatter, "{CREDENTIAL_DECRYPT_ERROR_PREFIX}{reason}"),
            Self::Write { path, source } => write!(
                formatter,
                "Unable to write Mode credentials: {} ({source})",
                path.display()
            ),
        }
    }
}

impl std::error::Error for CredentialError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Write { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct CredentialStore {
    file: PathBuf,
    cipher: Cipher,
    options: WriteOptions,
    on_corrupt: Option<CorruptListener>,
}

impl CredentialStore {
    /// `data_root` 是 `{dataBaseDir}/.mode`（壳侧 `resolve_data_root` 的结果），
    /// 自己拼 `v2`，与 conversation-mirror 的约定一致。
    pub fn new(data_root: impl AsRef<Path>) -> Self {
        Self::with_cipher(
            data_root.as_ref().join("v2"),
            Cipher::from_process_env(),
        )
    }

    pub fn with_secret(config_dir: impl AsRef<Path>, secret: &str) -> Self {
        Self::with_cipher(config_dir, Cipher::from_secret(secret))
    }

    pub fn with_cipher(config_dir: impl AsRef<Path>, cipher: Cipher) -> Self {
        Self {
            file: config_dir.as_ref().join(CREDENTIALS_FILE_NAME),
            cipher,
            options: WriteOptions::default(),
            on_corrupt: None,
        }
    }

    /// 坏文件告警：只传路径，不传内容。
    pub fn with_corrupt_listener(mut self, listener: CorruptListener) -> Self {
        self.on_corrupt = Some(listener);
        self
    }

    pub fn config_file(&self) -> &Path {
        &self.file
    }

    pub fn load(&self, key: &str) -> Result<Option<String>, CredentialError> {
        let key = validate_key(key)?;
        let entries = self.read_all()?;
        match entries.get(&key) {
            Some(value) => self
                .cipher
                .decrypt(value)
                .map(Some)
                .map_err(decrypt_error),
            None => Ok(None),
        }
    }

    pub fn save(&self, key: &str, value: &str) -> Result<(), CredentialError> {
        let key = validate_key(key)?;
        // 与 TS 一致：先加密（不占锁），锁只覆盖文件读写。
        let encrypted = self.cipher.encrypt(value).map_err(decrypt_error)?;
        self.update(|entries| {
            entries.insert(key, encrypted);
        })
    }

    pub fn delete(&self, key: &str) -> Result<(), CredentialError> {
        let key = validate_key(key)?;
        self.update(|entries| {
            entries.remove(&key);
        })
    }

    /// 只读键名、不解密，键名按升序返回。
    pub fn list(&self, prefix: &str) -> Result<Vec<String>, CredentialError> {
        let entries = self.read_all()?;
        Ok(entries
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }

    fn update<F>(&self, mutate: F) -> Result<(), CredentialError>
    where
        F: FnOnce(&mut BTreeMap<String, String>),
    {
        self.ensure_parent()?;
        // 锁必须覆盖读和写：Desktop Host 与 CLI 是独立进程，进程内排队挡不住丢更新。
        let _lock = acquire_file_lock(&self.file, &self.options).map_err(|source| {
            CredentialError::Write {
                path: self.file.clone(),
                source,
            }
        })?;
        let mut entries = self.read_all()?;
        mutate(&mut entries);
        self.write_all(&entries)
    }

    fn ensure_parent(&self) -> Result<(), CredentialError> {
        let Some(parent) = self.file.parent() else {
            return Ok(());
        };
        fs::create_dir_all(parent).map_err(|source| CredentialError::Write {
            path: self.file.clone(),
            source: AtomicError::Io(source),
        })
    }

    fn read_all(&self) -> Result<BTreeMap<String, String>, CredentialError> {
        let raw = match fs::read(&self.file) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(source) => {
                return Err(CredentialError::Read {
                    path: self.file.clone(),
                    source,
                })
            }
        };

        match parse_entries(&raw) {
            Ok(entries) => Ok(entries),
            Err(detail) => {
                // 损坏 JSON 若被当成空表继续 save，会清空其它 OAuth 与登录凭据。
                // 先按内容哈希留一份排他备份，再向上抛错，绝不自动覆盖。
                let backup = backup_corrupt_file(&self.file).ok();
                if let Some(listener) = &self.on_corrupt {
                    listener(&self.file, backup.as_deref());
                }
                Err(CredentialError::Corrupt {
                    path: self.file.clone(),
                    backup,
                    detail,
                })
            }
        }
    }

    fn write_all(&self, entries: &BTreeMap<String, String>) -> Result<(), CredentialError> {
        let object = entries
            .iter()
            .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
            .collect::<serde_json::Map<_, _>>();
        let mut text = serde_json::to_string_pretty(&serde_json::Value::Object(object)).map_err(
            |error| CredentialError::Write {
                path: self.file.clone(),
                source: AtomicError::Io(std::io::Error::new(ErrorKind::InvalidData, error)),
            },
        )?;
        // TS 写的是 `JSON.stringify(data, null, 2) + "\n"`。
        text.push('\n');

        let mut options = self.options.clone();
        // 外层已经持锁，这里再取同一把目录锁会自旋到超时。
        options.use_file_lock = false;
        write_private_text_atomic(&self.file, &text, &options).map_err(|source| {
            CredentialError::Write {
                path: self.file.clone(),
                source,
            }
        })
    }
}

fn decrypt_error(error: CipherError) -> CredentialError {
    CredentialError::Decrypt {
        reason: error.reason(),
    }
}

/// `credentialKeySchema = z.string().trim().min(1)`：校验的是 trim 之后的值，
/// 调用方传入 `"  key  "` 时存取都用 `"key"`。
fn validate_key(key: &str) -> Result<String, CredentialError> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err(CredentialError::KeyInvalid);
    }
    Ok(trimmed.to_string())
}

fn parse_entries(raw: &str) -> Result<BTreeMap<String, String>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("invalid JSON: {error}"))?;
    let serde_json::Value::Object(map) = value else {
        return Err("root is not a JSON object".to_string());
    };
    let mut entries = BTreeMap::new();
    for (key, item) in map {
        // 对应 `credentialRecordSchema = z.record(z.string(), z.string())`。
        let serde_json::Value::String(text) = item else {
            return Err(format!("value for `{key}` is not a string"));
        };
        entries.insert(key, text);
    }
    Ok(entries)
}

/// 备份路径 `<file>.corrupt-<sha256 前 24 位>.bak`，排他创建且权限 0600。
fn backup_corrupt_file(file: &Path) -> std::io::Result<PathBuf> {
    let content = fs::read(file)?;
    let digest = Sha256::digest(&content);
    let content_id = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut name = file.as_os_str().to_os_string();
    name.push(format!(".corrupt-{}.bak", &content_id[..24]));
    let backup = PathBuf::from(name);

    match create_private_new(&backup, &content) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&backup, fs::Permissions::from_mode(0o600));
    }
    Ok(backup)
}

fn create_private_new(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(content)
}
