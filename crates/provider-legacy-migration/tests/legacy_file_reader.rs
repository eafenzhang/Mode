use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};

use provider_config::JsonValue;
use provider_legacy_migration::{
    read_legacy_mode_config_providers_with, LegacyConfigFileError, LegacyConfigReaderEnv,
};

fn fixture() -> JsonValue {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("legacy-config-reader.json");
    let text = std::fs::read_to_string(&path).expect("读取 legacy-config-reader.json");
    JsonValue::from_json_str(&text).expect("解析 fixture")
}

fn env() -> LegacyConfigReaderEnv<'static> {
    LegacyConfigReaderEnv {
        runtime_env: "production",
        bigmodel_api_origin: "https://bigmodel.cn",
        now_ms: 1_700_000_000_000,
    }
}

fn fake_read(bytes: Vec<u8>) -> impl FnOnce(&Path) -> std::io::Result<Vec<u8>> {
    move |_| Ok(bytes)
}

#[test]
fn reads_all_fixture_cases_with_io_boundary() {
    let fixture = fixture();
    let cases = fixture
        .as_object()
        .and_then(|fixture| fixture.get("cases"))
        .and_then(JsonValue::as_array)
        .expect("fixture.cases");
    for case in cases {
        let case = case.as_object().expect("case 必须是对象");
        let name = case
            .get("name")
            .and_then(JsonValue::as_str)
            .expect("case.name");
        let config = case.get("config").expect("case.config");
        let expected = case.get("providers").and_then(JsonValue::as_array);
        let expected_error = case.get("errorField").and_then(JsonValue::as_str);
        let has_provider = config
            .as_object()
            .map(|root| root.contains_key("provider"))
            .unwrap_or(false);
        let case_env = case
            .get("env")
            .and_then(JsonValue::as_object)
            .expect("case.env");
        let case_env = LegacyConfigReaderEnv {
            runtime_env: case_env
                .get("runtime_env")
                .and_then(JsonValue::as_str)
                .expect("env.runtime_env"),
            bigmodel_api_origin: case_env
                .get("bigmodel_api_origin")
                .and_then(JsonValue::as_str)
                .expect("env.bigmodel_api_origin"),
            now_ms: 1_700_000_000_000,
        };
        let result = read_legacy_mode_config_providers_with(
            Path::new("fixture/config.json"),
            &case_env,
            fake_read(config.to_json_string().into_bytes()),
        );
        if let Some(expected_error) = expected_error {
            match result {
                Err(LegacyConfigFileError::Schema { field, .. }) => {
                    assert_eq!(field, expected_error, "{name}: schema 错误字段不一致");
                }
                Err(other) => panic!("{name}: 期望 schema 错误 {expected_error}，实际 {other}"),
                Ok(_) => panic!("{name}: 期望 schema 错误 {expected_error}，实际成功"),
            }
            continue;
        }
        let actual = result.unwrap_or_else(|error| panic!("{name}: {error}"));
        let expected = expected.expect("case.providers");
        if has_provider {
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "{name}: Provider 列表不一致"
            );
        } else {
            assert_eq!(actual, None, "{name}: 没有 provider 应返回 None");
        }
    }
}

#[test]
fn missing_file_returns_none() {
    let result = read_legacy_mode_config_providers_with(Path::new("missing.json"), &env(), |_| {
        Err(Error::from(ErrorKind::NotFound))
    })
    .expect("missing");
    assert_eq!(result, None);
}

#[test]
fn io_error_is_staged_and_includes_code() {
    let error =
        read_legacy_mode_config_providers_with(&PathBuf::from("denied.json"), &env(), |_| {
            Err(Error::from(ErrorKind::PermissionDenied))
        })
        .unwrap_err();
    assert!(matches!(error, LegacyConfigFileError::Io { .. }));
    assert_eq!(
        error.to_string(),
        "旧 Provider 配置读取失败 stage=io path=denied.json code=EACCES"
    );
}

#[test]
fn json_error_does_not_leak_input_values() {
    let error = read_legacy_mode_config_providers_with(
        Path::new("config.json"),
        &env(),
        fake_read(br#"{"apiKey":"SECRET","#.to_vec()),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "旧 Provider 配置解析失败 stage=json path=config.json"
    );
    assert!(!error.to_string().contains("SECRET"));
}

#[test]
fn schema_errors_report_field_paths() {
    for (text, expected_field) in [
        ("[]", ""),
        (r#"{"$schema":1}"#, "$schema"),
        (r#"{"provider":[]}"#, "provider"),
        (r#"{"provider":{"x":"no"}}"#, "provider.x"),
        (
            r#"{"provider":{"x":{"enabled":"yes"}}}"#,
            "provider.x.enabled",
        ),
        (
            r#"{"provider":{"x":{"headers":{"a":1}}}}"#,
            "provider.x.headers.a",
        ),
        (
            r#"{"provider":{"x":{"models":{"m":"no"}}}}"#,
            "provider.x.models.m",
        ),
        (
            r#"{"provider":{"x":{"mode":{"deletedModels":[""]}}}}"#,
            "provider.x.mode.deletedModels.0",
        ),
    ] {
        let error = read_legacy_mode_config_providers_with(
            Path::new("config.json"),
            &env(),
            fake_read(text.as_bytes().to_vec()),
        )
        .unwrap_err();
        assert!(
            matches!(error, LegacyConfigFileError::Schema { .. }),
            "{text}: 应为 schema 错误"
        );
        assert_eq!(
            error.to_string(),
            format!(
                "旧 Provider 配置校验失败 stage=schema path=config.json field={expected_field}"
            )
        );
    }
}

#[test]
fn missing_provider_key_returns_none() {
    let result = read_legacy_mode_config_providers_with(
        Path::new("config.json"),
        &env(),
        fake_read(br#"{"$schema":"https://example.invalid"}"#.to_vec()),
    )
    .expect("valid");
    assert_eq!(result, None);
}

#[test]
fn empty_provider_map_returns_empty_list() {
    let result = read_legacy_mode_config_providers_with(
        Path::new("config.json"),
        &env(),
        fake_read(br#"{"provider":{}}"#.to_vec()),
    )
    .expect("valid");
    assert_eq!(result, Some(Vec::new()));
}
