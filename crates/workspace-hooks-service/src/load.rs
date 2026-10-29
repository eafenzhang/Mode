//! Hook 读侧：把配置来源投影成 UI 的 `Hook[]`，并组装 `loadHooks` 结果。
//!
//! 真源：`packages/services/src/hooks/workspaceHookSettingsModel.ts`（`fromProjectSnapshot` /
//! `fromUserModeSource` / `fromLegacyHooksConfig` / `toHook`）与
//! `packages/services/src/hooks/hooksService.ts`（`loadHooks`）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use workspace_hook_config::{
    create_workspace_hook_source_input, path_to_forward_slashes, read_workspace_hook_project_sources,
    resolve_path, resolve_workspace_hook_runtime_root, validate_hooks_config, RuntimeRoot,
    RuntimeRootInput, SourceInput,
};
use workspace_hook_digest::{resolve_entries, HookEntry, Snapshot};
use workspace_hook_trust::parse_trust_store;

use crate::user_mode_config_path;

const KNOWN_DECLARATION_FIELDS: [&str; 9] = [
    "type",
    "command",
    "args",
    "async",
    "enabled",
    "shell",
    "statusMessage",
    "timeout",
    "timeoutMs",
];

#[derive(Debug, Clone, PartialEq)]
pub struct LoadHooksResult {
    pub hooks: Vec<Value>,
    pub hooks_enabled: bool,
    pub snapshot: Option<Value>,
    pub trust_store_corrupt: bool,
}

fn hook_events(source: &SourceInput) -> Option<&Map<String, Value>> {
    source.hooks.get("events").and_then(Value::as_object)
}

fn raw_declaration<'a>(
    source: &'a SourceInput,
    event: &str,
    matcher_index: usize,
    hook_index: usize,
) -> Option<&'a Value> {
    hook_events(source)?
        .get(event)?
        .as_array()?
        .get(matcher_index)?
        .get("hooks")?
        .as_array()?
        .get(hook_index)
}

fn optional_number(value: &Value) -> Option<f64> {
    value.as_f64()
}

/// `toHook` 的 `timeout` 字段：command 优先取秒级 `timeout`，否则由 `timeoutMs` 换算。
fn hook_timeout(raw: &Value) -> Option<f64> {
    let hook_type = raw.get("type").and_then(Value::as_str);
    if hook_type == Some("command") {
        if let Some(timeout) = optional_number(raw.get("timeout").unwrap_or(&Value::Null)) {
            return Some(timeout);
        }
    }
    // TS 是 `Math.round(ms) / 1000`（先取整再换算），顺序写反会把 2500ms 变成 3 秒。
    raw.get("timeoutMs")
        .and_then(Value::as_f64)
        .map(|value| value.round() / 1000.0)
}

fn raw_custom(raw: &Value) -> Option<Value> {
    let object = raw.as_object()?;
    let mut custom = Map::new();
    for (key, value) in object {
        if !KNOWN_DECLARATION_FIELDS.contains(&key.as_str()) {
            custom.insert(key.clone(), value.clone());
        }
    }
    if custom.is_empty() {
        None
    } else {
        Some(Value::Object(custom))
    }
}

fn dirname_text(path: &Path) -> String {
    path.parent()
        .map(path_to_forward_slashes)
        .unwrap_or_default()
}

struct Projection<'a> {
    id: String,
    location: Value,
    workspace_hook: Option<Value>,
    editable: bool,
    _marker: std::marker::PhantomData<&'a ()>,
}

