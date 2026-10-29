//! 「已审核开关」：只允许把审核时看到的同一条声明写回同一条工程配置。
//!
//! 对齐 TS `writeWorkspaceHookConfiguredToggle`：先校验目标仍是当前可编辑工程配置，
//! 再按审核时的 resolved 值重算声明摘要，磁盘上声明被改过就拒绝写入。

use std::path::Path;

use serde_json::Value;
use workspace_hook_config::{path_to_forward_slashes, resolve_path, validate_hooks_config};
use workspace_hook_digest::{
    declaration_digest, DeclarationInput, HookDefinition, Shell,
};

use crate::atomic::atomic_write_json;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceHookMutationError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for WorkspaceHookMutationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for WorkspaceHookMutationError {}

fn mismatch(message: &str) -> WorkspaceHookMutationError {
    WorkspaceHookMutationError {
        code: "workspace_hooks_snapshot_mismatch",
        message: message.to_string(),
    }
}

fn parse_shell(value: Option<&Value>) -> Shell {
    match value {
        None | Some(Value::Null) => Shell::Unset,
        Some(Value::Bool(true)) => Shell::Enabled,
        Some(Value::String(command)) => Shell::Command(command.clone()),
        _ => Shell::Unset,
    }
}

fn hook_definition(value: &Value) -> Option<HookDefinition> {
    let object = value.as_object()?;
    Some(HookDefinition {
        hook_type: object.get("type")?.as_str()?.to_string(),
        command: object.get("command")?.as_str()?.to_string(),
        args: object.get("args").and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        }),
        is_async: object.get("async").and_then(Value::as_bool),
        shell: parse_shell(object.get("shell")),
        timeout_ms: object.get("timeoutMs").and_then(Value::as_f64),
        timeout: object.get("timeout").and_then(Value::as_f64),
    })
}

fn path_text(value: &str) -> String {
    path_to_forward_slashes(&resolve_path(Path::new(value)))
}

/// 写入一条已审核声明的 `enabled`。
pub fn write_workspace_hook_configured_toggle(
    config_path: &Path,
    snapshot: &Value,
    review_item_id: &str,
    enabled: bool,
) -> Result<(), WorkspaceHookMutationError> {
    let config_path = resolve_path(config_path);
    let config_text = path_to_forward_slashes(&config_path);

    let entries = snapshot["hooks"].as_array().ok_or_else(|| {
        mismatch("Workspace Hook toggle target is not the current editable project config")
    })?;
    let entry = entries
        .iter()
        .find(|entry| entry.get("reviewItemId").and_then(Value::as_str) == Some(review_item_id))
        .ok_or_else(|| {
            mismatch("Workspace Hook toggle target is not the current editable project config")
        })?;
    let source_index = entry
        .get("sourceFileIndex")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX) as usize;
    let source = snapshot["sourceFiles"]
        .as_array()
        .and_then(|files| files.get(source_index))
        .ok_or_else(|| {
            mismatch("Workspace Hook toggle target is not the current editable project config")
        })?;

    let entry_editable = entry.get("editable").and_then(Value::as_bool) == Some(true);
    let source_editable = source.get("editable").and_then(Value::as_bool) == Some(true);
    let source_kind = source
        .get("configFileKind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let source_path = source
        .get("canonicalPath")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !entry_editable || !source_editable || source_kind != ".mode/config.json"
        || path_text(source_path) != config_text
    {
        return Err(mismatch(
            "Workspace Hook toggle target is not the current editable project config",
        ));
    }

    // 读不出来与写入失败必须分开报：用户据此知道该修配置文件而不是反复重试写入。
    let unreadable = || WorkspaceHookMutationError {
        code: "workspace_hooks_config_unreadable",
        message: "Workspace Hook config could not be read".to_string(),
    };
    let raw_text = std::fs::read_to_string(&config_path).map_err(|_| unreadable())?;
    let raw: Value = serde_json::from_str(&raw_text).map_err(|_| unreadable())?;
    let Some(root) = raw.as_object() else {
        return Err(mismatch("Workspace Hook config root is not an object"));
    };
    let Some(raw_hooks) = root.get("hooks") else {
        return Err(mismatch("Workspace Hook config no longer matches its schema"));
    };
    validate_hooks_config(raw_hooks)
        .map_err(|_| mismatch("Workspace Hook config no longer matches its schema"))?;
    let Some(raw_hooks) = raw_hooks.as_object() else {
        return Err(mismatch("Workspace Hook config has no hooks object"));
    };
    let event = entry
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let matcher_index = entry
        .get("matcherIndex")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX) as usize;
    let hook_index = entry
        .get("hookIndex")
        .and_then(Value::as_u64)
        .unwrap_or(u64::MAX) as usize;
    let Some(events) = raw_hooks.get("events").and_then(Value::as_object) else {
        return Err(mismatch("Workspace Hook config has no events object"));
    };
    let Some(matchers) = events.get(event).and_then(Value::as_array) else {
        return Err(mismatch("Workspace Hook event no longer exists"));
    };
    let Some(matcher) = matchers.get(matcher_index).and_then(Value::as_object) else {
        return Err(mismatch("Workspace Hook matcher no longer exists"));
    };
    let Some(declarations) = matcher.get("hooks").and_then(Value::as_array) else {
        return Err(mismatch("Workspace Hook matcher no longer exists"));
    };
    let Some(declaration) = declarations.get(hook_index) else {
        return Err(mismatch("Workspace Hook declaration no longer exists"));
    };
    let Some(definition) = hook_definition(declaration) else {
        return Err(mismatch(
            "Workspace Hook declaration no longer matches its schema",
        ));
    };
    let matcher_text = matcher
        .get("matcher")
        .and_then(Value::as_str)
        .map(str::to_string);
    let current_digest = declaration_digest(&DeclarationInput {
        source_relative_path: entry
            .get("sourceRelativePath")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        source_discovery_order: source
            .get("discoveryOrder")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        event: event.to_string(),
        matcher: matcher_text,
        matcher_index: matcher_index as u64,
        hook_index: hook_index as u64,
        hook: definition,
        resolved_timeout_ms: entry
            .get("resolvedTimeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        resolved_max_output_bytes: entry
            .get("resolvedMaxOutputBytes")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    });
    let expected_digest = entry
        .get("hookDeclarationDigest")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if current_digest != expected_digest {
        return Err(mismatch("Workspace Hook declaration changed after review"));
    }

    let mut root = root.clone();
    let mut hooks = raw_hooks.clone();
    let mut events = events.clone();
    let mut matchers = matchers.clone();
    let mut matcher = matcher.clone();
    let mut declarations = declarations.clone();
    let mut declaration = declaration.clone();
    if let Some(object) = declaration.as_object_mut() {
        object.insert("enabled".into(), Value::Bool(enabled));
    } else {
        return Err(mismatch(
            "Workspace Hook declaration no longer matches its schema",
        ));
    }
    declarations[hook_index] = declaration;
    matcher.insert("hooks".into(), Value::Array(declarations));
    matchers[matcher_index] = Value::Object(matcher);
    events.insert(event.to_string(), Value::Array(matchers));
    hooks.insert("events".into(), Value::Object(events));
    root.insert("hooks".into(), Value::Object(hooks));
    atomic_write_json(&config_path, &Value::Object(root)).map_err(|error| {
        WorkspaceHookMutationError {
            code: "workspace_hooks_config_write_failed",
            message: format!("Workspace Hook config write failed: {error}"),
        }
    })
}
