//! settings 服务面：读（默认值合并）、补丁更新、数据目录迁移、默认项目。

use std::path::{Path, PathBuf};

use atomic_file::{AtomicError, WriteOptions};

use crate::data_dir::{update_data_base_dir, DataDirError};
use crate::document::{apply_patch, defaults, merge_persisted};
use crate::paths::SettingsPaths;
use crate::project::{ensure_default_project, DefaultProject};
use crate::store::{read_document, write_document, ReadOptions, ReadOutcome};

pub struct SettingsService {
    paths: SettingsPaths,
    current_base: PathBuf,
    windows: bool,
    forbidden_dirs: Vec<PathBuf>,
}

impl SettingsService {
    pub fn new(paths: SettingsPaths, current_base: PathBuf) -> Self {
        Self {
            paths,
            current_base,
            windows: cfg!(windows),
            forbidden_dirs: Vec::new(),
        }
    }

    /// 读取：缺失/损坏/不可读都回落默认值（与 TS 一致）。
    pub fn get(&self) -> serde_json::Value {
        match read_document(&self.paths, &ReadOptions::default()) {
            ReadOutcome::Loaded(value) => merge_persisted(&value),
            _ => defaults().clone(),
        }
    }

    /// 打补丁并原子写回；返回更新后的完整设置。
    pub fn update(&self, patch: &serde_json::Value) -> Result<serde_json::Value, AtomicError> {
        let merged = apply_patch(&self.get(), patch);
        write_document(&self.paths, &merged, &WriteOptions::default())?;
        Ok(merged)
    }

    pub fn update_data_base_dir(&self, new_dir: Option<&str>) -> Result<PathBuf, DataDirError> {
        update_data_base_dir(
            &self.paths,
            &self.current_base,
            new_dir,
            self.windows,
            &self.forbidden_dirs,
        )
    }

    pub fn ensure_default_project(&self, home: &Path) -> std::io::Result<DefaultProject> {
        ensure_default_project(home)
    }
}
