//! fixture 回放对拍（`docs/specs/git-cli.md`）。
//!
//! `pnpm git-cli:fixtures` 用 TS 真源执行纯解析器用例；本测试重放并逐项深比较。

use serde::Deserialize;
use serde_json::{json, Value};

use git_cli::{
    is_path_in_workspace_scope, normalize_git_path, normalize_workspace_in_repo_path,
    parse_git_branch_mutation_issues, parse_git_config_value, parse_numstat,
    parse_status_porcelain, to_diff_result, to_workspace_relative_git_path,
    GitCommandExecutionResult,
};

const FIXTURE: &str = include_str!("../fixtures/git-cli.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    name: String,
    ops: Vec<Op>,
    expected: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Op {
    kind: String,
    stdout: Option<String>,
    result: Option<GitCommandExecutionResult>,
    path: Option<String>,
    options: Option<DiffOptions>,
    #[serde(rename = "fn")]
    function: Option<String>,
    args: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffOptions {
    allowed_exit_codes: Option<Vec<i32>>,
    empty_summary: Option<String>,
    binary_summary: Option<String>,
}

fn run_op(op: &Op) -> Value {
    match op.kind.as_str() {
        "status" => {
            serde_json::to_value(parse_status_porcelain(op.stdout.as_deref().unwrap_or("")))
                .expect("status serializes")
        }
        "numstat" => serde_json::to_value(parse_numstat(op.stdout.as_deref().unwrap_or("")))
            .expect("numstat serializes"),
        "config" => {
            let result = op.result.clone().unwrap_or_default();
            match parse_git_config_value(&result) {
                Ok(value) => serde_json::to_value(value).expect("config serializes"),
                Err(message) => json!({ "error": message }),
            }
        }
        "issues" => {
            let result = op.result.clone().unwrap_or_default();
            serde_json::to_value(parse_git_branch_mutation_issues(&result))
                .expect("issues serialize")
        }
        "diff" => {
            let result = op.result.clone().unwrap_or_default();
            let options = op.options.as_ref();
            serde_json::to_value(to_diff_result(
                op.path.as_deref().unwrap_or(""),
                &result,
                options.and_then(|options| options.allowed_exit_codes.as_deref()),
                options.and_then(|options| options.empty_summary.as_deref()),
                options.and_then(|options| options.binary_summary.as_deref()),
            ))
            .expect("diff serializes")
        }
        "path" => {
            let args = op.args.clone().unwrap_or_default();
            let first = args.first().map(String::as_str).unwrap_or("");
            let second = args.get(1).map(String::as_str).unwrap_or("");
            match op.function.as_deref() {
                Some("normalize") => Value::String(normalize_git_path(first)),
                Some("workspace") => Value::String(normalize_workspace_in_repo_path(first)),
                Some("inScope") => Value::Bool(is_path_in_workspace_scope(first, second)),
                Some("toWorkspaceRelative") => {
                    Value::String(to_workspace_relative_git_path(first, second))
                }
                other => panic!("unknown path fn: {other:?}"),
            }
        }
        other => panic!("unknown op: {other}"),
    }
}

#[test]
fn replay_ts_fixtures() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
    let mut failures = Vec::new();
    for case in &fixture.cases {
        if case.expected.len() != case.ops.len() {
            failures.push(format!("[case {}] ops/expected 数量不一致", case.name));
            continue;
        }
        for (index, op) in case.ops.iter().enumerate() {
            let actual = run_op(op);
            let expected = &case.expected[index];
            if actual != *expected {
                failures.push(format!(
                    "[case {}] op[{index}] {} 结果不一致\n--- expected ---\n{}\n--- actual ---\n{}",
                    case.name,
                    op.kind,
                    serde_json::to_string_pretty(expected).unwrap_or_default(),
                    serde_json::to_string_pretty(&actual).unwrap_or_default(),
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "fixture 回放存在差异：\n{}",
        failures.join("\n\n")
    );
}
