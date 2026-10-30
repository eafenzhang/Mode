//! provider-config 的单元/边界测试。

use provider_config::{
    parse_account_provider_config_map, parse_json_text, parse_manual_model_config,
    parse_mode_builtin_model_config_rules, parse_mode_builtin_provider_config_rules,
    parse_model_config, parse_personal_model_config_rules, parse_personal_provider_config_map,
    parse_provider_config, parse_provider_config_map, parse_provider_template_map, Field, JsonValue,
    ParseError,
};

fn input(text: &str) -> JsonValue {
    parse_json_text(text).expect("测试输入必须是合法 JSON")
}

fn issue_paths(error: &ParseError) -> Vec<String> {
    error.issues().iter().map(|issue| issue.path_key()).collect()
}

#[test]
fn provider_serialization_keeps_declared_field_and_map_order() {
    let config = parse_provider_config(
        &input(
            r#"{
              "visibility":"visible",
              "modelOrder":["b","a"],
              "builtinModelIds":["a"],
              "api":{"headers":{"Z":"1","A":"2"},"baseUrl":"https://api.example.com","type":"openai-chat-completions"},
              "access":{"apiKeyManagementUrl":"https://keys.example.com","type":"api-key","apiKey":"k"},
              "logo":{"key":"zhipu","type":"builtin"},
              "group":"zai-family"
            }"#,
        ),
        &[],
    )
    .expect("合法配置");
    assert_eq!(
        config.to_json().to_json_string(),
        r#"{"group":"zai-family","logo":{"type":"builtin","key":"zhipu"},"access":{"type":"api-key","apiKey":"k","apiKeyManagementUrl":"https://keys.example.com"},"api":{"type":"openai-chat-completions","baseUrl":"https://api.example.com","headers":{"Z":"1","A":"2"}},"builtinModelIds":["a"],"modelOrder":["b","a"],"visibility":"visible"}"#
    );
}