/// 由一条已解析条目 + 原始声明生成 UI Hook。
fn project_entry(
    entry: &HookEntry,
    source: &SourceInput,
    projection: Projection<'_>,
) -> Option<Value> {
    let raw = raw_declaration(
        source,
        &entry.event,
        entry.matcher_index,
        entry.hook_index,
    )?;
    let configured_state = {
        let mut state = Map::new();
        state.insert("sourceRootEnabled".into(), json!(entry.source_root_enabled));
        state.insert("declarationEnabled".into(), json!(entry.declaration_enabled));
        state.insert("runtimeHooksEnabled".into(), json!(entry.runtime_hooks_enabled));
        state.insert("configuredEnabled".into(), json!(entry.configured_enabled));
        state.insert(
            "sourcePath".into(),
            json!(path_to_forward_slashes(&source.canonical_path)),
        );
        Value::Object(state)
    };
    let mut hook = Map::new();
    hook.insert("id".into(), json!(projection.id));
    hook.insert("event".into(), json!(entry.event));
    if let Some(matcher) = &entry.matcher {
        hook.insert("matcher".into(), json!(matcher));
    }
    hook.insert("type".into(), json!(entry.hook_type));
    hook.insert("command".into(), json!(entry.command));
    if entry.hook_type == "process" {
        hook.insert(
            "args".into(),
            json!(entry.args.clone().unwrap_or_default()),
        );
    } else {
        if entry.is_async {
            hook.insert("async".into(), json!(true));
        }
        if !matches!(entry.shell, workspace_hook_digest::Shell::Unset) {
            let shell = match &entry.shell {
                workspace_hook_digest::Shell::Enabled => json!(true),
                workspace_hook_digest::Shell::Command(command) => json!(command),
                workspace_hook_digest::Shell::Unset => Value::Null,
            };
            hook.insert("shell".into(), shell);
        }
    }
    if let Some(status_message) = &entry.status_message {
        hook.insert("statusMessage".into(), json!(status_message));
    }
    if let Some(timeout) = hook_timeout(raw) {
        hook.insert("timeout".into(), serde_json::Number::from_f64(timeout)
            .map(Value::Number)
            .unwrap_or(Value::Null));
    }
    hook.insert("enabled".into(), json!(entry.configured_enabled));
    hook.insert("editable".into(), json!(projection.editable));
    hook.insert("configuredState".into(), configured_state.clone());
    if let Some(extra) = projection.workspace_hook {
        let mut workspace_hook = configured_state.as_object().cloned().unwrap_or_default();
        workspace_hook.insert("reviewItemId".into(), json!(entry.review_item_id));
        workspace_hook.insert("sourceFileIndex".into(), json!(entry.source_file_index));
        if let Some(extra) = extra.as_object() {
            for (key, value) in extra {
                workspace_hook.insert(key.clone(), value.clone());
            }
        }
        hook.insert("workspaceHook".into(), Value::Object(workspace_hook));
    }
    if let Some(custom) = raw_custom(raw) {
        hook.insert("custom".into(), custom);
    }
    hook.insert("location".into(), projection.location);
    Some(Value::Object(hook))
}

/// 工程来源投影：每条条目都带 trust 状态与 review 键。
pub fn from_project_snapshot(
    sources: &[SourceInput],
    snapshot: &Value,
    workspace_path: &str,
    workspace_identity: &str,
    persistent_trusted: &HashSet<String>,
) -> Vec<Value> {
    let Some(entries) = snapshot.get("hooks").and_then(Value::as_array) else {
        return Vec::new();
    };
    let bundle_digest = snapshot
        .get("bundleDigest")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut hooks = Vec::new();
    for entry in entries {
        let source_index = entry
            .get("sourceFileIndex")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX) as usize;
        let Some(source) = sources.get(source_index) else {
            continue;
        };
        let Some(parsed) = entry_to_hook_entry(entry) else {
            continue;
        };
        let digest = parsed.hook_declaration_digest.clone();
        let trust_state = if persistent_trusted.contains(&digest) {
            "trusted_persistent"
        } else {
            "pending_trust"
        };
        let location = json!({
            "source": "mode",
            "scope": "project",
            "directoryPath": dirname_text(&source.canonical_path),
            "projectPath": workspace_path,
        });
        let workspace_hook = json!({
            "workspaceIdentity": workspace_identity,
            "bundleDigest": bundle_digest,
            "hookDeclarationDigest": digest,
            "trustState": trust_state,
        });
        if let Some(hook) = project_entry(
            &parsed,
            source,
            Projection {
                id: parsed.review_item_id.clone(),
                location,
                workspace_hook: Some(workspace_hook),
                editable: source.editable,
                _marker: std::marker::PhantomData,
            },
        ) {
            hooks.push(hook);
        }
    }
    hooks
}

