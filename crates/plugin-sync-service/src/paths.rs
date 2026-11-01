//! 路径口径（`pluginSyncPath.ts` 与用户配置/插件根）。

use std::path::{Path, PathBuf};

use plugin_discovery::js;

pub const MODE_DATA_ROOT_DIR_NAME: &str = ".mode";

/// `resolveUserHomeDir`：HOME → USERPROFILE → homedir。
pub fn default_home_dir() -> PathBuf {
    for key in ["HOME", "USERPROFILE"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return PathBuf::from(value.trim());
            }
        }
    }
    js::node_homedir()
}

pub fn user_mode_config_path(home: &Path) -> PathBuf {
    home.join(MODE_DATA_ROOT_DIR_NAME)
        .join("cli")
        .join("config.json")
}

pub fn user_mode_plugin_root(home: &Path) -> PathBuf {
    home.join(MODE_DATA_ROOT_DIR_NAME).join("plugins")
}

/// `normalizePluginSyncRelativePath`：拒绝绝对路径、反斜杠、`.`/`..`/空段。
pub fn normalize_relative_path(raw: &str) -> Result<String, String> {
    if raw.contains('\\') || Path::new(raw).is_absolute() || raw.starts_with('/') {
        return Err(format!("unsafe plugin sync path: {raw}"));
    }
    let bytes = raw.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(format!("unsafe plugin sync path: {raw}"));
    }
    let normalized = raw.trim_matches('/');
    if normalized.is_empty()
        || normalized
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("unsafe plugin sync path: {raw}"));
    }
    Ok(normalized.to_string())
}

/// `resolvePluginSyncPathWithin`：相对路径拼接后必须仍在 root 内。
pub fn resolve_within(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let normalized = normalize_relative_path(relative)?;
    let joined = normalized
        .split('/')
        .fold(js::normalize_lexical(root), |path, segment| {
            path.join(segment)
        });
    if !js::contains_path(root, &joined) || joined == js::normalize_lexical(root) {
        return Err(format!("unsafe plugin sync path: {relative}"));
    }
    Ok(joined)
}

/// TS `resolve(rawDir)`：相对路径按 cwd 解析。
pub fn resolve_user_path(raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        js::normalize_lexical(path)
    } else {
        std::env::current_dir()
            .map(|cwd| js::normalize_lexical(&cwd.join(path)))
            .unwrap_or_else(|_| js::normalize_lexical(path))
    }
}
