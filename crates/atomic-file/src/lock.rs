//! 跨进程文件锁：目录 `mkdir` 做 CAS + 每个写者一个 owner 文件。
//!
//! 与 `packages/shared/src/node/atomicFileLock.ts` 对齐的规则：
//!
//! - 锁是一个目录 `<目标文件>.lock`；`mkdir` 成功 = 拿到锁（原子，跨进程有效）。
//! - 拿到锁后写 `owner-<token>.json`，再校验目录里**只有**自己的 owner 文件；
//!   目录被别人替换/抢占时能看到多出来的 owner，按竞争处理。
//! - 竞争时按 `[25,50,100,200,400]` 退避；到 `max_wait_ms` 直接超时，**不在超时点回收**，
//!   避免删掉后来写者刚建的锁。
//! - 回收只处理「owner 进程已退出」或「没有 pid 且超过 ownerless grace」的锁；目录里的
//!   owner 多于一个时，还要求目录本身也已超过 grace。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::liveness::process_alive;
use crate::{AtomicError, WriteOptions};

const OWNER_PREFIX: &str = "owner-";
const OWNER_SUFFIX: &str = ".json";
/// owner 文件里 `createdAt` 的合法上限（与 TS 的 5 分钟时钟偏移容忍一致）。
const MAX_CLOCK_SKEW_MS: u64 = 5 * 60_000;

pub struct FileLock {
    lock_dir: PathBuf,
    owner_file: PathBuf,
    released: bool,
}

impl FileLock {
    fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let _ = fs::remove_file(&self.owner_file);
        // 只删空目录：目录里若已出现别人的 owner，`remove_dir` 会失败，这是我们要的保护。
        let _ = fs::remove_dir(&self.lock_dir);
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        self.release();
    }
}

/// 锁目录路径（`<目标文件>.lock`）。
pub fn lock_dir_path(file_path: &Path) -> PathBuf {
    let mut raw = file_path.as_os_str().to_os_string();
    raw.push(".lock");
    PathBuf::from(raw)
}

/// 取得锁；超时返回 [`AtomicError::LockTimeout`]。
pub fn acquire_file_lock(
    file_path: &Path,
    options: &WriteOptions,
) -> Result<FileLock, AtomicError> {
    let lock_dir = lock_dir_path(file_path);
    let token = new_token();
    let owner_name = format!("{OWNER_PREFIX}{token}{OWNER_SUFFIX}");
    let owner_file = lock_dir.join(&owner_name);
    let payload = json!({
        "pid": std::process::id(),
        "createdAt": now_ms(),
        "token": token,
    })
    .to_string();
    let started = Instant::now();
    let grace = Duration::from_millis(
        options
            .lock_ownerless_grace_ms
            .min(options.lock_max_wait_ms / 2),
    );

    for attempt in 0usize.. {
        match fs::create_dir(&lock_dir) {
            Ok(()) => match claim(&lock_dir, &owner_file, &owner_name, &payload) {
                Ok(()) => {
                    return Ok(FileLock {
                        lock_dir,
                        owner_file,
                        released: false,
                    });
                }
                Err(ClaimOutcome::Busy) => {}
                Err(ClaimOutcome::Failed(error)) => return Err(error),
            },
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(AtomicError::Io(error)),
        }

        let waited_ms = started.elapsed().as_millis() as u64;
        if waited_ms >= options.lock_max_wait_ms {
            return Err(AtomicError::LockTimeout { lock_path: lock_dir, waited_ms });
        }
        if reclaim_abandoned(&lock_dir, grace)? {
            continue;
        }

        let remaining = options.lock_max_wait_ms.saturating_sub(waited_ms);
        let delay = if options.lock_retry_delays_ms.is_empty() {
            remaining
        } else {
            options.lock_retry_delays_ms[attempt.min(options.lock_retry_delays_ms.len() - 1)]
        };
        if delay == 0 || remaining == 0 {
            return Err(AtomicError::LockTimeout {
                lock_path: lock_dir,
                waited_ms: started.elapsed().as_millis() as u64,
            });
        }
        std::thread::sleep(Duration::from_millis(delay.min(remaining)));
    }
    unreachable!("锁获取循环必须通过 return 退出")
}

enum ClaimOutcome {
    /// 目录已存在或已被替换：按竞争继续等。
    Busy,
    Failed(AtomicError),
}

