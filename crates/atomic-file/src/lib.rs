//! 原子写文件与跨进程锁（W1 平台基元）。
//!
//! 行为真源：
//! - `packages/services/src/fs/atomicFileUtils.ts`（临时文件 + rename + 重试、过期临时文件清理）
//! - `packages/shared/src/node/atomicFileLock.ts`（目录 mkdir 做 CAS、owner 文件、废弃锁回收）
//!
//! 为什么需要这一层：设置、凭据、任务 JSON 都是「读-改-写」，多窗口/多进程同时保存时
//! 直接 `writeFile` 会把文件写成半截。rename 在同一文件系统内是原子的，但 Windows 上
//! Defender/索引器会短暂占用目标文件，所以还要对 `EPERM/EBUSY/EACCES` 做退避重试。
//!
//! 与 TS 实现的两处有意差异：
//! 1. 不实现 `lockInstanceObserver`（TS 用来防止「mtime 被刷新后锁永不过期」）。这里以
//!    owner 文件的 mtime 作为 `createdAt` 兜底；锁持有者不会主动 touch 它。
//! 2. 不校验锁目录的 dev/inode 身份（Rust std 跨平台拿不到稳定 inode）。改为在写入 owner
//!    后**校验目录里只有本写者的 owner 文件**：目录被替换时会看到多出来的 owner，按竞争处理。

mod atomic;
mod liveness;
mod lock;

pub use atomic::{
    write_bytes_atomic, write_json_atomic, write_private_text_atomic, write_text_atomic,
};
pub use lock::{acquire_file_lock, FileLock};

use std::path::PathBuf;

/// 与 TS 默认值逐项一致（改这里就是改行为，必须同步对拍）。
pub const DEFAULT_RENAME_RETRY_DELAYS_MS: [u64; 7] = [50, 100, 200, 400, 800, 1600, 3200];
pub const DEFAULT_LOCK_RETRY_DELAYS_MS: [u64; 5] = [25, 50, 100, 200, 400];
pub const DEFAULT_LOCK_OWNERLESS_GRACE_MS: u64 = 100;
pub const DEFAULT_LOCK_MAX_WAIT_MS: u64 = 8_000;
pub const DEFAULT_TEMP_FILE_STALE_MS: u64 = 60_000;

#[derive(Debug, Clone)]
pub struct WriteOptions {
    pub rename_retry_delays_ms: Vec<u64>,
    pub lock_retry_delays_ms: Vec<u64>,
    pub lock_ownerless_grace_ms: u64,
    pub lock_max_wait_ms: u64,
    pub temp_file_stale_ms: u64,
    /// `false` 跳过跨进程锁（只做临时文件 + rename）。测试与单进程场景用。
    pub use_file_lock: bool,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            rename_retry_delays_ms: DEFAULT_RENAME_RETRY_DELAYS_MS.to_vec(),
            lock_retry_delays_ms: DEFAULT_LOCK_RETRY_DELAYS_MS.to_vec(),
            lock_ownerless_grace_ms: DEFAULT_LOCK_OWNERLESS_GRACE_MS,
            lock_max_wait_ms: DEFAULT_LOCK_MAX_WAIT_MS,
            temp_file_stale_ms: DEFAULT_TEMP_FILE_STALE_MS,
            use_file_lock: true,
        }
    }
}

#[derive(Debug)]
pub enum AtomicError {
    Io(std::io::Error),
    /// 等锁超时（对应 TS 的 `MODE_FILE_LOCK_TIMEOUT_ERROR_CODE`）。
    LockTimeout { lock_path: PathBuf, waited_ms: u64 },
}

impl std::fmt::Display for AtomicError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::LockTimeout {
                lock_path,
                waited_ms,
            } => write!(
                formatter,
                "Timed out after {waited_ms}ms waiting for the Mode file lock: {}",
                lock_path.display()
            ),
        }
    }
}

impl std::error::Error for AtomicError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::LockTimeout { .. } => None,
        }
    }
}

impl From<std::io::Error> for AtomicError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