#[test]
fn unknown_key_is_rejected_at_object_path() {
    let error = parse_provider_config(&input(r#"{"surprise":1}"#), &[]).unwrap_err();
    assert_eq!(issue_paths(&error), vec![String::new()]);
    assert_eq!(error.issues()[0].code, "unrecognized_keys");
}

#[test]
fn optional_null_and_absent_are_distinct() {
    let absent = parse_provider_config(&input("{}"), &[]).expect("空配置");
    assert_eq!(absent.to_json().to_json_string(), "{}");
    let nulls = parse_provider_config(&input(r#"{"group":null,"api":null}"#), &[]).expect("null");
    assert_eq!(nulls.to_json().to_json_string(), r#"{"group":null,"api":null}"#);
    assert!(matches!(absent.group, Field::Absent));
    assert!(matches!(nulls.group, Field::Null));
}

#[test]
fn model_ids_are_trimmed_and_blank_is_invariant() {
    let config =
        parse_provider_config(&input(r#"{"builtinModelIds":["  m1  ","m2"]}"#), &[]).unwrap();
    assert_eq!(config.to_json().to_json_string(), r#"{"builtinModelIds":["m1","m2"]}"#);

    let error = parse_provider_config(&input(r#"{"builtinModelIds":["   "]}"#), &[]).unwrap_err();
    assert!(matches!(error, ParseError::Invariant(_)));
    assert!(error.issues().is_empty());
}

#[test]
fn duplicate_provider_and_template_keys_fail_closed() {
    let error = parse_provider_config_map(
        &input(r#"[{"providerId":"p","config":{}},{"providerId":"p","config":{}}]"#),
        &[],
    )
    .unwrap_err();
    assert!(matches!(error, ParseError::Invariant(_)));

    let error = parse_provider_template_map(
        &input(r#"[{"templateId":"t","templateNameMap":{},"config":{}},{"templateId":"t","templateNameMap":{},"config":{}}]"#),
        &[],
    )
    .unwrap_err();
    assert!(matches!(error, ParseError::Invariant(_)));
}

#[test]
fn number_boundaries_follow_safe_positive_integer() {
    for (text, expected_ok) in [
        (r#"{"properties":{"contextWindow":1}}"#, true),
        (r#"{"properties":{"contextWindow":1.0}}"#, true),
        (r#"{"properties":{"contextWindow":0}}"#, false),
        (r#"{"properties":{"contextWindow":-1}}"#, false),
        (r#"{"properties":{"contextWindow":1.5}}"#, false),
        (r#"{"properties":{"contextWindow":9007199254740992}}"#, false),
    ] {
        assert_eq!(
            parse_model_config(&input(text), &[]).is_ok(),
            expected_ok,
            "{text}"
        );
    }
}

#[test]
fn option_map_requires_compilable_object_expression() {
    for source in [
        r#"{"a": reasoningLevel}"#,
        r#"reasoningLevel == "high" ? {"a": 1} : {"a": 2}"#,
        r#"{'nested': {'x': 1}}"#,
    ] {
        let text = format!(
            r#"{{"optionSpecs":{{"reasoningLevel":{{"values":["low"],"map":{}}}}}}}"#,
            serde_json::to_string(source).expect("转义")
        );
        assert!(parse_model_config(&input(&text), &[]).is_ok(), "{source}");
    }
    for source in ["unknownIdentifier", "1 + 2", "{\"a\": }", "\"text\"", ""] {
        let text = format!(
            r#"{{"optionSpecs":{{"reasoningLevel":{{"values":["low"],"map":{}}}}}}}"#,
            serde_json::to_string(source).expect("转义")
        );
        let result = parse_model_config(&input(&text), &[]);
        assert!(result.is_err(), "{source}");
        assert_eq!(issue_paths(&result.unwrap_err()), vec!["optionSpecs\u{1f}reasoningLevel\u{1f}map"]);
    }
}

#[test]
fn regex_patterns_are_validated_like_js_regexp() {
    let rule = |pattern: &str| {
        format!(
            r#"{{"modelRules":[{{"modelMatch":{},"config":{{}}}}],"modelApiRules":[],"providerSiteRules":[],"templateModelRules":[],"builtinProviderModelRules":[]}}"#,
            serde_json::to_string(pattern).expect("转义")
        )
    };
    for pattern in [".*", "^gpt-", "openai-.*", "(?:a|b)", "[a-z]+", "(?<name>a)"] {
        assert!(
            parse_mode_builtin_model_config_rules(&input(&rule(pattern)), &[]).is_ok(),
            "{pattern}"
        );
    }
    for pattern in ["(", "[", "\\", "*abc", "a)b", "a**", "a{2,1}"] {
        assert!(
            parse_mode_builtin_model_config_rules(&input(&rule(pattern)), &[]).is_err(),
            "{pattern}"
        );
    }
}

#[test]
fn urls_follow_common_js_url_acceptance() {
    for url in ["https://api.example.com", "http://a", "a:b", "mailto:x@y", "x:"] {
        let text = format!(r#"{{"api":{{"baseUrl":{}}}}}"#, serde_json::to_string(url).unwrap());
        assert!(parse_provider_config(&input(&text), &[]).is_ok(), "{url}");
    }
    for url in ["not a url", "http://", "//x", "1:2", "http://a b", "http://example.com:99999"] {
        let text = format!(r#"{{"api":{{"baseUrl":{}}}}}"#, serde_json::to_string(url).unwrap());
        assert!(parse_provider_config(&input(&text), &[]).is_err(), "{url}");
    }
}

#[test]
fn personal_provider_rejects_account_access_and_builtin_model_ids() {
    let error = parse_personal_provider_config_map(
        &input(r#"{"providerRules":[{"providerId":"account:x","config":{"access":null}}]}"#),
        &[],
    )
    .unwrap_err();
    assert_eq!(issue_paths(&error), vec!["providerRules\u{1f}0\u{1f}config\u{1f}access"]);

    let error = parse_personal_provider_config_map(
        &input(r#"{"providerRules":[{"providerId":"p","config":{"builtinModelIds":["m"]}}]}"#),
        &[],
    )
    .unwrap_err();
    assert_eq!(issue_paths(&error), vec!["providerRules\u{1f}0\u{1f}config"]);
}

#[test]
fn template_names_and_config_are_strict() {
    let error = parse_provider_template_map(
        &input(r#"[{"templateId":"t","templateNameMap":{"ja-JP":"x"},"config":{}}]"#),
        &[],
    )
    .unwrap_err();
    assert_eq!(issue_paths(&error), vec!["0\u{1f}templateNameMap"]);

    let error = parse_provider_template_map(
        &input(r#"[{"templateId":"t","templateNameMap":{},"config":{"visibility":"visible"}}]"#),
        &[],
    )
    .unwrap_err();
    assert_eq!(issue_paths(&error), vec!["0\u{1f}config"]);
}

#[test]
fn manual_config_requires_properties_and_option_specs() {
    let error = parse_manual_model_config(&input(r#"{"enabled":true}"#), &[]).unwrap_err();
    assert_eq!(
        issue_paths(&error),
        vec!["properties".to_owned(), "optionSpecs".to_owned()]
    );

    let error = parse_manual_model_config(
        &input(
            r#"{"properties":{"contextWindow":1,"supportsJsonSchemaOutput":true,"supportsNativeWebSearch":true,"supportsMidConversationSystem":true,"inputFormat":{"supportsImage":true,"supportsVideo":true,"supportsPdf":true}},"optionSpecs":{"reasoningLevel":{"values":["low"],"map":"{\"a\": reasoningLevel}"},"maxOutputTokens":{"max":1,"map":"{\"a\": 1}"}}}"#,
        ),
        &[],
    )
    .unwrap_err();
    assert_eq!(issue_paths(&error), vec!["optionSpecs\u{1f}maxOutputTokens"]);
}

#[test]
fn manual_serialization_uses_schema_order() {
    let manual = parse_manual_model_config(
        &input(
            r#"{"enabled":true,"properties":{"inputFormat":{"supportsPdf":true,"supportsVideo":false,"supportsImage":true},"supportsMidConversationSystem":true,"supportsNativeWebSearch":false,"supportsJsonSchemaOutput":true,"contextWindow":64000},"optionSpecs":{"maxOutputTokens":{"max":8192},"reasoningLevel":{"map":"{\"effort\": reasoningLevel}","values":["low","high"]}}}"#,
        ),
        &[],
    )
    .unwrap();
    assert_eq!(
        manual.to_json().to_json_string(),
        r#"{"enabled":true,"properties":{"contextWindow":64000,"supportsJsonSchemaOutput":true,"supportsNativeWebSearch":false,"supportsMidConversationSystem":true,"inputFormat":{"supportsImage":true,"supportsVideo":false,"supportsPdf":true}},"optionSpecs":{"reasoningLevel":{"values":["low","high"],"map":"{\"effort\": reasoningLevel}"},"maxOutputTokens":{"max":8192}}}"#
    );
}

#[test]
fn model_rules_serialization_matches_tojson_order() {
    let rules = parse_personal_model_config_rules(
        &input(
            r#"{"providerModelRules":[{"providerId":"p","modelId":"m","config":{}}],"manualProviderModelRules":[{"providerId":"p","modelId":"m2","config":{"properties":{"contextWindow":1,"supportsJsonSchemaOutput":true,"supportsNativeWebSearch":true,"supportsMidConversationSystem":true,"inputFormat":{"supportsImage":true,"supportsVideo":true,"supportsPdf":true}},"optionSpecs":{"reasoningLevel":{"values":["low"],"map":"{\"a\": reasoningLevel}"},"maxOutputTokens":{"max":1}}}}]}"#,
        ),
        &[],
    )
    .unwrap();
    assert_eq!(
        rules.to_json().to_json_string(),
        r#"{"modelRules":[],"modelApiRules":[],"providerSiteRules":[],"templateModelRules":[],"builtinProviderModelRules":[{"modelId":"m","config":{},"providerId":"p"}],"manualProviderModelRules":[{"modelId":"m2","providerId":"p","config":{"properties":{"contextWindow":1,"inputFormat":{"supportsImage":true,"supportsVideo":true,"supportsPdf":true},"supportsJsonSchemaOutput":true,"supportsNativeWebSearch":true,"supportsMidConversationSystem":true},"optionSpecs":{"reasoningLevel":{"values":["low"],"map":"{\"a\": reasoningLevel}"},"maxOutputTokens":{"max":1}}}}]}"#
    );
}

#[test]
fn account_access_only_allows_type_and_entitled() {
    let map = parse_account_provider_config_map(
        &input(r#"{"account:x":{"builtinModelIds":["m"],"access":{"type":"zhipu-account","entitled":false}}}"#),
        &[],
    )
    .unwrap();
    assert_eq!(
        map.to_json().to_json_string(),
        r#"[{"providerId":"account:x","config":{"access":{"type":"zhipu-account","entitled":false},"builtinModelIds":["m"]}}]"#
    );
    let error = parse_account_provider_config_map(
        &input(r#"{"account:x":{"access":{"type":"zhipu-account","accountType":"zai"}}}"#),
        &[],
    )
    .unwrap_err();
    assert_eq!(issue_paths(&error), vec!["account:x\u{1f}access"]);
}

#[test]
fn validate_complete_reports_missing_required_fields() {
    let config = parse_provider_config(&input("{}"), &[]).unwrap();
    let issues = config.validate_complete();
    let paths = issues
        .iter()
        .map(|issue| issue.path_key())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["group", "access", "api"]);
    assert!(issues.iter().all(|issue| issue.code == "required-field-missing"));

    let map = parse_provider_config_map(
        &input(r#"[{"providerId":"p1","config":{"group":"zai-family","access":{"type":"api-key","apiKey":"k"},"api":{"type":"openai-responses","baseUrl":"https://x"}}}]"#),
        &[],
    )
    .unwrap();
    assert!(map.validate_complete().is_empty());
}

#[test]
fn builtin_provider_rules_return_both_maps_in_order() {
    let (providers, templates) = parse_mode_builtin_provider_config_rules(
        &input(
            r#"{"templateRules":[{"templateId":"t","templateNameMap":{"en-US":"T","zh-CN":"模"},"config":{}}],"providerRules":[{"providerId":"p","config":{"group":"zai-family"}}]}"#,
        ),
        &[],
    )
    .unwrap();
    assert_eq!(providers.keys(), vec!["p".to_owned()]);
    assert_eq!(templates.keys(), vec!["t".to_owned()]);
    assert_eq!(
        templates.get("t").unwrap().to_json().to_json_string(),
        r#"{"templateId":"t","templateNameMap":{"zh-CN":"模","en-US":"T"},"config":{}}"#
    );
}
