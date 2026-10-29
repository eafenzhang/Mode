//! Hook 条目解析与包级快照（对齐 TS `resolveWorkspaceHookEntries` /
//! `buildWorkspaceHookBundleSnapshot`）。
//!
//! 条目顺序 = `WORKSPACE_HOOK_EVENT_NAMES` × matcher × hook；`reviewItemId` 里带
//! sourceFileIndex / 事件名 / 两个下标，是 UI 审核与 trust 授权的稳定键，任何重排都会
//! 让已有授权失效。

use serde_json::{json, Map, Value};
use workspace_hook_config::{
    resolve_configured_gates, resolve_timeout_ms, RuntimeRoot, SourceInput as ConfigSourceInput,
    WORKSPACE_HOOK_EVENT_NAMES,
};

use crate::{
    bundle_digest, declaration_digest, normalize_relative_source_path, DeclarationInput,
    DigestHook, HookDefinition, Shell, SourceInput as DigestSourceInput, DIGEST_SCHEMA_VERSION,
};

#[derive(Debug, Clone, PartialEq)]
pub struct HookEntry {
    pub review_item_id: String,
    pub event: String,
    pub matcher_index: usize,
    pub hook_index: usize,
    pub source_file_index: usize,
    pub source_relative_path: String,
    pub matcher: Option<String>,
    pub hook_type: String,
    pub command: String,
    pub args: Option<Vec<String>>,
    pub is_async: bool,
    pub shell: Shell,
    pub resolved_timeout_ms: u64,
    pub resolved_max_output_bytes: u64,
    pub status_message: Option<String>,
    pub source_root_enabled: bool,
    pub declaration_enabled: bool,
    pub runtime_hooks_enabled: bool,
    pub configured_enabled: bool,
    pub editable: bool,
    pub hook_declaration_digest: String,
}

