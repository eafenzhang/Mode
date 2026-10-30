//! 与 TS 真源的对拍。
//!
//! fixture 由 `pnpm provider-config:fixtures` 生成：每条记录保存输入 JSON 原文、
//! 成功时的 `toJSON()` 原文/解析后 JSON，或失败时的 zod/构造器问题。
//! 本测试重建输入，成功按原文逐字节比较，失败按接受/拒绝与路径集合比较。

use std::collections::BTreeSet;
use std::path::PathBuf;

use provider_config::{
    parse_account_provider_config_map, parse_json_text, parse_manual_model_config,
    parse_mode_builtin_model_config_rules, parse_mode_builtin_provider_config_rules,
    parse_model_config, parse_personal_model_config_rules, parse_personal_provider_config_map,
    parse_provider_config, parse_provider_config_map, parse_provider_template_map, JsonValue,
    ParseError, ParseResult,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Fixture {
    version: u32,
    records: Vec<Record>,
    validations: Vec<Validation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    name: String,
    kind: String,
    input: String,
    ok: bool,
    output: Option<String>,
    output_json: Option<Value>,
    issues: Option<Vec<IssueFixture>>,
    #[allow(dead_code)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Validation {
    name: String,
    kind: String,
    input: String,
    issues: Vec<IssueFixture>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct IssueFixture {
    code: String,
    path: Vec<String>,
    message: String,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/provider-config.json")
}

fn load_fixture() -> Fixture {
    let text = std::fs::read_to_string(fixture_path()).expect("读取 fixture");
    serde_json::from_str(&text).expect("解析 fixture")
}

fn parse_input(record: &Record) -> JsonValue {
    parse_json_text(&record.input).expect("fixture 输入必须是合法 JSON")
}

fn run_parse(kind: &str, input: &JsonValue) -> ParseResult<JsonValue> {
    match kind {
        "provider-config" => parse_provider_config(input, &[]).map(|value| value.to_json()),
        "provider-config-map" => parse_provider_config_map(input, &[]).map(|value| value.to_json()),
        "mode-builtin-provider" => {
            let (providers, templates) = parse_mode_builtin_provider_config_rules(input, &[])?;
            Ok(JsonValue::object_from([
                ("providers".to_owned(), providers.to_json()),
                ("providerTemplates".to_owned(), templates.to_json()),
            ]))
        }
        "personal-provider" => {
            parse_personal_provider_config_map(input, &[]).map(|value| value.to_json())
        }
        "provider-template-map" => {
            parse_provider_template_map(input, &[]).map(|value| value.to_json())
        }
        "account-provider" => {
            parse_account_provider_config_map(input, &[]).map(|value| value.to_json())
        }
        "model-config" => parse_model_config(input, &[]).map(|value| value.to_json()),
        "manual-model-config" => {
            parse_manual_model_config(input, &[]).map(|value| value.to_json())
        }
        "mode-builtin-model-rules" => {
            parse_mode_builtin_model_config_rules(input, &[]).map(|value| value.to_json())
        }
        "personal-model-rules" => {
            parse_personal_model_config_rules(input, &[]).map(|value| value.to_json())
        }
        other => panic!("未知 fixture kind: {other}"),
    }
}

fn error_path_set(error: &ParseError) -> (Vec<String>, bool) {
    let paths = error
        .issues()
        .iter()
        .map(|issue| issue.path_key())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    (paths, matches!(error, ParseError::Invariant(_)))
}

fn fixture_path_set(issues: &[IssueFixture]) -> Vec<String> {
    issues
        .iter()
        .map(|issue| issue.path.join("\u{1f}"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[test]
fn parity_with_typescript() {
    let fixture = load_fixture();
    assert_eq!(fixture.version, 1);
    assert!(!fixture.records.is_empty());

    for record in &fixture.records {
        let input = parse_input(record);
        let actual = run_parse(&record.kind, &input);
        match (record.ok, actual) {
            (true, Ok(value)) => {
                let expected_output = record.output.as_deref().expect("成功记录缺少 output");
                let actual_output = value.to_json_string();
                assert_eq!(
                    actual_output, expected_output,
                    "记录 {} 的序列化原文不一致",
                    record.name
                );
                let actual_json: Value =
                    serde_json::from_str(&actual_output).expect("Rust 输出必须是合法 JSON");
                let expected_json = record.output_json.as_ref().expect("成功记录缺少 outputJson");
                assert_eq!(
                    &actual_json, expected_json,
                    "记录 {} 的深层 JSON 不一致",
                    record.name
                );
            }
            (false, Err(error)) => {
                let expected_paths = fixture_path_set(record.issues.as_deref().unwrap_or(&[]));
                let (actual_paths, invariant) = error_path_set(&error);
                assert_eq!(
                    actual_paths, expected_paths,
                    "记录 {} 的失败路径集合不一致（invariant={invariant}, error={error:?}）",
                    record.name
                );
                if !expected_paths.is_empty() && !invariant {
                    let actual_codes: BTreeSet<_> =
                        error.issues().iter().map(|issue| issue.code.as_str()).collect();
                    assert!(
                        !actual_codes.is_empty(),
                        "记录 {} 有路径但缺少问题码",
                        record.name
                    );
                }
            }
            (true, Err(error)) => panic!(
                "记录 {} 在 TS 成功、Rust 失败: {error:?}",
                record.name
            ),
            (false, Ok(value)) => panic!(
                "记录 {} 在 TS 失败、Rust 成功: {}",
                record.name,
                value.to_json_string()
            ),
        }
    }
}

#[test]
fn validate_complete_parity() {
    let fixture = load_fixture();
    assert!(!fixture.validations.is_empty());

    for validation in &fixture.validations {
        let input = parse_json_text(&validation.input).expect("validation 输入必须是合法 JSON");
        let actual = run_validation(&validation.kind, &input);
        let expected = validation
            .issues
            .iter()
            .map(|issue| format!("{}@{}", issue.code, issue.path.join("\u{1f}")))
            .collect::<BTreeSet<_>>();
        let actual = actual
            .iter()
            .map(|issue| format!("{}@{}", issue.code, issue.path_key()))
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "校验记录 {} 不一致", validation.name);
    }
}

fn run_validation(kind: &str, input: &JsonValue) -> Vec<provider_config::Issue> {
    match kind {
        "validate-provider-config" => parse_provider_config(input, &[])
            .expect("validation 记录应可解析")
            .validate_complete(),
        "validate-provider-config-map" => parse_provider_config_map(input, &[])
            .expect("validation 记录应可解析")
            .validate_complete(),
        "validate-model-config" => parse_model_config(input, &[])
            .expect("validation 记录应可解析")
            .validate_complete(),
        "validate-provider-template-map" => parse_provider_template_map(input, &[])
            .expect("validation 记录应可解析")
            .validate_complete(),
        other => panic!("未知 validation kind: {other}"),
    }
}