fn claim(
    lock_dir: &Path,
    owner_file: &Path,
    owner_name: &str,
    payload: &str,
) -> Result<(), ClaimOutcome> {
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(owner_file)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ClaimOutcome::Busy);
        }
        Err(error) => return Err(ClaimOutcome::Failed(AtomicError::Io(error))),
    };
    if let Err(error) = file.write_all(payload.as_bytes()) {
        let _ = fs::remove_file(owner_file);
        let _ = fs::remove_dir(lock_dir);
        return Err(ClaimOutcome::Failed(AtomicError::Io(error)));
    }
    drop(file);

    match fs::read_dir(lock_dir) {
        Ok(entries) => {
            let names: Vec<String> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
            if names.len() == 1 && names[0] == owner_name {
                Ok(())
            } else {
                // 目录在 create_dir 与写 owner 之间被替换/抢占：只清理自己的 owner，
                // 目录里还有别人的条目时 `remove_dir` 会失败（正是我们要的）。
                let _ = fs::remove_file(owner_file);
                let _ = fs::remove_dir(lock_dir);
                Err(ClaimOutcome::Busy)
            }
        }
        Err(error) => {
            let _ = fs::remove_file(owner_file);
            let _ = fs::remove_dir(lock_dir);
            Err(ClaimOutcome::Failed(AtomicError::Io(error)))
        }
    }
}

/// 回收废弃锁；返回 `true` 表示现场已清理，可以立刻重试。
fn reclaim_abandoned(lock_dir: &Path, grace: Duration) -> Result<bool, AtomicError> {
    let metadata = match fs::metadata(lock_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(AtomicError::Io(error)),
    };

    if metadata.is_dir() {
        let entries: Vec<PathBuf> = match fs::read_dir(lock_dir) {
            Ok(reader) => reader
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(AtomicError::Io(error)),
        };
        let owners: Vec<&PathBuf> = entries
            .iter()
            .filter(|path| {
                path.file_name()
                    .map(|name| {
                        let name = name.to_string_lossy();
                        name.starts_with(OWNER_PREFIX) && name.ends_with(OWNER_SUFFIX)
                    })
                    .unwrap_or(false)
            })
            .collect();

        if owners.len() == 1 {
            if !owner_reclaimable(owners[0], grace)? {
                return Ok(false);
            }
            let _ = fs::remove_file(owners[0]);
            return Ok(remove_dir_best_effort(lock_dir));
        }

        // 空目录（mkdir 后、owner 落盘前崩溃）或多个 owner：要求目录本身也已超过 grace。
        if directory_age(metadata) < grace {
            return Ok(false);
        }
        for owner in &owners {
            if !owner_reclaimable(owner, grace)? {
                return Ok(false);
            }
        }
        for entry in &entries {
            if entry.is_dir() {
                let _ = fs::remove_dir_all(entry);
            } else {
                let _ = fs::remove_file(entry);
            }
        }
        return Ok(remove_dir_best_effort(lock_dir));
    }

    // 升级前遗留的单文件锁：内容在被删除前必须保持一致（防 ABA）。
    let raw = match fs::read(lock_dir) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(AtomicError::Io(error)),
    };
    if !owner_bytes_reclaimable(&raw, Some(metadata), grace) {
        return Ok(false);
    }
    match fs::read(lock_dir) {
        Ok(recheck) if recheck == raw => {}
        _ => return Ok(false),
    }
    match fs::remove_file(lock_dir) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AtomicError::Io(error)),
    }
}

fn remove_dir_best_effort(dir: &Path) -> bool {
    match fs::remove_dir(dir) {
        Ok(()) => true,
        // 目录非空说明出现了后来写者的条目：不回收，继续等。
        Err(_) => false,
    }
}

fn owner_reclaimable(owner_file: &Path, grace: Duration) -> Result<bool, AtomicError> {
    let raw = match fs::read(owner_file) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(AtomicError::Io(error)),
    };
    let metadata = fs::metadata(owner_file).ok();
    Ok(owner_bytes_reclaimable(&raw, metadata, grace))
}

fn owner_bytes_reclaimable(raw: &[u8], metadata: Option<fs::Metadata>, grace: Duration) -> bool {
    let parsed: Option<serde_json::Value> = serde_json::from_slice(raw).ok();
    let pid = parsed
        .as_ref()
        .and_then(|meta| meta.get("pid"))
        .and_then(serde_json::Value::as_i64)
        .filter(|pid| *pid > 0 && *pid <= i64::from(u32::MAX));
    let created_at = parsed
        .as_ref()
        .and_then(|meta| meta.get("createdAt"))
        .and_then(serde_json::Value::as_u64)
        .filter(|created| *created <= now_ms().saturating_add(MAX_CLOCK_SKEW_MS))
        .or_else(|| metadata.as_ref().and_then(modified_ms));

    match pid {
        Some(pid) => !process_alive(pid as u32),
        None => created_at
            .map(|created| now_ms().saturating_sub(created) >= grace.as_millis() as u64)
            .unwrap_or(false),
    }
}

fn directory_age(metadata: fs::Metadata) -> Duration {
    modified_ms(&metadata)
        .map(|modified| Duration::from_millis(now_ms().saturating_sub(modified)))
        .unwrap_or_default()
}

fn modified_ms(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn new_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{:x}",
        std::process::id(),
        now_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
