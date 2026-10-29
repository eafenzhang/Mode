//! 设备身份 `deviceMid`（W1 基元）。
//!
//! 真源：`packages/services/src/device/deviceMid.ts`。落点
//! `{dataRoot}/v2/telemetry-state.json`（文件名沿用历史，改名等于重置设备身份）。
//!
//! 与 TS 的行为差异（都更保守，不影响文件格式）：
//! 1. 锁用 `crates/atomic-file` 的目录锁（`<文件>.lock` 目录 + owner 文件），TS 用的是
//!    `<文件>.lock` **普通文件**。两者互不兼容，但同一版本内不会混用；文件内容仍是
//!    `telemetry-state.json`，跨端共享不受影响。
//! 2. 写回走原子写（临时文件 + rename），TS 是直接 `writeFile`。
//! 3. 不做进程内 Promise 缓存（Rust 侧调用方可以自己缓存；重复调用只是一次读 + 一次锁）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use atomic_file::{acquire_file_lock, write_json_atomic, AtomicError, WriteOptions};


pub const STATE_FILE_NAME: &str = "telemetry-state.json";
pub const DEVICE_MID_KEY: &str = "deviceMid";

pub struct DeviceMidStore {
    state_file: PathBuf,
    options: WriteOptions,
}

impl DeviceMidStore {
    /// `data_root` 是 `{dataBaseDir}/.mode`。
    pub fn new(data_root: impl Into<PathBuf>) -> Self {
        let state_file = Path::new(&data_root.into()).join("v2").join(STATE_FILE_NAME);
        Self {
            state_file,
            options: WriteOptions::default(),
        }
    }

    pub fn state_file(&self) -> &Path {
        &self.state_file
    }

    /// 读取已有 deviceMid；缺失时在锁内生成并写回。保留文件里的其它字段。
    pub fn ensure(&self) -> Result<String, AtomicError> {
        if let Some(existing) = self.read()? {
            return Ok(existing);
        }
        // 锁目录建在数据文件旁边：父目录不存在时 create_dir 会失败，先补齐。
        if let Some(parent) = self.state_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // 与 TS 相同的临界区：锁内重读，避免两个进程同时生成两个 id。
        let _lock = acquire_file_lock(&self.state_file, &self.options)?;
        if let Some(existing) = self.read()? {
            return Ok(existing);
        }
        let device_mid = generate_uuid_v4();
        let mut state = self.read_state_object().unwrap_or_default();
        state.insert(
            DEVICE_MID_KEY.to_string(),
            serde_json::Value::String(device_mid.clone()),
        );
        // 已经在锁内：内层写入不能再抢同一把锁（同路径目录锁不可重入，会死等）。
        let unlocked = WriteOptions {
            use_file_lock: false,
            ..self.options.clone()
        };
        write_json_atomic(
            &self.state_file,
            &serde_json::Value::Object(state),
            &unlocked,
        )?;
        Ok(device_mid)
    }

    fn read(&self) -> Result<Option<String>, AtomicError> {
        let Some(object) = self.read_state_object() else {
            return Ok(None);
        };
        Ok(object
            .get(DEVICE_MID_KEY)
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string))
    }

    /// 损坏/缺失都返回 `None`（TS 同样把坏文件当作「从未记录」，下次写入时重建）。
    fn read_state_object(&self) -> Option<serde_json::Map<String, serde_json::Value>> {
        let raw = std::fs::read_to_string(&self.state_file).ok()?;
        match serde_json::from_str::<serde_json::Value>(&raw).ok()? {
            serde_json::Value::Object(object) => Some(object),
            _ => None,
        }
    }
}

/// UUID v4（格式合法即可；非加密用途）。熵来自 `RandomState`（进程级 OS 种子）+ 时间 + pid。
fn generate_uuid_v4() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hash, Hasher};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seeded = RandomState::new();
    let mut bytes = [0u8; 16];
    for (index, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = seeded.build_hasher();
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
            .hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        index.hash(&mut hasher);
        COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
        let hash = hasher.finish().to_le_bytes();
        chunk.copy_from_slice(&hash[..chunk.len()]);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10x
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}
