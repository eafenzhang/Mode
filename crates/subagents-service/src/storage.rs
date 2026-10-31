//! 存储路径解析。
//!
//! 真源：`packages/services/src/subagents/subagentStorage.ts`。

use std::path::{Path, PathBuf};

use plugin_discovery::js;

const MODE_DATA_ROOT_DIR_NAME: &str = ".mode";
const HOME_PREFIX: &str = "~/";

#[derive(Debug, Clone, Default)]
pub struct StorageOptions {
    pub home_dir: Option<PathBuf>,
}

impl StorageOptions {
    /// 壳内默认：`HOME.trim() || USERPROFILE.trim() || homedir()`。
    pub fn from_process() -> Self {
        let home = std::env::var("HOME")
            .ok()
            .map(|value| js::js_trim(&value).to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                std::env::var("USERPROFILE")
                    .ok()
                    .map(|value| js::js_trim(&value).to_string())
                    .filter(|value| !value.is_empty())
            })
            .map(PathBuf::from)
            .unwrap_or_else(js::node_homedir);
        Self {
            home_dir: Some(home),
        }
    }

    pub fn with_home(home_dir: PathBuf) -> Self {
        Self {
            home_dir: Some(home_dir),
        }
    }

    pub fn resolve_home(&self) -> PathBuf {
        if let Some(home) = &self.home_dir {
            if !js::js_trim(&home.to_string_lossy()).is_empty() {
                return home.clone();
            }
        }
        let env_home = std::env::var("HOME")
            .ok()
            .map(|value| js::js_trim(&value).to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                std::env::var("USERPROFILE")
                    .ok()
                    .map(|value| js::js_trim(&value).to_string())
                    .filter(|value| !value.is_empty())
            });
        env_home.map(PathBuf::from).unwrap_or_else(js::node_homedir)
    }
}

fn read_user_cli_config(options: &StorageOptions) -> serde_json::Map<String, serde_json::Value> {
    let path = options
        .resolve_home()
        .join(MODE_DATA_ROOT_DIR_NAME)
        .join("cli")
        .join("config.json");
    let Some(raw) = js::read_text(&path) else {
        return serde_json::Map::new();
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Object(object)) => object,
        _ => serde_json::Map::new(),
    }
}

pub fn resolve_mode_storage_root(options: &StorageOptions) -> PathBuf {
    let config = read_user_cli_config(options);
    let storage_dir = config
        .get("storage")
        .and_then(serde_json::Value::as_object)
        .and_then(|storage| storage.get("dir"))
        .and_then(serde_json::Value::as_str)
        .map(js::js_trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("~/.mode");
    resolve_config_path(options, storage_dir)
}

pub fn resolve_config_path(options: &StorageOptions, path: &str) -> PathBuf {
    let expanded = match path.strip_prefix(HOME_PREFIX) {
        Some(rest) => options.resolve_home().join(rest),
        None => PathBuf::from(path),
    };
    if js::is_absolute(&expanded.to_string_lossy()) {
        return expanded;
    }
    let base = std::env::current_dir().unwrap_or_default();
    js::normalize_lexical(&base.join(expanded))
}

pub fn resolve_user_subagent_root(options: &StorageOptions) -> PathBuf {
    resolve_mode_storage_root(options).join("agents")
}

pub fn resolve_workspace_subagent_root(workspace_path: &Path) -> PathBuf {
    workspace_path.join(MODE_DATA_ROOT_DIR_NAME).join("agents")
}

pub fn resolve_subagent_state_file(options: &StorageOptions) -> PathBuf {
    resolve_mode_storage_root(options)
        .join("v2")
        .join("agents-state.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_prefix_expands_and_relative_paths_resolve() {
        let options = StorageOptions::with_home(PathBuf::from("/home/user"));
        assert_eq!(
            resolve_config_path(&options, "~/.mode"),
            Path::new("/home/user/.mode")
        );
        let absolute = resolve_config_path(&options, "/srv/mode");
        assert_eq!(absolute, Path::new("/srv/mode"));
    }

    #[test]
    fn explicit_home_wins_over_environment() {
        let options = StorageOptions::with_home(PathBuf::from("/explicit"));
        assert_eq!(options.resolve_home(), Path::new("/explicit"));
    }
}