impl HookEntry {
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("reviewItemId".into(), json!(self.review_item_id));
        object.insert("event".into(), json!(self.event));
        object.insert("matcherIndex".into(), json!(self.matcher_index));
        object.insert("hookIndex".into(), json!(self.hook_index));
        object.insert("sourceFileIndex".into(), json!(self.source_file_index));
        object.insert("sourceRelativePath".into(), json!(self.source_relative_path));
        object.insert("matcher".into(), json!(self.matcher));
        object.insert("type".into(), json!(self.hook_type));
        object.insert("command".into(), json!(self.command));
        if self.hook_type == "process" {
            if let Some(args) = &self.args {
                if !args.is_empty() {
                    object.insert("args".into(), json!(args));
                }
            }
        } else {
            if self.is_async {
                object.insert("async".into(), json!(true));
            }
            match &self.shell {
                Shell::Unset => {}
                Shell::Enabled => {
                    object.insert("shell".into(), json!(true));
                }
                Shell::Command(command) => {
                    object.insert("shell".into(), json!(command));
                }
            }
        }
        object.insert("resolvedTimeoutMs".into(), json!(self.resolved_timeout_ms));
        object.insert(
            "resolvedMaxOutputBytes".into(),
            json!(self.resolved_max_output_bytes),
        );
        if let Some(status_message) = &self.status_message {
            object.insert("statusMessage".into(), json!(status_message));
        }
        object.insert("sourceRootEnabled".into(), json!(self.source_root_enabled));
        object.insert("declarationEnabled".into(), json!(self.declaration_enabled));
        object.insert("runtimeHooksEnabled".into(), json!(self.runtime_hooks_enabled));
        object.insert("configuredEnabled".into(), json!(self.configured_enabled));
        object.insert("editable".into(), json!(self.editable));
        object.insert("declarationDigestAlgorithm".into(), json!("sha256"));
        object.insert("hookDeclarationDigest".into(), json!(self.hook_declaration_digest));
        Value::Object(object)
    }

    fn digest_hook(&self) -> DigestHook {
        DigestHook {
            hook_declaration_digest: self.hook_declaration_digest.clone(),
            source_root_enabled: self.source_root_enabled,
            declaration_enabled: self.declaration_enabled,
            runtime_hooks_enabled: self.runtime_hooks_enabled,
            configured_enabled: self.configured_enabled,
        }
    }

    fn definition(&self) -> HookDefinition {
        HookDefinition {
            hook_type: self.hook_type.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            is_async: if self.is_async { Some(true) } else { None },
            shell: self.shell.clone(),
            timeout_ms: None,
            timeout: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub workspace_identity: String,
    pub discovered_at: String,
    pub source_files: Vec<Value>,
    pub entries: Vec<HookEntry>,
    pub bundle_digest: String,
}

impl Snapshot {
    pub fn to_json(&self) -> Value {
        json!({
            "schemaVersion": DIGEST_SCHEMA_VERSION,
            "workspaceIdentity": self.workspace_identity,
            "discoveredAt": self.discovered_at,
            "sourceFiles": self.source_files,
            "hooks": self.entries.iter().map(HookEntry::to_json).collect::<Vec<_>>(),
            "digestAlgorithm": "sha256",
            "bundleDigest": self.bundle_digest,
        })
    }
}

struct RawHook {
    hook_type: String,
    command: String,
    args: Option<Vec<String>>,
    is_async: bool,
    shell: Shell,
    timeout_ms: Option<f64>,
    timeout: Option<f64>,
    status_message: Option<String>,
    enabled: Option<bool>,
}

impl RawHook {
    fn definition(&self) -> HookDefinition {
        HookDefinition {
            hook_type: self.hook_type.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            is_async: Some(self.is_async),
            shell: self.shell.clone(),
            timeout_ms: self.timeout_ms,
            timeout: self.timeout,
        }
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

fn parse_hook(value: &Value) -> Option<RawHook> {
    let object = value.as_object()?;
    Some(RawHook {
        hook_type: object.get("type")?.as_str()?.to_string(),
        command: object.get("command")?.as_str()?.to_string(),
        args: object.get("args").and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        }),
        is_async: object.get("async").and_then(Value::as_bool) == Some(true),
        shell: parse_shell(object.get("shell")),
        timeout_ms: object.get("timeoutMs").and_then(Value::as_f64),
        timeout: object.get("timeout").and_then(Value::as_f64),
        status_message: object
            .get("statusMessage")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        enabled: object.get("enabled").and_then(Value::as_bool),
    })
}

fn events_of(source: &ConfigSourceInput) -> Option<&Map<String, Value>> {
    source.hooks.get("events").and_then(Value::as_object)
}

/// 把一份份配置展开成可审核条目。
pub fn resolve_entries(
    workspace_path: &str,
    sources: &[ConfigSourceInput],
    runtime_root: &RuntimeRoot,
) -> Vec<HookEntry> {
    let mut entries = Vec::new();
    for (source_file_index, source) in sources.iter().enumerate() {
        let canonical_text = source.canonical_path.to_string_lossy();
        let source_relative_path =
            normalize_relative_source_path(workspace_path, canonical_text.as_ref());
        let source_enabled = source.hooks.get("enabled").and_then(Value::as_bool);
        let events = events_of(source);
        for event in WORKSPACE_HOOK_EVENT_NAMES {
            let Some(matchers) = events.and_then(|events| events.get(event)).and_then(Value::as_array)
            else {
                continue;
            };
            for (matcher_index, matcher) in matchers.iter().enumerate() {
                let Some(hooks) = matcher.get("hooks").and_then(Value::as_array) else {
                    continue;
                };
                let matcher_text = matcher
                    .get("matcher")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                for (hook_index, value) in hooks.iter().enumerate() {
                    let Some(hook) = parse_hook(value) else {
                        continue;
                    };
                    let gates = resolve_configured_gates(
                        source_enabled,
                        hook.enabled,
                        runtime_root.enabled,
                    );
                    let resolved_timeout_ms = resolve_timeout_ms(
                        &hook.hook_type,
                        hook.timeout_ms,
                        hook.timeout,
                        runtime_root.timeout_ms,
                    );
                    let digest = declaration_digest(&DeclarationInput {
                        source_relative_path: source_relative_path.clone(),
                        source_discovery_order: source.discovery_order as u64,
                        event: event.to_string(),
                        matcher: matcher_text.clone(),
                        matcher_index: matcher_index as u64,
                        hook_index: hook_index as u64,
                        hook: hook.definition(),
                        resolved_timeout_ms,
                        resolved_max_output_bytes: runtime_root.max_output_bytes,
                    });
                    entries.push(HookEntry {
                        review_item_id: format!(
                            "workspace-hook-{source_file_index}-{event}-{matcher_index}-{hook_index}"
                        ),
                        event: event.to_string(),
                        matcher_index,
                        hook_index,
                        source_file_index,
                        source_relative_path: source_relative_path.clone(),
                        matcher: matcher_text.clone(),
                        hook_type: hook.hook_type.clone(),
                        command: hook.command.clone(),
                        args: hook.args.clone(),
                        is_async: hook.is_async,
                        shell: hook.shell.clone(),
                        resolved_timeout_ms,
                        resolved_max_output_bytes: runtime_root.max_output_bytes,
                        status_message: hook.status_message.clone(),
                        source_root_enabled: gates.source_root_enabled,
                        declaration_enabled: gates.declaration_enabled,
                        runtime_hooks_enabled: gates.runtime_hooks_enabled,
                        configured_enabled: gates.configured_enabled,
                        editable: source.editable,
                        hook_declaration_digest: digest,
                    });
                }
            }
        }
    }
    entries
}

fn source_hooks_root(source: &ConfigSourceInput) -> Value {
    let mut root = Map::new();
    if let Some(enabled) = source.hooks.get("enabled") {
        root.insert("enabled".into(), enabled.clone());
    }
    if let Some(timeout_ms) = source.hooks.get("timeoutMs") {
        root.insert("timeoutMs".into(), timeout_ms.clone());
    }
    if let Some(max_output_bytes) = source.hooks.get("maxOutputBytes") {
        root.insert("maxOutputBytes".into(), max_output_bytes.clone());
    }
    Value::Object(root)
}

fn digest_source(source: &ConfigSourceInput) -> DigestSourceInput {
    DigestSourceInput {
        cache_key: source.canonical_path.to_string_lossy().into_owned(),
        canonical_path: source.canonical_path.to_string_lossy().into_owned(),
        discovery_order: source.discovery_order as u64,
        config_file_kind: source.config_file_kind.to_string(),
        explicit_project_config: source.explicit_project_config,
        enabled: source.hooks.get("enabled").cloned(),
        timeout_ms: source.hooks.get("timeoutMs").cloned(),
        max_output_bytes: source.hooks.get("maxOutputBytes").cloned(),
    }
}

/// 组装包级快照；没有任何条目时返回 `None`（TS 同口径）。
pub fn build_bundle_snapshot(
    workspace_identity: &str,
    workspace_path: &str,
    sources: &[ConfigSourceInput],
    runtime_root: &RuntimeRoot,
    discovered_at: &str,
) -> Option<Snapshot> {
    let entries = resolve_entries(workspace_path, sources, runtime_root);
    if entries.is_empty() {
        return None;
    }
    let digest_sources: Vec<DigestSourceInput> = sources.iter().map(digest_source).collect();
    let digest_hooks: Vec<DigestHook> = entries.iter().map(HookEntry::digest_hook).collect();
    let bundle = bundle_digest(workspace_path, &digest_sources, &digest_hooks);
    let source_files: Vec<Value> = sources
        .iter()
        .map(|source| {
            json!({
                "canonicalPath": source.canonical_path.to_string_lossy(),
                "baseDir": source.base_dir.to_string_lossy(),
                "discoveryOrder": source.discovery_order,
                "configFileKind": source.config_file_kind,
                "explicitProjectConfig": source.explicit_project_config,
                "editable": source.editable,
                "hooksRoot": source_hooks_root(source),
            })
        })
        .collect();
    Some(Snapshot {
        workspace_identity: workspace_identity.to_string(),
        discovered_at: discovered_at.to_string(),
        source_files,
        entries,
        bundle_digest: bundle,
    })
}

impl HookEntry {
    /// 供保存路径复算 declare digest 的输入形态。
    pub fn declaration_input(&self, source_discovery_order: u64, default_timeout_ms: u64) -> DeclarationInput {
        DeclarationInput {
            source_relative_path: self.source_relative_path.clone(),
            source_discovery_order,
            event: self.event.clone(),
            matcher: self.matcher.clone(),
            matcher_index: self.matcher_index as u64,
            hook_index: self.hook_index as u64,
            hook: self.definition(),
            resolved_timeout_ms: default_timeout_ms,
            resolved_max_output_bytes: self.resolved_max_output_bytes,
        }
    }
}
