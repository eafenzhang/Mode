//! 与 TS `legacyPersonalProviderConfigImporter.ts` 的对拍：
//! `pnpm provider-legacy-import:fixtures` 生成的 `fixtures/legacy-personal-import.json`。

use std::path::Path;

use provider_config::JsonValue;
use provider_legacy_migration::import_legacy_personal_provider_config;

fn fixture() -> JsonValue {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("legacy-personal-import.json");
    let text = std::fs::read_to_string(&path).expect("读取 legacy-personal-import.json");
    JsonValue::from_json_str(&text).expect("解析 fixture")
}

#[test]
fn matches_typescript_personal_provider_import() {
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
        let providers = case
            .get("providers")
            .and_then(JsonValue::as_array)
            .expect("case.providers");
        let expected = case.get("expected").expect("case.expected");
        let actual = import_legacy_personal_provider_config(providers);
        assert_eq!(&actual, expected, "{name}: Personal 层更新不一致");
    }
}
