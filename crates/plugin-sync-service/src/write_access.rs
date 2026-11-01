//! 写权限探测（`checkRemoteUserPluginWriteAccess`）：在目标目录写一次性标记文件。

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::paths::{user_mode_config_path, user_mode_plugin_root};
use crate::types::RemoteSyncWriteAccessResult;

pub fn check_remote_user_plugin_write_access(home: &Path) -> RemoteSyncWriteAccessResult {
    let dirs = [
        user_mode_plugin_root(home),
        user_mode_config_path(home)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| user_mode_config_path(home)),
    ];
    for directory in &dirs {
        let result = check_directory_write_access(directory);
        if !result.ok {
            return result;
        }
    }
    RemoteSyncWriteAccessResult {
        ok: true,
        path: dirs
            .iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(", "),
        error: None,
    }
}

fn check_directory_write_access(directory: &Path) -> RemoteSyncWriteAccessResult {
    let marker: PathBuf = directory.join(marker_name());
    let outcome = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(directory)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)?;
        file.write_all(b"ok")?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&marker);
    match outcome {
        Ok(()) => RemoteSyncWriteAccessResult {
            ok: true,
            path: directory.to_string_lossy().into_owned(),
            error: None,
        },
        Err(error) => RemoteSyncWriteAccessResult {
            ok: false,
            path: directory.to_string_lossy().into_owned(),
            error: Some(error.to_string()),
        },
    }
}

fn marker_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(".mode-sync-preflight-{}-{nanos}", std::process::id())
}
