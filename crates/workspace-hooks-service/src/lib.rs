//! Workspace Hook **保存路径**（Rust 实现）。
//!
//! 真源：`packages/services/src/hooks/workspaceHookSettingsModel.ts` 的写侧函数、
//! `packages/services/src/hooks/hooksService.ts` 的 `saveHooks` / `writeModeHooksConfig`、
//! `packages/shared/src/workspace-hook-mutation.ts` 的原子写与「已审核开关」。
//!
//! 读侧投影（`fromProjectSnapshot` / `fromLegacyHooksConfig` / `loadHooks`）不在这里：
//! 它们要接 trust store 与来源目录，留到 `loadHooks` 一起做。

mod atomic;
mod toggle;
mod writable;

pub use atomic::atomic_write_json;
pub use toggle::{write_workspace_hook_configured_toggle, WorkspaceHookMutationError};
pub use writable::{get_writable_hook, resolve_next_root_enabled, to_mode_hooks_events};

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use workspace_hook_config::path_to_forward_slashes;

/// UI 的 `Hook` 是 JSON 对象；保存路径只依赖其中少量字段，因此按 `Value` 处理避免类型漂移。
pub type Hook = Value;

fn object(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

/// `getWritableHook` 的 `...hook.custom`：自定义字段是嵌套的 `custom` 对象，
/// 不是 UI Hook 的未知顶层键（TS 的 `getCustomHookFields` 只用于读侧投影）。
pub(crate) fn custom_fields(hook: &Value) -> Map<String, Value> {
    object(hook)
        .and_then(|object| object.get("custom"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// `hook.enabled`；缺省视为 false（TS 里 `undefined` 为 falsy）。
pub(crate) fn hook_enabled(hook: &Value) -> bool {
    object(hook)
        .and_then(|object| object.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn is_mode_user_location(hook: &Value) -> bool {
    let Some(location) = object(hook).and_then(|object| object.get("location")) else {
        return false;
    };
    location.get("source").and_then(Value::as_str) == Some("mode")
        && location.get("scope").and_then(Value::as_str) == Some("user")
}

fn is_mode_project_location(hook: &Value) -> bool {
    let Some(location) = object(hook).and_then(|object| object.get("location")) else {
        return false;
    };
    location.get("source").and_then(Value::as_str) == Some("mode")
        && location.get("scope").and_then(Value::as_str) == Some("project")
}

fn is_editable(hook: &Value) -> bool {
    object(hook)
        .and_then(|object| object.get("editable"))
        .and_then(Value::as_bool)
        != Some(false)
}

fn configured_source_path(hook: &Value) -> Option<&str> {
    object(hook)
        .and_then(|object| object.get("configuredState"))
        .and_then(|state| state.get("sourcePath"))
        .and_then(Value::as_str)
}

/// `~/.mode/cli/config.json`（用户级设置）。
pub fn user_mode_config_path(home_dir: &Path) -> PathBuf {
    home_dir.join(".mode").join("cli").join("config.json")
}

/// 工程级设置路径：`<workspace>/.mode/config.json`。
pub fn project_mode_config_path(workspace_path: &Path) -> PathBuf {
    workspace_path.join(".mode").join("config.json")
}

/// 读取 JSON 对象；不存在或不可解析时按 `{}` 处理（与 TS `readJsonFile` 的保存侧一致）。
fn read_json_object(path: &Path) -> Map<String, Value> {
    match std::fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(Value::Object(object)) => object,
            _ => Map::new(),
        },
        Err(_) => Map::new(),
    }
}

/// 把一批 Hook 写回某个 `config.json` 的 `hooks` 字段（保留其余顶层键）。
pub fn write_mode_hooks_config(config_path: &Path, hooks: &[Hook]) -> std::io::Result<()> {
    let existing = read_json_object(config_path);
    let existing_hooks = existing.get("hooks").and_then(Value::as_object).cloned();
    let existing_enabled = existing_hooks
        .as_ref()
        .and_then(|hooks| hooks.get("enabled"))
        .and_then(Value::as_bool);
    let enabled = resolve_next_root_enabled(existing_enabled, hooks);
    let mut hooks_object = existing_hooks.unwrap_or_default();
    match enabled {
        Some(value) => {
            hooks_object.insert("enabled".into(), Value::Bool(value));
        }
        None => {
            hooks_object.remove("enabled");
        }
    }
    hooks_object.insert("events".into(), to_mode_hooks_events(hooks));
    let mut root = existing;
    root.insert("hooks".into(), Value::Object(hooks_object));
    atomic_write_json(config_path, &Value::Object(root))
}

/// `saveHooks`：把可编辑的 Hook 拆成用户级与工程级两份配置写回。
pub fn save_hooks(
    workspace_path: &Path,
    hooks: &[Hook],
    home_dir: &Path,
) -> std::io::Result<()> {
    let current_project_config = workspace_hook_config::resolve_path(&project_mode_config_path(workspace_path));
    let current_project_text = path_to_forward_slashes(&current_project_config);
    let user_hooks: Vec<Hook> = hooks
        .iter()
        .filter(|hook| is_editable(hook))
        .filter(|hook| {
            object(hook).and_then(|object| object.get("location")).is_none()
                || is_mode_user_location(hook)
        })
        .cloned()
        .collect();
    let project_hooks: Vec<Hook> = hooks
        .iter()
        .filter(|hook| is_editable(hook))
        .filter(|hook| is_mode_project_location(hook))
        .filter(|hook| match configured_source_path(hook) {
            None => true,
            Some(path) => path_to_forward_slashes(&workspace_hook_config::resolve_path(Path::new(path)))
                == current_project_text,
        })
        .cloned()
        .collect();
    write_mode_hooks_config(&user_mode_config_path(home_dir), &user_hooks)?;
    write_mode_hooks_config(&project_mode_config_path(workspace_path), &project_hooks)
}