/// 用户级 mode 来源投影（id 按条目下标生成）。
pub fn from_user_mode_source(
    source: Option<&SourceInput>,
    runtime_root: &RuntimeRoot,
    workspace_path: &str,
    location: Value,
) -> Vec<Value> {
    let Some(source) = source else {
        return Vec::new();
    };
    resolve_entries(workspace_path, std::slice::from_ref(source), runtime_root)
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            project_entry(
                entry,
                source,
                Projection {
                    id: format!("hook-mode-user-{index}"),
                    location: location.clone(),
                    workspace_hook: None,
                    editable: source.editable,
                    _marker: std::marker::PhantomData,
                },
            )
        })
        .collect()
}

fn is_hook_event(value: &str) -> bool {
    workspace_hook_config::WORKSPACE_HOOK_EVENT_NAMES.contains(&value)
}

/// legacy `settings.json` 形状（`{hooks: {事件: [matcher]}}`）的投影；一律不可编辑、默认关闭。
pub fn from_legacy_hooks_config(legacy: Option<&Value>, location: &Value) -> Vec<Value> {
    let Some(hooks_root) = legacy.and_then(|config| config.get("hooks")).and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let source_name = location
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let scope = location
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut hooks = Vec::new();
    let mut counter = 0usize;
    for (event, matchers) in hooks_root {
        if !is_hook_event(event) {
            continue;
        }
        let Some(matchers) = matchers.as_array() else {
            continue;
        };
        for matcher in matchers {
            let Some(declarations) = matcher.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for hook in declarations {
                let hook_type = hook.get("type").and_then(Value::as_str).unwrap_or_default();
                let command = hook.get("command").and_then(Value::as_str).unwrap_or_default();
                if !matches!(hook_type, "command" | "process") || command.is_empty() {
                    continue;
                }
                let mut out = Map::new();
                out.insert(
                    "id".into(),
                    json!(format!("hook-{source_name}-{scope}-{counter}")),
                );
                counter += 1;
                out.insert("event".into(), json!(event));
                if let Some(matcher) = matcher.get("matcher") {
                    out.insert("matcher".into(), matcher.clone());
                }
                out.insert("type".into(), json!(hook_type));
                out.insert("command".into(), json!(command));
                if hook_type == "process" {
                    out.insert(
                        "args".into(),
                        hook.get("args").cloned().unwrap_or_else(|| json!([])),
                    );
                }
                if let Some(is_async) = hook.get("async") {
                    out.insert("async".into(), is_async.clone());
                }
                if let Some(shell) = hook.get("shell") {
                    out.insert("shell".into(), shell.clone());
                }
                if let Some(status_message) = hook
                    .get("statusMessage")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    out.insert("statusMessage".into(), json!(status_message));
                }
                let timeout = hook
                    .get("timeout")
                    .and_then(Value::as_f64)
                    .or_else(|| {
                        hook.get("timeoutMs")
                            .and_then(Value::as_f64)
                            .map(|value| (value / 1000.0).round())
                    });
                // 缺省时 TS 的键是 undefined（JSON 里不出现），不能写成 null。
                if let Some(timeout) = timeout {
                    out.insert("timeout".into(), json!(timeout));
                }
                out.insert("enabled".into(), json!(false));
                out.insert("editable".into(), json!(false));
                out.insert("location".into(), location.clone());
                hooks.push(Value::Object(out));
            }
        }
    }
    hooks
}

