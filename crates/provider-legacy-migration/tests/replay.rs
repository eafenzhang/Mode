//! 与 TS `legacyModelProviderSerialized.ts` 的对拍：
//! `pnpm provider-legacy:fixtures` 生成的 `fixtures/legacy-providers.json`。

use std::path::Path;

use indexmap::IndexMap;
use provider_config::JsonValue;
use provider_legacy_migration::{
    create_model_provider_model_config, default_endpoint_path,
    default_supported_formats_from_api_format, default_supported_formats_from_endpoints,
    get_model_provider_model_ids, map_api_format_to_kind, migrate_legacy_model_provider,
    normalize_model_provider_configured_base_url, parse_legacy_model_provider_list_value,
    resolve_model_provider_api_format, resolve_model_provider_context_window,
    resolve_model_provider_default_kind, resolve_model_provider_kind_api_format,
    resolve_model_provider_runtime_base_url, strip_legacy_claude_provider_mappings,
    strip_model_provider_reasoning_patches, ApiFormat, CreateModelConfigParams, EndpointsInput,
    Modality, ProviderKind, ProviderShape,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct Fixture {
    cases: Vec<Case>,
    helpers: Value,
}

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    input: Value,
    valid: bool,
    migrated: Option<Value>,
}

fn ordered(value: &Value) -> JsonValue {
    JsonValue::from_json_str(&value.to_string()).expect("保序 JSON 转换")
}

fn parse_paths(value: Option<&Value>) -> Option<IndexMap<String, String>> {
    let object = value?.as_object()?;
    Some(
        object
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_string())))
            .collect(),
    )
}

fn parse_shape(input: &Value) -> ProviderShape {
    let endpoints = input.get("endpoints").and_then(Value::as_object);
    ProviderShape {
        api_format: input
            .get("apiFormat")
            .and_then(Value::as_str)
            .and_then(ApiFormat::parse),
        default_kind: input
            .get("defaultKind")
            .and_then(Value::as_str)
            .and_then(ProviderKind::parse),
        endpoints: EndpointsInput {
            base_url: endpoints
                .and_then(|endpoints| endpoints.get("baseURL"))
                .and_then(Value::as_str)
                .map(str::to_string),
            paths: parse_paths(endpoints.and_then(|endpoints| endpoints.get("paths"))),
        },
    }
}

fn parse_kind(value: Option<&Value>) -> Option<ProviderKind> {
    value.and_then(Value::as_str).and_then(ProviderKind::parse)
}

fn parse_modalities(value: Option<&Value>) -> Option<Vec<Modality>> {
    Some(
        value?
            .as_array()?
            .iter()
            .filter_map(|item| item.as_str().and_then(Modality::parse))
            .collect(),
    )
}

fn parse_create_params(params: &Value) -> CreateModelConfigParams {
    CreateModelConfigParams {
        id: params
            .get("id")
            .and_then(Value::as_str)
            .expect("id")
            .to_string(),
        name: params
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        kinds: params
            .get("kinds")
            .and_then(Value::as_array)
            .map(|kinds| {
                kinds
                    .iter()
                    .filter_map(|kind| kind.as_str().and_then(ProviderKind::parse))
                    .collect()
            })
            .unwrap_or_default(),
        default_kind: parse_kind(params.get("defaultKind")),
        context_window: params.get("contextWindow").and_then(Value::as_f64),
        max_output_tokens: params.get("maxOutputTokens").and_then(Value::as_f64),
        input_modalities: params
            .get("modalities")
            .and_then(|modalities| modalities.get("input"))
            .and_then(|value| parse_modalities(Some(value))),
        output_modalities: params
            .get("modalities")
            .and_then(|modalities| modalities.get("output"))
            .and_then(|value| parse_modalities(Some(value))),
        reasoning: params.get("reasoning").map(ordered),
        priority: params.get("priority").and_then(Value::as_f64),
        disabled_reason: params
            .get("disabledReason")
            .and_then(Value::as_str)
            .map(str::to_string),
        supports_tools: params.get("supportsTools").and_then(Value::as_bool),
        supports_structured_output: params
            .get("supportsStructuredOutput")
            .and_then(Value::as_bool),
        modified: params.get("modified").and_then(Value::as_bool),
        deleted: params.get("deleted").and_then(Value::as_bool),
    }
}

fn helper_array<'a>(helpers: &'a Value, key: &str) -> &'a Vec<Value> {
    helpers
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("helpers.{key} 缺失"))
}

fn assert_value(name: &str, index: usize, actual: Value, expected: &Value) {
    assert_eq!(actual, *expected, "{name}[{index}] 结果不一致");
}

