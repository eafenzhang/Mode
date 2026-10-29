//! 数据目录迁移的原语：路径包含判定与 `{base}/.mode/v2` 的复制。
//!
//! 真源：`packages/services/src/paths.ts` 的 `copyDataDirectory` 与
//! `validateDataBaseDirTarget`。复制规则逐条对齐：
//! - 只迁移 `{old}/.mode/v2` → `{new}/.mode/v2`；
//! - **跳过** `setting.json` 与 `setting.json.*`（原子写的 lock/tmp 中间态，复制会 ENOENT）；
//! - 跳过符号链接（Windows 非提权下无法复制）；
//! - 不覆盖已存在的目标文件（对应 `force: false`）。

use std::path::{Path, PathBuf};

pub const FORBIDDEN_DIR_CODE: &str = "DATA_BASE_DIR_FORBIDDEN_WINDOWS_INSTALL_DIR";

#[derive(Debug)]
pub enum DataDirError {
    Io(std::io::Error),
    TargetExists(PathBuf),
    Write(String),
    Forbidden { target: PathBuf, forbidden: PathBuf },
}

impl std::fmt::Display for DataDirError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::TargetExists(path) => write!(formatter, "目标文件已存在：{}", path.display()),
            Self::Write(reason) => write!(formatter, "写入 setting.json 失败：{reason}"),
            Self::Forbidden { target, forbidden } => write!(
                formatter,
                "不能把数据目录放进应用安装目录：{}（{}）",
                target.display(),
                forbidden.display()
            ),
        }
    }
}

impl std::error::Error for DataDirError {}

impl From<std::io::Error> for DataDirError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// 大小写不敏感的「等于或位于其内」（Windows 语义；其它平台调用方不应使用本函数）。
pub fn is_path_equal_or_inside(path: &Path, dir: &Path) -> bool {
    let normalize = |value: &Path| {
        value
            .to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    let path = normalize(path);
    let dir = normalize(dir);
    path == dir || path.starts_with(&format!("{dir}/"))
}

/// 目标目录不得落在应用安装目录内（只在 Windows 生效，与 TS 一致）。
pub fn validate_data_base_dir_target(
    target: &Path,
    forbidden_dirs: &[PathBuf],
    windows: bool,
) -> Result<(), DataDirError> {
    if !windows {
        return Ok(());
    }
    for forbidden in forbidden_dirs {
        if is_path_equal_or_inside(target, forbidden) {
            return Err(DataDirError::Forbidden {
                target: target.to_path_buf(),
                forbidden: forbidden.clone(),
            });
        }
    }
    Ok(())
}

/// 复制 `{old}/.mode/v2` → `{new}/.mode/v2`。
pub fn copy_data_directory(old_base: &Path, new_base: &Path) -> Result<(), DataDirError> {
    let source = old_base.join(".mode").join("v2");
    let target = new_base.join(".mode").join("v2");
    copy_dir(&source, &target)
}

fn copy_dir(source: &Path, target: &Path) -> Result<(), DataDirError> {
    if !source.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // 原子写的中间态：lock 目录 / 临时文件 / 损坏备份都不迁移。
        if name == "setting.json" || name.starts_with("setting.json.") {
            continue;
        }
        let from = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let to = target.join(&name);
        if file_type.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            if to.exists() {
                return Err(DataDirError::TargetExists(to));
            }
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// `setting.updateDataBaseDir` 的编排：校验 → 复制 → 原子更新 `setting.json` 指针。
///
/// 顺序与 TS 一致：`None`/空白表示回到 home；目标与原目录相同时不复制，只写指针。
/// 任一步失败都保留旧配置（复制失败时还没写 setting；写失败时数据已在新目录，指针未变）。
pub fn update_data_base_dir(
    paths: &crate::paths::SettingsPaths,
    current_base: &Path,
    new_dir: Option<&str>,
    windows: bool,
    forbidden_dirs: &[PathBuf],
) -> Result<PathBuf, DataDirError> {
    let target = match new_dir.map(str::trim) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => paths.home.clone(),
    };
    validate_data_base_dir_target(&target, forbidden_dirs, windows)?;
    if target != current_base {
        copy_data_directory(current_base, &target)?;
    }

    let mut document = match crate::store::read_document(paths, &crate::store::ReadOptions::default())
    {
        crate::store::ReadOutcome::Loaded(value) => value,
        _ => serde_json::json!({}),
    };
    if !document.is_object() {
        document = serde_json::json!({});
    }
    if let Some(object) = document.as_object_mut() {
        object.insert(
            "dataBaseDir".to_string(),
            serde_json::Value::String(target.to_string_lossy().into_owned()),
        );
    }
    crate::store::write_document(paths, &document, &atomic_file::WriteOptions::default())
        .map_err(|error| DataDirError::Write(error.to_string()))?;
    Ok(target)
}