/// 把快照里的条目 JSON 还原成条目结构（快照本身是 JSON，读侧不再依赖序列化派生）。
fn entry_to_hook_entry(entry: &Value) -> Option<HookEntry> {
    let shell = match entry.get("shell") {
        None => workspace_hook_digest::Shell::Unset,
        Some(Value::Bool(true)) => workspace_hook_digest::Shell::Enabled,
        Some(Value::String(command)) => workspace_hook_digest::Shell::Command(command.clone()),
        Some(_) => workspace_hook_digest::Shell::Unset,
    };
    Some(HookEntry {
        review_item_id: entry.get("reviewItemId")?.as_str()?.to_string(),
        event: entry.get("event")?.as_str()?.to_string(),
        matcher_index: entry.get("matcherIndex")?.as_u64()? as usize,
        hook_index: entry.get("hookIndex")?.as_u64()? as usize,
        source_file_index: entry.get("sourceFileIndex")?.as_u64()? as usize,
        source_relative_path: entry.get("sourceRelativePath")?.as_str()?.to_string(),
        matcher: entry.get("matcher").and_then(Value::as_str).map(str::to_string),
        hook_type: entry.get("type")?.as_str()?.to_string(),
        command: entry.get("command")?.as_str()?.to_string(),
        args: entry.get("args").and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        }),
        is_async: entry.get("async").and_then(Value::as_bool) == Some(true),
        shell,
        resolved_timeout_ms: entry.get("resolvedTimeoutMs")?.as_u64()?,
        resolved_max_output_bytes: entry.get("resolvedMaxOutputBytes")?.as_u64()?,
        status_message: entry
            .get("statusMessage")
            .and_then(Value::as_str)
            .map(str::to_string),
        source_root_enabled: entry.get("sourceRootEnabled")?.as_bool()?,
        declaration_enabled: entry.get("declarationEnabled")?.as_bool()?,
        runtime_hooks_enabled: entry.get("runtimeHooksEnabled")?.as_bool()?,
        configured_enabled: entry.get("configuredEnabled")?.as_bool()?,
        editable: entry.get("editable").and_then(Value::as_bool).unwrap_or(false),
        hook_declaration_digest: entry.get("hookDeclarationDigest")?.as_str()?.to_string(),
    })
}

fn read_json_object(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<Value>(&raw) {
        Ok(value @ Value::Object(_)) => Some(value),
        _ => None,
    }
}

fn read_user_mode_source(home_dir: &Path) -> Option<SourceInput> {
    let path = user_mode_config_path(home_dir);
    let config = read_json_object(&path)?;
    let hooks = config.get("hooks")?;
    validate_hooks_config(hooks).ok()?;
    let mut source = create_workspace_hook_source_input(&path, home_dir, hooks.clone(), 0, true);
    source.editable = true;
    Some(source)
}

fn runtime_root_from_hooks(hooks: &Value) -> RuntimeRootInput {
    RuntimeRootInput {
        enabled: hooks.get("enabled").and_then(Value::as_bool),
        timeout_ms: hooks.get("timeoutMs").and_then(Value::as_f64),
        max_output_bytes: hooks.get("maxOutputBytes").and_then(Value::as_f64),
    }
}

/// 与 TS 相同的 storage.dir 解析：`~/` 展开、绝对路径原样、相对路径相对 home。
fn storage_root(user_config: Option<&Value>, home_dir: &Path) -> PathBuf {
    let configured = user_config
        .and_then(|config| config.get("storage"))
        .and_then(|storage| storage.get("dir"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty());
    match configured {
        None => home_dir.join(".mode"),
        Some(value) if value.starts_with("~/") => home_dir.join(&value[2..]),
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                resolve_path(&path)
            } else {
                resolve_path(&home_dir.join(path))
            }
        }
    }
}

/// 读取持久信任；返回 (`digests`, `corrupt`)。
fn read_persistent_trust(
    workspace_identity: &str,
    home_dir: &Path,
    user_config: Option<&Value>,
) -> (HashSet<String>, bool) {
    let trust_path = storage_root(user_config, home_dir)
        .join("security")
        .join("workspace-hook-trust-v1.json");
    let content = match std::fs::read_to_string(&trust_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (HashSet::new(), false)
        }
        Err(_) => return (HashSet::new(), true),
    };
    match parse_trust_store(&content) {
        None => (HashSet::new(), true),
        Some(store) => (
            store
                .records
                .iter()
                .filter(|record| record.workspace_identity == workspace_identity)
                .map(|record| record.hook_declaration_digest.clone())
                .collect(),
            false,
        ),
    }
}

