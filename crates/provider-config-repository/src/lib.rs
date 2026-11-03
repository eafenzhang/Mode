//! Personal Provider Config 仓库：读/写/锁/原子写/legacy 导入。
//!
//! 真源：`packages/provider-node/src/personal-provider-config-repository.ts` 与
//! `runtime-paths.ts`。行为契约见 `docs/specs/provider-config-repository.md`。
//!
//! 本切片不做轮询/`onDidChange`；Node 仓库在删除前始终是 oracle。

use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atomic_file::{
    acquire_file_lock, write_private_text_atomic, AtomicError, FileLock, WriteOptions,
};
use provider_config::JsonValue;
use provider_config_file_codec::{
    decode_provider_config_file, encode_provider_config_file, ProviderConfigLayerUpdate,
};
use provider_config_source::{decode_personal_document, empty_personal_layer, PersonalLayer};

pub mod runtime_paths;

pub use runtime_paths::{
    create_node_provider_runtime_path_env, resolve_node_provider_runtime_paths,
    ProviderRuntimePaths, RuntimePathError, MODE_BUILTIN_PROVIDER_BUNDLED_CONFIG_FILE_ENV,
    MODE_BUILTIN_PROVIDER_CONFIG_FILE_ENV, MODE_PERSONAL_PROVIDER_CONFIG_FILE_ENV,
    PERSONAL_PROVIDER_CONFIG_FILE_NAME,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryError {
    Io(String),
    Invalid(String),
    Lock(String),
}

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) | Self::Invalid(message) | Self::Lock(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for RepositoryError {}

impl From<AtomicError> for RepositoryError {
    fn from(error: AtomicError) -> Self {
        match error {
            AtomicError::LockTimeout {
                lock_path,
                waited_ms,
            } => Self::Lock(format!(
                "Timed out after {waited_ms}ms waiting for the Mode file lock: {}",
                lock_path.display()
            )),
            AtomicError::Io(error) => Self::Io(error.to_string()),
        }
    }
}

pub type LegacyImporter = Arc<dyn Fn() -> Option<ProviderConfigLayerUpdate> + Send + Sync>;
pub type RecoveryListener = Arc<dyn Fn(&RepositoryError) + Send + Sync>;

pub struct PersonalConfigRepositoryOptions {
    pub file_path: PathBuf,
    pub import_legacy: Option<LegacyImporter>,
    pub on_recovery: Option<RecoveryListener>,
}

pub struct PersonalConfigRepository {
    file_path: PathBuf,
    import_legacy: Option<LegacyImporter>,
    on_recovery: Option<RecoveryListener>,
}

impl PersonalConfigRepository {
    pub fn new(options: PersonalConfigRepositoryOptions) -> Result<Self, RepositoryError> {
        if options.file_path.to_string_lossy().trim().is_empty() {
            return Err(RepositoryError::Invalid(
                "Personal Provider Config filePath 不能为空".to_string(),
            ));
        }
        Ok(Self {
            file_path: options.file_path,
            import_legacy: options.import_legacy,
            on_recovery: options.on_recovery,
        })
    }

    /// 读取快照；任何读取/解码失败都降级为空层并上报恢复事件。
    pub fn read(&self) -> PersonalLayer {
        match self.read_current() {
            Ok(layer) => layer,
            Err(error) => {
                if let Some(listener) = &self.on_recovery {
                    listener(&error);
                }
                empty_personal_layer()
            }
        }
    }

    /// 在跨进程锁内读-改-写；返回规范化后的快照。
    pub fn update<F>(&self, transform: F) -> Result<PersonalLayer, RepositoryError>
    where
        F: FnOnce(&PersonalLayer) -> ProviderConfigLayerUpdate,
    {
        let _lock = self.acquire_lock()?;
        let current = self.read_locked()?;
        let next = transform(&current);
        self.write_locked(next)
    }

    fn acquire_lock(&self) -> Result<FileLock, RepositoryError> {
        acquire_file_lock(&self.file_path, &WriteOptions::default()).map_err(RepositoryError::from)
    }

    fn read_current(&self) -> Result<PersonalLayer, RepositoryError> {
        // 纯读不抢锁：正式文件由同目录临时文件原子替换，读到的要么是旧完整文档，
        // 要么是新完整文档。
        let file = read_json_if_exists(&self.file_path)?;
        if file.is_none() && self.import_legacy.is_none() {
            return Ok(empty_personal_layer());
        }
        if let Some(value) = &file {
            let update = decode_provider_config_file(value).map_err(codec_error)?;
            let canonical = encode_provider_config_file(&update);
            if value.to_json_string() == canonical.to_json_string() {
                return decode_personal_document(value).map_err(source_error);
            }
        }
        // 导入和规范化仍会写盘。拿锁后必须重读，不能用加锁前的旧内容覆盖其他 writer。
        let _lock = self.acquire_lock()?;
        self.read_locked()
    }

    fn read_locked(&self) -> Result<PersonalLayer, RepositoryError> {
        let file = read_json_if_exists(&self.file_path)?;
        if file.is_none() {
            if let Some(import) = &self.import_legacy {
                if let Some(update) = import() {
                    return self.write_locked(update);
                }
            }
            return Ok(empty_personal_layer());
        }
        let value = file.expect("已检查 file 存在");
        let update = decode_provider_config_file(&value).map_err(codec_error)?;
        let canonical = encode_provider_config_file(&update);
        if value.to_json_string() != canonical.to_json_string() {
            return self.write_locked(update);
        }
        decode_personal_document(&value).map_err(source_error)
    }

    fn write_locked(
        &self,
        update: ProviderConfigLayerUpdate,
    ) -> Result<PersonalLayer, RepositoryError> {
        // 使用与读取相同的规范形态再验证一次；不能落盘后才发现坏值。
        let canonical = encode_provider_config_file(&update);
        let layer = decode_personal_document(&canonical).map_err(source_error)?;
        let text = serde_json::to_string_pretty(&canonical)
            .map_err(|error| RepositoryError::Invalid(error.to_string()))?;
        // 外层已持锁；原子写内部再抢锁会自锁超时。
        let options = WriteOptions {
            use_file_lock: false,
            ..WriteOptions::default()
        };
        write_private_text_atomic(&self.file_path, &text, &options)
            .map_err(RepositoryError::from)?;
        Ok(layer)
    }
}

fn read_json_if_exists(path: &Path) -> Result<Option<JsonValue>, RepositoryError> {
    match std::fs::read(path) {
        Ok(bytes) => {
            // Node `readFile(..., "utf8")` 对非法序列使用 U+FFFD 替换。
            let text = String::from_utf8_lossy(&bytes);
            JsonValue::from_json_str(&text)
                .map(Some)
                .map_err(|error| RepositoryError::Invalid(error.to_string()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RepositoryError::Io(error.to_string())),
    }
}

fn source_error(error: provider_config_source::SourceError) -> RepositoryError {
    RepositoryError::Invalid(error.to_string())
}

fn codec_error(error: provider_config_file_codec::ProviderConfigCodecError) -> RepositoryError {
    RepositoryError::Invalid(error.to_string())
}

/// `read_json_if_exists` 的公开等价物，供后续轮询切片复用。
pub fn read_json_document_if_exists(path: &Path) -> Result<Option<JsonValue>, RepositoryError> {
    read_json_if_exists(path)
}
