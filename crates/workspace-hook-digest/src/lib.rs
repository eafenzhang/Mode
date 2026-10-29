//! Workspace Hook 摘要（条目级与包级）。
//!
//! 真源：`packages/shared/src/workspace-hook-digest.ts`。摘要不是对象稳定序列化，而是
//! **构造有序数组后 `sha256(JSON.stringify(array))`**；`canonicalOptional` 把可选值包成
//! `["unset"]` / `["set", value]`，因此 `undefined` 与 `false` 可区分——这是最容易做错的
//! 一处，必须逐字节一致，否则同一份配置会算出两个 digest、trust store 永远判「未信任」。
//!
//! 与 TS 的已知差异：`path.resolve/relative` 用词法规范化实现（不访问文件系统），
//! 对已存在的路径两者结果一致；纯词法输入（如 `a/../b`）也按词法归约。

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

pub const DIGEST_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shell {
    Unset,
    Enabled,
    Command(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookDefinition {
    /// `"command"` 或 `"process"`。
    pub hook_type: String,
    pub command: String,
    pub args: Option<Vec<String>>,
    pub is_async: Option<bool>,
    pub shell: Shell,
    pub timeout_ms: Option<f64>,
    pub timeout: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeclarationInput {
    pub source_relative_path: String,
    pub source_discovery_order: u64,
    pub event: String,
    pub matcher: Option<String>,
    pub matcher_index: u64,
    pub hook_index: u64,
    pub hook: HookDefinition,
    pub resolved_timeout_ms: u64,
    pub resolved_max_output_bytes: u64,
}

pub fn canonical_optional(value: Option<&Value>) -> Value {
    match value {
        None => json!(["unset"]),
        Some(value) => json!(["set", value]),
    }
}

pub fn execution_payload(hook: &HookDefinition) -> Value {
    if hook.hook_type == "process" {
        return json!([
            "process",
            hook.command,
            hook.args.clone().unwrap_or_default()
        ]);
    }
    let shell = match &hook.shell {
        Shell::Unset => json!(["unset"]),
        Shell::Enabled => json!(["true"]),
        Shell::Command(command) => json!(["string", command]),
    };
    json!(["command", hook.command, hook.is_async == Some(true), shell])
}

pub fn canonical_declaration_payload(input: &DeclarationInput) -> Value {
    json!([
        "workspace-hook-declaration",
        DIGEST_SCHEMA_VERSION,
        input.source_relative_path,
        input.source_discovery_order,
        input.event,
        input.matcher,
        input.matcher_index,
        input.hook_index,
        execution_payload(&input.hook),
        input.resolved_timeout_ms,
        input.resolved_max_output_bytes,
    ])
}

pub fn declaration_digest(input: &DeclarationInput) -> String {
    sha256(&canonical_declaration_payload(input))
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceInput {
    pub cache_key: String,
    pub canonical_path: String,
    pub discovery_order: u64,
    pub config_file_kind: String,
    pub explicit_project_config: bool,
    /// 摘要里参与的三项，缺省与显式设值必须可区分。
    pub enabled: Option<Value>,
    pub timeout_ms: Option<Value>,
    pub max_output_bytes: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestHook {
    pub hook_declaration_digest: String,
    pub source_root_enabled: bool,
    pub declaration_enabled: bool,
    pub runtime_hooks_enabled: bool,
    pub configured_enabled: bool,
}

/// `normalizeRelativeSourcePath`：`relative(resolve(ws), resolve(source))`，
/// 反斜杠换 `/`，空串回落 basename。
pub fn normalize_relative_source_path(workspace_path: &str, source_path: &str) -> String {
    let base = resolve_lexical(Path::new(workspace_path));
    let target = resolve_lexical(Path::new(source_path));
    let mut value = relative_lexical(&base, &target);
    value = value.replace('\\', "/");
    if value.is_empty() {
        Path::new(source_path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        value
    }
}

/// 包级摘要：source 列表按发现顺序 + 条目摘要与四个门禁位。
pub fn bundle_digest(workspace_path: &str, sources: &[SourceInput], hooks: &[DigestHook]) -> String {
    let source_payload: Vec<Value> = sources
        .iter()
        .map(|source| {
            json!([
                normalize_relative_source_path(workspace_path, &source.canonical_path),
                source.discovery_order,
                source.config_file_kind,
                source.explicit_project_config,
                canonical_optional(source.enabled.as_ref()),
                canonical_optional(source.timeout_ms.as_ref()),
                canonical_optional(source.max_output_bytes.as_ref()),
            ])
        })
        .collect();
    let hook_payload: Vec<Value> = hooks
        .iter()
        .map(|hook| {
            json!([
                hook.hook_declaration_digest,
                hook.source_root_enabled,
                hook.declaration_enabled,
                hook.runtime_hooks_enabled,
                hook.configured_enabled,
            ])
        })
        .collect();
    sha256(&json!([
        "workspace-hook-bundle",
        DIGEST_SCHEMA_VERSION,
        source_payload,
        hook_payload,
    ]))
}

/// `sha256(JSON.stringify(value))`：紧凑 JSON 的 UTF-8 字节。
pub fn sha256(value: &Value) -> String {
    use sha2::{Digest, Sha256};
    let text = serde_json::to_string(value).unwrap_or_default();
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn resolve_lexical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn relative_lexical(base: &Path, target: &Path) -> String {
    let base: Vec<Component> = base.components().collect();
    let target: Vec<Component> = target.components().collect();
    let mut index = 0;
    while index < base.len() && index < target.len() && base[index] == target[index] {
        index += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in index..base.len() {
        parts.push("..".to_string());
    }
    for component in &target[index..] {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_hook() -> HookDefinition {
        HookDefinition {
            hook_type: "command".into(),
            command: "npm test".into(),
            args: None,
            is_async: None,
            shell: Shell::Unset,
            timeout_ms: None,
            timeout: None,
        }
    }

    #[test]
    fn canonical_optional_distinguishes_unset_from_false() {
        assert_eq!(canonical_optional(None), json!(["unset"]));
        assert_eq!(canonical_optional(Some(&json!(false))), json!(["set", false]));
    }

    #[test]
    fn execution_variants_follow_the_ts_shapes() {
        assert_eq!(
            execution_payload(&command_hook()),
            json!(["command", "npm test", false, ["unset"]])
        );
        let mut process = command_hook();
        process.hook_type = "process".into();
        process.args = Some(vec!["--flag".into()]);
        assert_eq!(
            execution_payload(&process),
            json!(["process", "npm test", ["--flag"]])
        );
        let mut shell = command_hook();
        shell.shell = Shell::Command("/bin/bash".into());
        assert_eq!(
            execution_payload(&shell),
            json!(["command", "npm test", false, ["string", "/bin/bash"]])
        );
    }

    #[test]
    fn digests_are_hex_and_change_with_inputs() {
        let input = DeclarationInput {
            source_relative_path: ".mode/hooks.json".into(),
            source_discovery_order: 0,
            event: "PreToolUse".into(),
            matcher: None,
            matcher_index: 0,
            hook_index: 0,
            hook: command_hook(),
            resolved_timeout_ms: 60_000,
            resolved_max_output_bytes: 1_048_576,
        };
        let first = declaration_digest(&input);
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));

        let mut changed = input.clone();
        changed.hook.is_async = Some(true);
        assert_ne!(declaration_digest(&changed), first, "async 参与摘要");

        let bundle = bundle_digest(
            "/w",
            &[SourceInput {
                cache_key: "k".into(),
                canonical_path: "/w/.mode/hooks.json".into(),
                discovery_order: 0,
                config_file_kind: "mode".into(),
                explicit_project_config: true,
                enabled: Some(json!(false)),
                timeout_ms: None,
                max_output_bytes: Some(json!(1024)),
            }],
            &[DigestHook {
                hook_declaration_digest: first.clone(),
                source_root_enabled: true,
                declaration_enabled: true,
                runtime_hooks_enabled: true,
                configured_enabled: true,
            }],
        );
        assert_eq!(bundle.len(), 64);
        assert_ne!(bundle, first);
    }

    #[test]
    fn relative_paths_use_forward_slashes_and_basename_fallback() {
        assert_eq!(
            normalize_relative_source_path("/w", "/w/.mode/hooks.json"),
            ".mode/hooks.json"
        );
        assert_eq!(
            normalize_relative_source_path("/w", "/other/hooks.json"),
            "../other/hooks.json"
        );
    }
}
