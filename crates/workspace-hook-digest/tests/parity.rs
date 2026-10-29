//! 与 TS `createWorkspaceHookDeclarationDigest` 的黄金字节对拍。

use workspace_hook_digest::{declaration_digest, DeclarationInput, HookDefinition, Shell};

#[derive(serde::Deserialize)]
struct Fixture { cases: Vec<Case> }

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    hook: Hook,
    #[serde(rename = "resolvedTimeoutMs")]
    resolved_timeout_ms: u64,
    expected: String,
}

#[derive(serde::Deserialize)]
struct Hook {
    #[serde(rename = "type")]
    hook_type: String,
    command: String,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default, rename = "async")]
    is_async: Option<bool>,
    #[serde(default)]
    shell: Option<serde_json::Value>,
    #[serde(default, rename = "timeout")]
    timeout: Option<f64>,
}

fn to_definition(hook: &Hook) -> HookDefinition {
    HookDefinition {
        hook_type: hook.hook_type.clone(),
        command: hook.command.clone(),
        args: hook.args.clone(),
        is_async: hook.is_async,
        shell: match &hook.shell {
            None => Shell::Unset,
            Some(serde_json::Value::Bool(true)) => Shell::Enabled,
            Some(serde_json::Value::String(text)) => Shell::Command(text.clone()),
            Some(other) => panic!("unsupported shell {other:?}"),
        },
        timeout_ms: None,
        timeout: hook.timeout,
    }
}

#[test]
fn entry_digests_match_ts_goldens() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/entries.json")).expect("合法 fixture");
    assert!(!fixture.cases.is_empty());
    for case in &fixture.cases {
        let input = DeclarationInput {
            source_relative_path: ".mode/hooks.json".into(),
            source_discovery_order: 0,
            event: "PreToolUse".into(),
            matcher: None,
            matcher_index: 0,
            hook_index: 0,
            hook: to_definition(&case.hook),
            resolved_timeout_ms: case.resolved_timeout_ms,
            resolved_max_output_bytes: 1_048_576,
        };
        assert_eq!(
            declaration_digest(&input),
            case.expected,
            "{} 的摘要与 TS 不一致",
            case.name
        );
    }
}
