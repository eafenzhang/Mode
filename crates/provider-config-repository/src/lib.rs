//! Personal Provider Config 仓库：读/写/锁/原子写/legacy 导入/轮询。
//!
//! 真源：`packages/provider-node/src/personal-provider-config-repository.ts` 与
//! `runtime-paths.ts`。行为契约见 `docs/specs/provider-config-repository.md`。
//!
//! 轮询只在首次 `read`/`update` 后启动；`dispose` 停止线程并清空订阅者。

use std::collections::HashMap;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use atomic_file::{
    acquire_file_lock, write_private_text_atomic, AtomicError, FileLock, WriteOptions,
};
use provider_config::JsonValue;
use provider_config_file_codec::{
    decode_provider_config_file, encode_provider_config_file, ProviderConfigLayerUpdate,
};
use provider_config_source::{decode_personal_document, empty_personal_layer, PersonalLayer};

pub mod cache_paths;
pub mod materializer;
pub mod remote_refresh;
pub mod remote_sync;
pub mod runtime_paths;

pub use cache_paths::{
    create_mode_builtin_endpoint_key, normalize_mode_builtin_endpoint_origin,
    resolve_mode_builtin_cache_paths, resolve_mode_builtin_client_platform,
    resolve_mode_builtin_client_platform_from_process, ModeBuiltinCachePathOptions,
    ModeBuiltinCachePaths,
};
pub use materializer::{
    materialize_mode_builtin_provider_config, MaterializeModeBuiltinProviderConfigOptions,
};
pub use remote_refresh::{
    refresh_mode_builtin_release, ApplyOutcome, RefreshOptions, RefreshOutcome, RefreshResult,
    SkipReason,
};
pub use remote_sync::{
    acquire_refresh_lease, finish_refresh_lease, read_refresh_control, write_refresh_control,
    AcquireOptions, AcquireOutcome, FinishOptions, LeaseFinish, RefreshControl,
};
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
pub type PollingErrorListener = Arc<dyn Fn(&RepositoryError) + Send + Sync>;
pub type ChangeListener = Arc<dyn Fn(&str) + Send + Sync>;

/// 轮询间隔；`Disabled` 对应 TS 的 `pollingIntervalMs: false`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollingInterval {
    Disabled,
    Millis(u64),
}

impl Default for PollingInterval {
    fn default() -> Self {
        Self::Millis(1_000)
    }
}

pub struct PersonalConfigRepositoryOptions {
    pub file_path: PathBuf,
    pub import_legacy: Option<LegacyImporter>,
    pub on_recovery: Option<RecoveryListener>,
    pub on_polling_error: Option<PollingErrorListener>,
    pub polling_interval: PollingInterval,
}

pub struct PersonalConfigRepository {
    inner: Arc<Inner>,
}

impl PersonalConfigRepository {
    pub fn new(options: PersonalConfigRepositoryOptions) -> Result<Self, RepositoryError> {
        if options.file_path.to_string_lossy().trim().is_empty() {
            return Err(RepositoryError::Invalid(
                "Personal Provider Config filePath 不能为空".to_string(),
            ));
        }
        if options.polling_interval == PollingInterval::Millis(0) {
            return Err(RepositoryError::Invalid(
                "Personal Provider Config pollingIntervalMs 必须大于 0".to_string(),
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                file_path: options.file_path,
                import_legacy: options.import_legacy,
                on_recovery: options.on_recovery,
                on_polling_error: options.on_polling_error,
                polling_interval: options.polling_interval,
                state: Mutex::new(PollingState::default()),
                listeners: Mutex::new(HashMap::new()),
                next_listener: AtomicU64::new(1),
                polling_started: AtomicBool::new(false),
                disposed: AtomicBool::new(false),
                stop_tx: Mutex::new(None),
            }),
        })
    }

    /// 读取快照；任何读取/解码失败都降级为空层并上报恢复事件。
    pub fn read(&self) -> PersonalLayer {
        let layer = self.inner.read();
        self.inner.ensure_polling();
        layer
    }

    /// 在跨进程锁内读-改-写；返回规范化后的快照。
    pub fn update<F>(&self, transform: F) -> Result<PersonalLayer, RepositoryError>
    where
        F: FnOnce(&PersonalLayer) -> ProviderConfigLayerUpdate,
    {
        let result = self.inner.update(transform);
        self.inner.ensure_polling();
        result
    }

    /// 订阅变更通知；返回的句柄 drop 时解除订阅。
    pub fn on_did_change(&self, listener: ChangeListener) -> ChangeSubscription {
        let token = self.inner.next_listener.fetch_add(1, Ordering::SeqCst);
        if !self.inner.disposed.load(Ordering::SeqCst) {
            lock(&self.inner.listeners).insert(token, listener);
        }
        ChangeSubscription {
            inner: Arc::clone(&self.inner),
            token,
        }
    }

    /// 停止轮询并清空订阅者；幂等。
    pub fn dispose(&self) {
        self.inner.dispose();
    }
}