fn check_helpers(helpers: &Value) {
    for (index, entry) in helper_array(helpers, "apiFormat").iter().enumerate() {
        let actual = resolve_model_provider_api_format(&parse_shape(&entry["input"]));
        assert_value(
            "apiFormat",
            index,
            json!(actual.as_str()),
            &entry["expected"],
        );
    }
    for (index, entry) in helper_array(helpers, "defaultKind").iter().enumerate() {
        let actual = resolve_model_provider_default_kind(&parse_shape(&entry["input"]));
        assert_value(
            "defaultKind",
            index,
            json!(actual.as_str()),
            &entry["expected"],
        );
    }
    for (index, entry) in helper_array(helpers, "runtimeBaseUrl").iter().enumerate() {
        let actual = resolve_model_provider_runtime_base_url(
            &parse_shape(&entry["input"]),
            parse_kind(entry.get("kind")),
        );
        assert_value("runtimeBaseUrl", index, json!(actual), &entry["expected"]);
    }
    for (index, entry) in helper_array(helpers, "configuredBaseUrl")
        .iter()
        .enumerate()
    {
        let actual =
            normalize_model_provider_configured_base_url(entry["input"].as_str().expect("input"));
        assert_value(
            "configuredBaseUrl",
            index,
            json!(actual),
            &entry["expected"],
        );
    }
    for (index, entry) in helper_array(helpers, "modelIds").iter().enumerate() {
        let models = ordered(&entry["models"]);
        let actual = get_model_provider_model_ids(models.as_array().expect("models"));
        assert_value("modelIds", index, json!(actual), &entry["expected"]);
    }
    for (index, entry) in helper_array(helpers, "createModelConfig")
        .iter()
        .enumerate()
    {
        let config = create_model_provider_model_config(&parse_create_params(&entry["params"]));
        assert_value(
            "createModelConfig",
            index,
            serde_json::to_value(config).expect("serialize"),
            &entry["expected"],
        );
    }
    for (index, entry) in helper_array(helpers, "stripReasoning").iter().enumerate() {
        let stripped = strip_model_provider_reasoning_patches(&ordered(&entry["reasoning"]));
        assert_value(
            "stripReasoning",
            index,
            serde_json::to_value(stripped).expect("serialize"),
            &entry["expected"],
        );
    }
    for (index, entry) in helper_array(helpers, "stripClaude").iter().enumerate() {
        let mappings = entry
            .get("mappings")
            .filter(|mappings| !mappings.is_null())
            .map(ordered);
        let stripped =
            strip_legacy_claude_provider_mappings(mappings.as_ref().and_then(JsonValue::as_object));
        assert_value(
            "stripClaude",
            index,
            serde_json::to_value(stripped).expect("serialize"),
            &entry["expected"],
        );
    }
    for (index, entry) in helper_array(helpers, "endpointPaths").iter().enumerate() {
        let kind = parse_kind(entry.get("kind")).expect("kind");
        assert_value(
            "endpointPaths",
            index,
            json!(default_endpoint_path(kind)),
            &entry["expected"],
        );
    }
    for (index, entry) in helper_array(helpers, "kindApiFormat").iter().enumerate() {
        let kind = parse_kind(entry.get("kind")).expect("kind");
        assert_value(
            "kindApiFormat",
            index,
            json!(resolve_model_provider_kind_api_format(kind).as_str()),
            &entry["expected"],
        );
    }
    for (index, entry) in helper_array(helpers, "defaultFormatsApiFormat")
        .iter()
        .enumerate()
    {
        let api_format = entry["apiFormat"]
            .as_str()
            .and_then(ApiFormat::parse)
            .expect("apiFormat");
        let formats: Vec<&str> = default_supported_formats_from_api_format(api_format)
            .into_iter()
            .map(|format| format.as_str())
            .collect();
        assert_value(
            "defaultFormatsApiFormat",
            index,
            json!(formats),
            &entry["expected"],
        );
    }
    for (index, entry) in helper_array(helpers, "defaultFormatsEndpoints")
        .iter()
        .enumerate()
    {
        let input = &entry["input"];
        let base_url = input.get("baseURL").and_then(Value::as_str);
        let paths = parse_paths(input.get("paths"));
        let formats: Vec<&str> = default_supported_formats_from_endpoints(base_url, paths.as_ref())
            .into_iter()
            .map(|format| format.as_str())
            .collect();
        assert_value(
            "defaultFormatsEndpoints",
            index,
            json!(formats),
            &entry["expected"],
        );
    }
    // 保持 helper 覆盖面：别名映射在任何 provider 形态上都应稳定。
    for kind in [
        ProviderKind::Anthropic,
        ProviderKind::OpenAi,
        ProviderKind::OpenAiCompatible,
    ] {
        assert_eq!(
            map_api_format_to_kind(resolve_model_provider_kind_api_format(kind)),
            kind
        );
    }
    assert_eq!(
        resolve_model_provider_context_window(None),
        provider_legacy_migration::MODEL_PROVIDER_NEW_MODEL_CONTEXT_WINDOW
    );
}

#[test]
fn matches_ts_legacy_provider_migration() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("legacy-providers.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
    let fixture: Fixture =
        serde_json::from_str(&raw).unwrap_or_else(|error| panic!("解析夹具失败: {error}"));
    assert!(!fixture.cases.is_empty());

    for case in &fixture.cases {
        let ordered_input = JsonValue::from_json_str(&case.input.to_string())
            .unwrap_or_else(|error| panic!("{}: 输入无法转成保序 JSON: {error}", case.name));
        let parsed = parse_legacy_model_provider_list_value(&JsonValue::Array(vec![ordered_input]));
        if !case.valid {
            assert!(
                parsed.is_err(),
                "{}: TS 判为非法，Rust 却解析成功",
                case.name
            );
            continue;
        }
        let providers = parsed.unwrap_or_else(|error| panic!("{}: 解析失败: {error}", case.name));
        let expected = case
            .migrated
            .as_ref()
            .unwrap_or_else(|| panic!("{}: 缺少 migrated", case.name));
        let actual = serde_json::to_value(migrate_legacy_model_provider(&providers[0]))
            .expect("序列化迁移结果");
        assert_eq!(actual, *expected, "{}: 迁移结果不一致", case.name);
    }

    check_helpers(&fixture.helpers);
}
