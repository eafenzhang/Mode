//! 配置文件原子写：同目录临时文件 + fsync + rename。
//!
//! 运行时 watcher 绝不能看到半份配置，因此临时文件必须落在目标同目录（跨盘 rename 不原子），
//! 写完 `sync_all` 再替换；任何失败都清掉临时文件。

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn atomic_write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = path.with_file_name(format!(
        ".{file_name}.{}.{nanos}.{counter}.tmp",
        std::process::id()
    ));

    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp_path, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}
