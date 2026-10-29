//! 默认项目目录（`setting.ensureDefaultProject`）。
//!
//! 真源：`packages/services/src/setting/settingService.ts` 的同名方法——在用户 home 下建
//! `ModeProject/`，返回路径与「本次是否新建」。只做目录，不动 settings（TS 也不动）。

use std::path::{Path, PathBuf};

pub const DEFAULT_PROJECT_NAME: &str = "ModeProject";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultProject {
    pub path: PathBuf,
    pub created: bool,
}

/// 幂等：已存在时 `created=false`，目录创建失败原样返回 IO 错误。
pub fn ensure_default_project(user_home: &Path) -> std::io::Result<DefaultProject> {
    let path = user_home.join(DEFAULT_PROJECT_NAME);
    let existed = path.exists();
    std::fs::create_dir_all(&path)?;
    Ok(DefaultProject {
        path,
        created: !existed,
    })
}
