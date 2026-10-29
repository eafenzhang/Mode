//! settings 路径解析。
//!
//! 与 TS 对齐：`MODE_DESKTOP_HOME_DIR` → `HOME` → `USERPROFILE` → 进程 home；
//! 三者都要 trim 且非空才生效（空串等同于未设置）。
//! 目录固定为 `{home}/.mode/v2`，文件 `setting.json`。
//!
//! 每次调用都按传入的环境重新解析——不在模块加载时固化为常量（TS 里踩过：
//! 测试只改 HOME 时读到真实用户目录）。

use std::ffi::OsString;
use std::path::PathBuf;

pub const MODE_DATA_ROOT_DIR_NAME: &str = ".mode";

#[derive(Debug, Clone)]
pub struct HomeEnv {
    pub mode_desktop_home_dir: Option<OsString>,
    pub home: Option<OsString>,
    pub user_profile: Option<OsString>,
    /// 三者都不可用时的兜底（进程 home 或调用方指定）。
    pub fallback_home: PathBuf,
}

impl HomeEnv {
    pub fn from_process() -> Self {
        let fallback_home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            mode_desktop_home_dir: std::env::var_os("MODE_DESKTOP_HOME_DIR"),
            home: std::env::var_os("HOME"),
            user_profile: std::env::var_os("USERPROFILE"),
            fallback_home,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsPaths {
    pub home: PathBuf,
    pub dir: PathBuf,
    pub file: PathBuf,
}

fn usable(value: Option<&OsString>) -> Option<PathBuf> {
    let text = value?.to_string_lossy();
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

pub fn resolve_home(env: &HomeEnv) -> PathBuf {
    usable(env.mode_desktop_home_dir.as_ref())
        .or_else(|| usable(env.home.as_ref()))
        .or_else(|| usable(env.user_profile.as_ref()))
        .unwrap_or_else(|| env.fallback_home.clone())
}

pub fn resolve_settings_paths(env: &HomeEnv) -> SettingsPaths {
    let home = resolve_home(env);
    let dir = home.join(MODE_DATA_ROOT_DIR_NAME).join("v2");
    let file = dir.join("setting.json");
    SettingsPaths { home, dir, file }
}
