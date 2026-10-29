//! 原子写：临时文件 + rename，并对 Windows 上的短暂占用做退避重试。
//!
//! 与 `packages/services/src/fs/atomicFileUtils.ts` 对齐：
//! 1. 同目录写临时文件（跨卷 rename 会失败）；
//! 2. 清理**同名目标**的过期临时文件（默认 60s），不碰别人的 active temp；
//! 3. rename 只对 `EPERM/EBUSY/EACCES` 重试（50ms→3.2s），真实权限问题原样抛出；
//! 4. 任一步失败删除本次临时文件，不留垃圾。

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::lock::acquire_file_lock;
use crate::{AtomicError, WriteOptions};

/// 原子写入文本（UTF-8）。
pub fn write_text_atomic(
    path: &Path,
    contents: &str,
    options: &WriteOptions,
) -> Result<(), AtomicError> {
    write_bytes_atomic(path, contents.as_bytes(), options)
}

/// 原子写入 JSON（两空格缩进，无尾随换行——与 TS `JSON.stringify(data, null, 2)` 同形）。
pub fn write_json_atomic(
    path: &Path,
    value: &serde_json::Value,
    options: &WriteOptions,
) -> Result<(), AtomicError> {
    let text = serde_json::to_string_pretty(value).map_err(|error| {
        AtomicError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    write_text_atomic(path, &text, options)
}

/// 原子写入私有文本：临时文件在 Unix 上以 `0600` 创建（对应 TS
/// `atomicWritePrivateTextFile` 的 `mode: 0o600`）。Windows 没有可移植的等价位。
pub fn write_private_text_atomic(
    path: &Path,
    contents: &str,
    options: &WriteOptions,
) -> Result<(), AtomicError> {
    write_bytes_atomic_with_mode(path, contents.as_bytes(), options, true)
}

pub fn write_bytes_atomic(
    path: &Path,
    bytes: &[u8],
    options: &WriteOptions,
) -> Result<(), AtomicError> {
    write_bytes_atomic_with_mode(path, bytes, options, false)
}

fn write_bytes_atomic_with_mode(
    path: &Path,
    bytes: &[u8],
    options: &WriteOptions,
    private: bool,
) -> Result<(), AtomicError> {
    let parent = parent_dir(path);
    fs::create_dir_all(&parent)?;

    // 锁的持有期覆盖「清理 + 写临时 + rename」整段：否则两个写者的 rename 可能交错。
    let _lock = if options.use_file_lock {
        Some(acquire_file_lock(path, options)?)
    } else {
        None
    };

    let temp = temp_path(path);
    cleanup_stale_temps(path, &temp, options.temp_file_stale_ms);

    let result = (|| {
        write_temp(&temp, bytes, private)?;
        rename_with_retry(&temp, path, &options.rename_retry_delays_ms)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_temp(temp: &Path, bytes: &[u8], private: bool) -> Result<(), std::io::Error> {
    if !private {
        return fs::write(temp, bytes);
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(temp)?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        fs::write(temp, bytes)
    }
}

fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "atomic".to_string());
    parent_dir(path).join(format!(
        "{name}.{}.{}.{}.tmp",
        std::process::id(),
        now_ms(),
        next_nonce()
    ))
}

fn cleanup_stale_temps(target: &Path, exclude: &Path, stale_ms: u64) {
    let Some(target_name) = target.file_name() else {
        return;
    };
    let prefix = format!("{}.", target_name.to_string_lossy());
    let Ok(entries) = fs::read_dir(parent_dir(target)) else {
        return;
    };
    let now = now_ms();
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !file_name.starts_with(&prefix) || !file_name.ends_with(".tmp") {
            continue;
        }
        let candidate = entry.path();
        if candidate == exclude {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let modified_ms = modified
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(now);
        if now.saturating_sub(modified_ms) < stale_ms {
            continue;
        }
        let _ = fs::remove_file(&candidate);
    }
}

fn rename_with_retry(
    temp: &Path,
    target: &Path,
    retry_delays_ms: &[u64],
) -> Result<(), AtomicError> {
    let mut attempt = 0usize;
    loop {
        match fs::rename(temp, target) {
            Ok(()) => return Ok(()),
            Err(error) => {
                let Some(delay) = retry_delays_ms.get(attempt) else {
                    return Err(AtomicError::Io(error));
                };
                if !is_retryable_rename_error(&error) {
                    return Err(AtomicError::Io(error));
                }
                std::thread::sleep(Duration::from_millis(*delay));
                attempt += 1;
            }
        }
    }
}

fn is_retryable_rename_error(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        ErrorKind::PermissionDenied | ErrorKind::WouldBlock
    ) {
        return true;
    }
    let code = error.raw_os_error();
    #[cfg(windows)]
    {
        // ERROR_ACCESS_DENIED / ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION
        matches!(code, Some(5 | 32 | 33))
    }
    #[cfg(unix)]
    {
        // EPERM / EACCES / EBUSY
        matches!(code, Some(1 | 13 | 16))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = code;
        false
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn next_nonce() -> u64 {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or(0);
    NONCE.fetch_add(1, Ordering::Relaxed) ^ nanos
}