fn legacy_location(source: &str, workspace_path: Option<&Path>, home_dir: &Path) -> Value {
    let base = workspace_path.unwrap_or(home_dir);
    let directory = base.join(if source == "agents" { ".agents" } else { ".claude" });
    let mut location = Map::new();
    location.insert("source".into(), json!(source));
    location.insert(
        "scope".into(),
        json!(if workspace_path.is_some() { "project" } else { "user" }),
    );
    location.insert("directoryPath".into(), json!(path_to_forward_slashes(&directory)));
    if let Some(workspace) = workspace_path {
        location.insert("projectPath".into(), json!(path_to_forward_slashes(workspace)));
    }
    Value::Object(location)
}

fn load_legacy(
    source: &str,
    workspace_path: Option<&Path>,
    home_dir: &Path,
) -> Vec<Value> {
    let base = workspace_path.unwrap_or(home_dir);
    let directory = base.join(if source == "agents" { ".agents" } else { ".claude" });
    let config = read_json_object(&directory.join("settings.json"));
    from_legacy_hooks_config(
        config.as_ref(),
        &legacy_location(source, workspace_path, home_dir),
    )
}

/// `loadHooks`：工程来源 + 用户级 mode + 四处 legacy 配置合并。
pub fn load_hooks(
    workspace_path: &Path,
    home_dir: &Path,
    discovered_at: &str,
) -> LoadHooksResult {
    let workspace_path = resolve_path(workspace_path);
    let workspace_text = path_to_forward_slashes(&workspace_path);
    let workspace_identity = workspace_text.clone();

    let (project_sources, _errors) = read_workspace_hook_project_sources(&workspace_path, None);
    let user_config = read_json_object(&user_mode_config_path(home_dir));
    let user_source = read_user_mode_source(home_dir);

    let mut root_inputs: Vec<Option<RuntimeRootInput>> = Vec::new();
    if let Some(source) = &user_source {
        root_inputs.push(Some(runtime_root_from_hooks(&source.hooks)));
    }
    for source in &project_sources {
        root_inputs.push(Some(runtime_root_from_hooks(&source.hooks)));
    }
    let runtime_root: RuntimeRoot = resolve_workspace_hook_runtime_root(&root_inputs);

    let snapshot = workspace_hook_digest::build_bundle_snapshot(
        &workspace_identity,
        &workspace_text,
        &project_sources,
        &runtime_root,
        discovered_at,
    );
    let (trusted, corrupt) = read_persistent_trust(&workspace_identity, home_dir, user_config.as_ref());

    let mut hooks = Vec::new();
    if let Some(snapshot) = &snapshot {
        hooks.extend(from_project_snapshot(
            &project_sources,
            &snapshot.to_json(),
            &workspace_text,
            &workspace_identity,
            &trusted,
        ));
    }
    hooks.extend(load_legacy("agents", Some(&workspace_path), home_dir));
    hooks.extend(load_legacy("claude", Some(&workspace_path), home_dir));
    hooks.extend(from_user_mode_source(
        user_source.as_ref(),
        &runtime_root,
        &workspace_text,
        json!({
            "source": "mode",
            "scope": "user",
            "directoryPath": path_to_forward_slashes(&home_dir.join(".mode").join("cli")),
        }),
    ));
    hooks.extend(load_legacy("agents", None, home_dir));
    hooks.extend(load_legacy("claude", None, home_dir));

    let hooks_enabled = hooks.iter().any(|hook| {
        hook.get("enabled").and_then(Value::as_bool) == Some(true)
    });
    LoadHooksResult {
        hooks,
        hooks_enabled,
        snapshot: snapshot.map(|snapshot: Snapshot| snapshot.to_json()),
        trust_store_corrupt: corrupt,
    }
}