struct Inner {
    file_path: PathBuf,
    import_legacy: Option<LegacyImporter>,
    on_recovery: Option<RecoveryListener>,
    on_polling_error: Option<PollingErrorListener>,
    polling_interval: PollingInterval,
    state: Mutex<PollingState>,
    listeners: Mutex<HashMap<u64, ChangeListener>>,
    next_listener: AtomicU64,
    polling_started: AtomicBool,
    disposed: AtomicBool,
    stop_tx: Mutex<Option<Sender<()>>>,
}

#[derive(Default)]
struct PollingState {
    observed_revision: Option<String>,
    write_generation: u64,
    polling_error_active: bool,
}

impl Inner {
    fn read(&self) -> PersonalLayer {
        match self.read_current() {
            Ok(layer) => {
                let mut state = lock(&self.state);
                if state.observed_revision.is_none() {
                    state.observed_revision = Some(layer.revision.clone());
                }
                drop(state);
                layer
            }
            Err(error) => {
                self.report_recovery(&error);
                empty_personal_layer()
            }
        }
    }

    fn update<F>(&self, transform: F) -> Result<PersonalLayer, RepositoryError>
    where
        F: FnOnce(&PersonalLayer) -> ProviderConfigLayerUpdate,
    {
        if self.disposed.load(Ordering::SeqCst) {
            return Err(RepositoryError::Invalid(
                "Personal Provider Config Repository 已 dispose".to_string(),
            ));
        }
        let _lock = self.acquire_lock()?;
        let current = self.read_locked()?;
        let next = transform(&current);
        let layer = self.write_locked(next)?;
        lock(&self.state).observed_revision = Some(layer.revision.clone());
        self.emit("updated");
        Ok(layer)
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
        lock(&self.state).write_generation += 1;
        Ok(layer)
    }

    fn ensure_polling(self: &Arc<Self>) {
        if self.disposed.load(Ordering::SeqCst)
            || self.polling_interval == PollingInterval::Disabled
            || self.polling_started.swap(true, Ordering::SeqCst)
        {
            return;
        }
        let PollingInterval::Millis(millis) = self.polling_interval else {
            return;
        };
        let interval = Duration::from_millis(millis);
        let (tx, rx) = mpsc::channel::<()>();
        *lock(&self.stop_tx) = Some(tx);
        let inner = Arc::clone(self);
        std::thread::spawn(move || loop {
            match rx.recv_timeout(interval) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    if inner.disposed.load(Ordering::SeqCst) {
                        break;
                    }
                    inner.poll_once();
                }
            }
        });
    }

    fn poll_once(&self) {
        let generation = lock(&self.state).write_generation;
        let snapshot = self.read_polling_snapshot();
        let mut state = lock(&self.state);
        // 去掉读锁后，旧轮询可能晚于本进程保存返回；丢弃它，避免版本倒退及重复通知。
        if self.disposed.load(Ordering::SeqCst) || generation != state.write_generation {
            return;
        }
        match snapshot {
            Ok(layer) => {
                state.polling_error_active = false;
                if state.observed_revision.as_deref() == Some(layer.revision.as_str()) {
                    return;
                }
                state.observed_revision = Some(layer.revision);
                drop(state);
                self.emit("poll-changed");
            }
            Err(error) => {
                if !state.polling_error_active {
                    state.polling_error_active = true;
                    drop(state);
                    if let Some(listener) = &self.on_polling_error {
                        listener(&error);
                    }
                    self.emit("poll-error");
                }
            }
        }
    }

    fn read_polling_snapshot(&self) -> Result<PersonalLayer, RepositoryError> {
        let Some(value) = read_json_if_exists(&self.file_path)? else {
            return Ok(empty_personal_layer());
        };
        let update = decode_provider_config_file(&value).map_err(codec_error)?;
        let canonical = encode_provider_config_file(&update);
        decode_personal_document(&canonical).map_err(source_error)
    }

    fn report_recovery(&self, error: &RepositoryError) {
        if let Some(listener) = &self.on_recovery {
            listener(error);
        }
    }

    fn emit(&self, reason: &str) {
        if self.disposed.load(Ordering::SeqCst) {
            return;
        }
        let listeners: Vec<ChangeListener> = lock(&self.listeners).values().cloned().collect();
        for listener in listeners {
            listener(reason);
        }
    }

    fn dispose(&self) {
        if self.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        lock(&self.listeners).clear();
        if let Some(tx) = lock(&self.stop_tx).take() {
            let _ = tx.send(());
        }
    }
}

/// 变更订阅句柄；drop 时解除订阅。
pub struct ChangeSubscription {
    inner: Arc<Inner>,
    token: u64,
}

impl Drop for ChangeSubscription {
    fn drop(&mut self) {
        lock(&self.inner.listeners).remove(&self.token);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
