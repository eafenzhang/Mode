//! `setting.json` 的读取与写回。
//!
//! 读取语义（对齐 TS `readSettingsWithMeta`）：
//! - 文件不存在 → [`ReadOutcome::Missing`]（调用方用默认值）；
//! - JSON 解析失败 → 短重试（默认 3 次 × 300 ms，每次都重新读文件，避免把「另一个写者
//!   正在覆盖、读到半截」误判成坏文件）；仍失败才把文件改名为
//!   `setting.json.corrupt-<millis>` 并返回 [`ReadOutcome::Corrupt`]；
//! - 其它 IO 错误 → [`ReadOutcome::Unreadable`]（TS 同样回落默认值，但这里保留原因）。
//!
//! **schema 校验不在这里**：TS 在校验失败时返回默认值且**不隔离文件**，那一步需要
//! `AppSettings` 字段表，属 W1-1b。

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use atomic_file::{write_json_atomic, AtomicError, WriteOptions};
use serde_json::Value;

use crate::paths::SettingsPaths;

#[derive(Debug, Clone)]
pub struct ReadOptions {
    pub parse_retry_count: u32,
    pub parse_retry_delay: Duration,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            parse_retry_count: 3,
            parse_retry_delay: Duration::from_millis(300),
        }
    }
}

#[derive(Debug)]
pub enum ReadOutcome {
    Loaded(Value),
    Missing,
    Unreadable { error: String },
    Corrupt { backup: Option<PathBuf> },
}

/// 读取 `setting.json`。
pub fn read_document(paths: &SettingsPaths, options: &ReadOptions) -> ReadOutcome {
    let raw = match fs::read_to_string(&paths.file) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return ReadOutcome::Missing,
        Err(error) => {
            return ReadOutcome::Unreadable {
                error: error.to_string(),
            }
        }
    };
    if let Ok(value) = serde_json::from_str::<Value>(&raw) {
        return ReadOutcome::Loaded(value);
    }

    for _ in 0..options.parse_retry_count {
        std::thread::sleep(options.parse_retry_delay);
        if let Ok(next) = fs::read_to_string(&paths.file) {
            if let Ok(value) = serde_json::from_str::<Value>(&next) {
                return ReadOutcome::Loaded(value);
            }
        }
    }

    ReadOutcome::Corrupt {
        backup: quarantine(&paths.file),
    }
}

/// 原子写回（走 `crates/atomic-file` 的锁与 rename）。
pub fn write_document(
    paths: &SettingsPaths,
    value: &Value,
    options: &WriteOptions,
) -> Result<(), AtomicError> {
    write_json_atomic(&paths.file, value, options)
}

/// 把坏文件改名保留；`ENOENT` 说明别的读者已经隔离过，属于预期竞争。
fn quarantine(file: &Path) -> Option<PathBuf> {
    let backup = backup_path(file);
    match fs::rename(file, &backup) {
        Ok(()) => Some(backup),
        Err(_) => None,
    }
}

fn backup_path(file: &Path) -> PathBuf {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "setting.json".to_string());
    file.with_file_name(format!("{name}.corrupt-{}", now_ms()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
