//! 与 TS `legacyModeConfigProviderReader.ts` 的对拍：
//! `pnpm provider-legacy-reader:fixtures` 生成的 `fixtures/legacy-config-reader.json`。

use std::path::Path;

use provider_config::JsonValue;
use provider_legacy_migration::{read_mode_config_providers, LegacyConfigReaderEnv};

fn fixture() -> JsonValue {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("legacy-config-reader.json");
    let text = std::fs::read_to_string(&path).expect("读取 legacy-config-reader.json");
    JsonValue::from_json_str(&text).expect("解析 fixture")
}

#[test]
fn matches_typescript_legacy_config_reader() {
    let fixture = fixture();
    let cases = fixture
        .as_object()
        .and_then(|fixture| fixture.get("cases"))
        .and_then(JsonValue::as_array)
        .expect("fixture.cases");
    assert!(!cases.is_empty(), "fixture 至少包含一条用例");
    for case in cases {
        let case = case.as_object().expect("case 必须是对象");
        let name = case
            .get("name")
            .and_then(JsonValue::as_str)
            .expect("case.name");
        let env = case
            .get("env")
            .and_then(JsonValue::as_object)
            .expect("case.env");
        let config = case.get("config").expect("case.config");
        let Some(expected) = case.get("providers").and_then(JsonValue::as_array) else {
            // schema 错误用例由 legacy_file_reader.rs 断言错误字段；这里只跑成功用例。
            continue;
        };
        let actual = read_mode_config_providers(
            config,
            &LegacyConfigReaderEnv {
                runtime_env: env
                    .get("runtime_env")
                    .and_then(JsonValue::as_str)
                    .expect("env.runtime_env"),
                bigmodel_api_origin: env
                    .get("bigmodel_api_origin")
                    .and_then(JsonValue::as_str)
                    .expect("env.bigmodel_api_origin"),
                now_ms: 1_700_000_000_000,
            },
        )
        .unwrap_or_else(|error| panic!("{name}: 读取失败: {error}"));
        assert_eq!(actual, expected, "{name}: Provider 列表不一致");
    }
}
