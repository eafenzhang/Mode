//! 随包 Mode Built-in Release 的物化写入。
//!
//! 真源：`packages/provider-node/src/mode-builtin-provider-config-materializer.ts`。
//! 在环境目录释放唯一随包基线；升级以退出旧进程为前提，不保留历史 hash 副本。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use atomic_file::{acquire_file_lock, write_private_text_atomic, WriteOptions};
use provider_config::JsonValue;
use provider_config_source::{decode_mode_builtin_release, serialize_mode_builtin_release};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeModeBuiltinProviderConfigOptions {
    pub environment_config_root: PathBuf,
    pub content: String,
}

/// 写入 `runtime/provider/bundled/mode-builtin.json`，返回文件路径。
/// 内容未变化时不重写；写入复用跨进程锁与私有原子写。
pub fn materialize_mode_builtin_provider_config(
    options: &MaterializeModeBuiltinProviderConfigOptions,
) -> Result<PathBuf, String> {
    let parsed = JsonValue::from_json_str(&options.content).map_err(|error| error.to_string())?;
    let release = decode_mode_builtin_release(&parsed).map_err(|error| error.to_string())?;
    let content = format!("{}\n", serialize_mode_builtin_release(&release));

    let file_path = options
        .environment_config_root
        .join("runtime")
        .join("provider")
        .join("bundled")
        .join("mode-builtin.json");
    // 锁目录建在目标文件的父目录下；首次物化时父目录还不存在。
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let _lock = acquire_file_lock(&file_path, &WriteOptions::default())
        .map_err(|error| error.to_string())?;
    if read_optional_file(&file_path)?.as_deref() != Some(content.as_str()) {
        // 外层已持锁；原子写内部再抢锁会自锁超时。
        let write_options = WriteOptions {
            use_file_lock: false,
            ..WriteOptions::default()
        };
        write_private_text_atomic(&file_path, &content, &write_options)
            .map_err(|error| error.to_string())?;
    }
    Ok(file_path)
}

fn read_optional_file(file_path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(file_path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}
