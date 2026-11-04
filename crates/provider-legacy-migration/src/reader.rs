//! 旧 `config.json` Provider 读取与 v2 归一化（保序）。
//!
//! 真源：`packages/services/src/model-provider/legacyModeConfigProviderReader.ts`。
//! 输入是已解析的 config JSON；文件读取/日志由调用方负责。

use indexmap::IndexMap;
use provider_config::JsonValue;
use serde_json::Number;

use crate::{
    create_model_provider_model_config, default_endpoint_path,
    default_supported_formats_from_api_format, default_supported_formats_from_endpoints,
    get_model_provider_model_ids, migrate_legacy_model_provider,
    normalize_model_provider_base_url_for_kind, normalize_model_provider_configured_base_url,
    parse_legacy_model_provider_list_value, resolve_model_provider_api_format,
    resolve_model_provider_default_kind, resolve_model_provider_kind_api_format,
    resolve_model_provider_runtime_base_url, strip_model_provider_reasoning_patches, ApiFormat,
    CreateModelConfigParams, EndpointsInput, Modality, ProviderKind, ProviderShape,
    SupportedFormat,
};

const BIGMODEL_CODING_PLAN_ANTHROPIC_BASE_URL: &str = "https://open.bigmodel.cn/api/anthropic";
const BIGMODEL_INDIVIDUAL_CODING_PLAN: &str = "account:bigmodel-individual-coding-plan";
const RETIRED_ZAPI_PROVIDER_ID: &str = "builtin:zapi";
const LEGACY_FALLBACK_CONTEXT_WINDOW: i64 = 200_000;

const PRESET_GLM_PROVIDER_IDS: [&str; 6] = [
    "zai-api",
    "account:zai-individual-coding-plan",
    "account:zai-start-plan",
    "bigmodel-api",
    BIGMODEL_INDIVIDUAL_CODING_PLAN,
    "account:bigmodel-start-plan",
];

pub struct LegacyConfigReaderEnv<'a> {
    pub runtime_env: &'a str,
    pub bigmodel_api_origin: &'a str,
    pub now_ms: i64,
}

fn object(value: &JsonValue) -> Option<&IndexMap<String, JsonValue>> {
    value.as_object()
}

fn string_value(value: Option<&JsonValue>) -> Option<String> {
    value?.as_str().map(str::trim).map(str::to_string)
}

fn non_empty(value: Option<&JsonValue>) -> Option<String> {
    string_value(value).filter(|value| !value.is_empty())
}

fn number_value(value: Option<&JsonValue>) -> Option<f64> {
    value?.as_number()?.as_f64()
}

fn positive(value: Option<&JsonValue>) -> Option<f64> {
    number_value(value).filter(|value| value.is_finite() && *value > 0.0)
}

fn finite(value: Option<&JsonValue>) -> Option<f64> {
    number_value(value).filter(|value| value.is_finite())
}

fn bool_value(value: Option<&JsonValue>) -> Option<bool> {
    value?.as_bool()
}

fn number(value: f64) -> JsonValue {
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        JsonValue::Number(Number::from(value as i64))
    } else {
        Number::from_f64(value)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null)
    }
}

fn string(value: &str) -> JsonValue {
    JsonValue::String(value.to_string())
}

fn object_from(entries: Vec<(String, JsonValue)>) -> JsonValue {
    JsonValue::object_from(entries)
}

fn push(entries: &mut Vec<(String, JsonValue)>, key: &str, value: JsonValue) {
    entries.push((key.to_string(), value));
}

fn read_string_record(value: Option<&JsonValue>) -> Option<IndexMap<String, JsonValue>> {
    let object = object(value?)?;
    let mut result = IndexMap::new();
    for (key, item) in object {
        let normalized_key = key.trim();
        let Some(normalized_value) = non_empty(Some(item)) else {
            continue;
        };
        if !normalized_key.is_empty() {
            result.insert(normalized_key.to_string(), string(&normalized_value));
        }
    }
    (!result.is_empty()).then_some(result)
}

fn parse_kind(value: Option<&JsonValue>) -> Option<ProviderKind> {
    value?.as_str().and_then(ProviderKind::parse)
}

fn parse_api_format(value: Option<&JsonValue>) -> Option<ApiFormat> {
    value?.as_str().and_then(ApiFormat::parse)
}

fn parse_modalities(value: Option<&JsonValue>) -> Option<Vec<Modality>> {
    let values = value?.as_array()?;
    let mut result = Vec::new();
    for item in values {
        let Some(modality) = item.as_str().and_then(Modality::parse) else {
            continue;
        };
        if !result.contains(&modality) {
            result.push(modality);
        }
    }
    Some(result)
}

pub(crate) fn provider_shape(provider: &IndexMap<String, JsonValue>) -> ProviderShape {
    let endpoints = provider.get("endpoints").and_then(object);
    ProviderShape {
        api_format: parse_api_format(provider.get("apiFormat")),
        default_kind: parse_kind(provider.get("defaultKind")),
        endpoints: EndpointsInput {
            base_url: string_value(endpoints.and_then(|endpoints| endpoints.get("baseURL"))),
            paths: endpoints
                .and_then(|endpoints| endpoints.get("paths"))
                .and_then(object)
                .map(|paths| {
                    paths
                        .iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.to_string()))
                        })
                        .collect::<IndexMap<_, _>>()
                }),
        },
    }
}

fn infer_kind_from_open_code(provider: &IndexMap<String, JsonValue>) -> ProviderKind {
    if let Some(kind) = parse_kind(provider.get("kind")) {
        return kind;
    }
    let npm = provider
        .get("npm")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_lowercase();
    if npm.contains("anthropic") {
        ProviderKind::Anthropic
    } else if npm.contains("openai-compatible") {
        ProviderKind::OpenAiCompatible
    } else if npm.contains("openai") {
        ProviderKind::OpenAi
    } else {
        ProviderKind::OpenAiCompatible
    }
}

fn has_open_code_runtime_fields(provider: &IndexMap<String, JsonValue>) -> bool {
    let options = provider.get("options").and_then(object);
    provider.contains_key("kind")
        || non_empty(options.and_then(|options| options.get("baseURL"))).is_some()
        || non_empty(provider.get("api")).is_some()
        || non_empty(provider.get("npm")).is_some()
}

fn resolve_open_code_default_kind(provider: &IndexMap<String, JsonValue>) -> ProviderKind {
    if let Some(kind) = parse_kind(provider.get("kind")) {
        return kind;
    }
    let options = provider.get("options").and_then(object);
    if non_empty(options.and_then(|options| options.get("baseURL"))).is_some()
        || non_empty(provider.get("api")).is_some()
        || non_empty(provider.get("npm")).is_some()
    {
        return infer_kind_from_open_code(provider);
    }
    parse_kind(provider.get("defaultKind"))
        .or_else(|| {
            provider
                .get("mode")
                .and_then(object)
                .and_then(|mode| parse_kind(mode.get("defaultKind")))
        })
        .unwrap_or_else(|| infer_kind_from_open_code(provider))
}

fn runtime_base_url_path(base_url: &str) -> String {
    url::Url::parse(base_url)
        .map(|url| url.path().to_lowercase())
        .unwrap_or_else(|_| base_url.to_lowercase())
}

fn is_bigmodel_coding_plan_legacy_openai_runtime(
    provider_id: &str,
    base_url: Option<&str>,
) -> bool {
    if provider_id != BIGMODEL_INDIVIDUAL_CODING_PLAN {
        return false;
    }
    let Some(base_url) = base_url.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    runtime_base_url_path(base_url).contains("/coding/paas/v4")
}

fn resolve_preset_kind_from_runtime_base_url(
    provider_id: &str,
    base_url: Option<&str>,
) -> Option<ProviderKind> {
    let base_url = base_url?;
    if !PRESET_GLM_PROVIDER_IDS.contains(&provider_id) {
        return None;
    }
    let normalized = base_url.trim().to_lowercase();
    if normalized.is_empty() {
        return None;
    }
    let path = runtime_base_url_path(&normalized);
    if is_bigmodel_coding_plan_legacy_openai_runtime(provider_id, Some(&normalized)) {
        return Some(ProviderKind::Anthropic);
    }
    if path.contains("/coding/paas/v4") {
        return Some(ProviderKind::OpenAiCompatible);
    }
    if path.contains("/api/anthropic") || path.contains("/mode-plan/anthropic") {
        return Some(ProviderKind::Anthropic);
    }
    None
}

fn normalize_bigmodel_coding_plan_anthropic_base_url_for_env(
    base_url: Option<&str>,
    env: &LegacyConfigReaderEnv<'_>,
) -> String {
    let fallback = if env.runtime_env == "production" {
        BIGMODEL_CODING_PLAN_ANTHROPIC_BASE_URL.to_string()
    } else {
        format!(
            "{}/api/anthropic",
            env.bigmodel_api_origin.trim_end_matches('/')
        )
    };
    let normalized = normalize_model_provider_base_url_for_kind(
        base_url.unwrap_or(&fallback),
        ProviderKind::Anthropic,
    );
    if env.runtime_env == "production" {
        return if normalized.is_empty() {
            fallback
        } else {
            normalized
        };
    }
    let Ok(parsed) = url::Url::parse(if normalized.is_empty() {
        &fallback
    } else {
        &normalized
    }) else {
        return fallback;
    };
    let Ok(production) = url::Url::parse(BIGMODEL_CODING_PLAN_ANTHROPIC_BASE_URL) else {
        return fallback;
    };
    if parsed.origin() == production.origin() || normalized.is_empty() {
        fallback
    } else {
        normalized
    }
}

fn resolve_open_code_endpoints(
    provider: &IndexMap<String, JsonValue>,
    default_kind: ProviderKind,
    provider_id: &str,
    env: &LegacyConfigReaderEnv<'_>,
) -> JsonValue {
    let options = provider.get("options").and_then(object);
    let base_url = non_empty(options.and_then(|options| options.get("baseURL")))
        .or_else(|| non_empty(provider.get("api")));
    if let Some(base_url) = base_url {
        if is_bigmodel_coding_plan_legacy_openai_runtime(provider_id, Some(&base_url)) {
            return object_from(vec![
                (
                    "baseURL".to_string(),
                    string(&normalize_bigmodel_coding_plan_anthropic_base_url_for_env(
                        None, env,
                    )),
                ),
                (
                    "paths".to_string(),
                    object_from(vec![("anthropic".to_string(), string("/v1/messages"))]),
                ),
            ]);
        }
        if provider_id == BIGMODEL_INDIVIDUAL_CODING_PLAN {
            return object_from(vec![
                (
                    "baseURL".to_string(),
                    string(&normalize_bigmodel_coding_plan_anthropic_base_url_for_env(
                        Some(&base_url),
                        env,
                    )),
                ),
                (
                    "paths".to_string(),
                    object_from(vec![("anthropic".to_string(), string("/v1/messages"))]),
                ),
            ]);
        }
        let normalized = normalize_model_provider_configured_base_url(&base_url);
        return object_from(vec![
            ("baseURL".to_string(), string(&normalized)),
            (
                "paths".to_string(),
                object_from(vec![(
                    default_kind.as_str().to_string(),
                    string(default_endpoint_path(default_kind)),
                )]),
            ),
        ]);
    }
    provider
        .get("endpoints")
        .cloned()
        .or_else(|| {
            provider
                .get("mode")
                .and_then(object)
                .and_then(|mode| mode.get("endpoints").cloned())
        })
        .unwrap_or_else(|| object_from(Vec::new()))
}

fn infer_reasoning_from_open_code(model: &IndexMap<String, JsonValue>) -> Option<JsonValue> {
    let reasoning = model.get("reasoning")?;
    if reasoning.as_bool() == Some(false) {
        return None;
    }
    let config = object(reasoning)?;
    if config.get("enabled").and_then(JsonValue::as_bool) == Some(false) {
        return None;
    }
    let variants = config
        .get("variants")
        .and_then(JsonValue::as_array)
        .map(|variants| variants.to_vec())
        .unwrap_or_default();
    if variants.is_empty() {
        return None;
    }
    let mut levels = IndexMap::new();
    for variant in variants {
        if let Some(level) = variant.as_str() {
            levels.insert(level.to_string(), object_from(Vec::new()));
        }
    }
    let mut entries = Vec::new();
    if let Some(default_level) = non_empty(config.get("defaultVariant")) {
        push(&mut entries, "defaultLevel", string(&default_level));
    }
    push(&mut entries, "levels", JsonValue::Object(levels));
    Some(object_from(entries))
}

fn open_code_reasoning(
    model: &IndexMap<String, JsonValue>,
    prefer_open_code_fields: bool,
) -> Option<JsonValue> {
    if !prefer_open_code_fields {
        if let Some(spec) = model.get("reasoningSpec") {
            return Some(strip_model_provider_reasoning_patches(spec));
        }
        if let Some(reasoning) = model
            .get("mode")
            .and_then(object)
            .and_then(|mode| mode.get("reasoning"))
        {
            return Some(strip_model_provider_reasoning_patches(reasoning));
        }
    }
    infer_reasoning_from_open_code(model)
}

fn open_code_model_to_provider_model(
    model_id: &str,
    model: &IndexMap<String, JsonValue>,
    provider_default_kind: ProviderKind,
    prefer_open_code_fields: bool,
) -> JsonValue {
    let mode_extensions = model.get("mode").and_then(object);
    // TS `openCodeModelToModelProviderModel`：prefer-open-code 时模型级 `mode` 整体
    // 置空（只有 modeExtensions 供 priority/modified/deleted）；否则才参与
    // supportsTools/defaultKind/disabledReason 等回退。
    let mode = if prefer_open_code_fields {
        None
    } else {
        mode_extensions
    };
    let limit = model.get("limit").and_then(object);
    let options = model.get("options").and_then(object);
    let context_window = positive(limit.and_then(|limit| limit.get("context")))
        .or_else(|| positive(model.get("contextWindow")));
    let max_output_tokens = positive(limit.and_then(|limit| limit.get("output")))
        .or_else(|| positive(options.and_then(|options| options.get("max_tokens"))))
        .or_else(|| positive(model.get("maxOutputTokens")));
    let has_max_output_tokens = if prefer_open_code_fields {
        None
    } else {
        bool_value(model.get("hasMaxOutputTokens"))
    }
    .or_else(|| bool_value(mode.and_then(|mode| mode.get("hasMaxOutputTokens"))))
    .unwrap_or(max_output_tokens.is_some());
    let kinds = if !prefer_open_code_fields {
        model
            .get("kinds")
            .and_then(JsonValue::as_array)
            .map(|kinds| {
                kinds
                    .iter()
                    .filter_map(|kind| parse_kind(Some(kind)))
                    .collect::<Vec<_>>()
            })
    } else {
        None
    }
    .filter(|kinds| !kinds.is_empty())
    .or_else(|| {
        mode.and_then(|mode| mode.get("kinds"))
            .and_then(JsonValue::as_array)
            .map(|kinds| {
                kinds
                    .iter()
                    .filter_map(|kind| parse_kind(Some(kind)))
                    .collect::<Vec<_>>()
            })
            .filter(|kinds| !kinds.is_empty())
    })
    .unwrap_or_else(|| vec![provider_default_kind]);
    let default_kind = if !prefer_open_code_fields {
        parse_kind(model.get("defaultKind"))
    } else {
        None
    }
    .or_else(|| mode.and_then(|mode| parse_kind(mode.get("defaultKind"))))
    .unwrap_or(provider_default_kind);
    let priority = finite(model.get("priority"))
        .or_else(|| finite(mode_extensions.and_then(|mode| mode.get("priority"))));
    let disabled_reason = non_empty(model.get("disabledReason"))
        .or_else(|| non_empty(mode.and_then(|mode| mode.get("disabledReason"))));
    let supports_tools = bool_value(model.get("supportsTools"))
        .or_else(|| bool_value(mode.and_then(|mode| mode.get("supportsTools"))));
    let supports_structured_output = bool_value(model.get("supportsStructuredOutput"))
        .or_else(|| bool_value(mode.and_then(|mode| mode.get("supportsStructuredOutput"))));
    let modified = bool_value(model.get("modified"))
        .or_else(|| bool_value(mode_extensions.and_then(|mode| mode.get("modified"))));
    let deleted = bool_value(model.get("deleted"))
        .or_else(|| bool_value(mode_extensions.and_then(|mode| mode.get("deleted"))));
    let id = non_empty(model.get("id")).unwrap_or_else(|| model_id.to_string());
    let modalities = model.get("modalities").and_then(object);
    create_model_provider_model_config(&CreateModelConfigParams {
        id,
        name: non_empty(model.get("name")),
        kinds,
        default_kind: Some(default_kind),
        context_window,
        max_output_tokens: has_max_output_tokens.then_some(max_output_tokens).flatten(),
        input_modalities: parse_modalities(
            modalities.and_then(|modalities| modalities.get("input")),
        ),
        output_modalities: parse_modalities(
            modalities.and_then(|modalities| modalities.get("output")),
        ),
        reasoning: open_code_reasoning(model, prefer_open_code_fields),
        priority,
        disabled_reason,
        supports_tools,
        supports_structured_output,
        modified,
        deleted,
    })
}

fn create_deleted_model_tombstone(model_id: &str, default_kind: ProviderKind) -> JsonValue {
    create_model_provider_model_config(&CreateModelConfigParams {
        id: model_id.to_string(),
        kinds: vec![default_kind],
        context_window: Some(LEGACY_FALLBACK_CONTEXT_WINDOW as f64),
        input_modalities: Some(vec![Modality::Text]),
        output_modalities: Some(vec![Modality::Text]),
        modified: Some(true),
        deleted: Some(true),
        ..CreateModelConfigParams::default()
    })
}

fn normalize_deleted_model_ids(value: Option<&JsonValue>) -> Vec<String> {
    let mut result = Vec::new();
    let mut seen = Vec::new();
    let Some(values) = value.and_then(JsonValue::as_array) else {
        return result;
    };
    for raw in values {
        let Some(model_id) = raw
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let key = model_id.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        result.push(model_id.to_string());
    }
    result
}

fn strip_mode_prefixed_provider_mappings(
    mappings: Option<&IndexMap<String, JsonValue>>,
) -> Option<JsonValue> {
    let mappings = mappings?;
    let filtered: IndexMap<String, JsonValue> = mappings
        .iter()
        .filter(|(key, _)| !key.to_lowercase().starts_with("mode"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Some(JsonValue::Object(filtered))
}

fn normalize_open_code_provider(
    provider_id: &str,
    provider: &IndexMap<String, JsonValue>,
    env: &LegacyConfigReaderEnv<'_>,
) -> Result<JsonValue, String> {
    let mode = provider.get("mode").and_then(object);
    let prefer_open_code_fields = has_open_code_runtime_fields(provider);
    let configured_default_kind = resolve_open_code_default_kind(provider);
    let options = provider.get("options").and_then(object);
    let configured_runtime_base_url = non_empty(options.and_then(|options| options.get("baseURL")))
        .or_else(|| non_empty(provider.get("api")));
    let default_kind = resolve_preset_kind_from_runtime_base_url(
        provider_id,
        configured_runtime_base_url.as_deref(),
    )
    .unwrap_or(configured_default_kind);
    let endpoints = resolve_open_code_endpoints(provider, default_kind, provider_id, env);
    let api_key =
        string_value(options.and_then(|options| options.get("apiKey"))).unwrap_or_default();
    let now = env.now_ms;
    let models_object = provider
        .get("models")
        .and_then(object)
        .cloned()
        .unwrap_or_default();
    let mut models = Vec::new();
    for (model_id, model) in &models_object {
        let Some(model_object) = object(model) else {
            return Err("provider model must be an object".to_string());
        };
        models.push(open_code_model_to_provider_model(
            model_id,
            model_object,
            default_kind,
            prefer_open_code_fields,
        ));
    }
    let model_keys: Vec<String> = models
        .iter()
        .filter_map(|model| {
            model
                .as_object()
                .and_then(|model| model.get("id"))
                .and_then(JsonValue::as_str)
                .map(|id| id.trim().to_lowercase())
        })
        .filter(|id| !id.is_empty())
        .collect();
    for deleted_model_id in
        normalize_deleted_model_ids(mode.and_then(|mode| mode.get("deletedModels")))
    {
        let key = deleted_model_id.to_lowercase();
        if model_keys.contains(&key) {
            continue;
        }
        models.push(create_deleted_model_tombstone(
            &deleted_model_id,
            default_kind,
        ));
    }
    let api_key_required = bool_value(provider.get("apiKeyRequired"))
        .or_else(|| bool_value(mode.and_then(|mode| mode.get("apiKeyRequired"))))
        .or_else(|| bool_value(options.and_then(|options| options.get("apiKeyRequired"))));
    let provider_mappings = if prefer_open_code_fields {
        None
    } else {
        provider
            .get("providerMappings")
            .and_then(object)
            .or_else(|| {
                mode.and_then(|mode| mode.get("providerMappings"))
                    .and_then(object)
            })
            .and_then(|mappings| strip_mode_prefixed_provider_mappings(Some(mappings)))
    };
    let mut entries = Vec::new();
    push(
        &mut entries,
        "id",
        string(&non_empty(provider.get("id")).unwrap_or_else(|| provider_id.to_string())),
    );
    push(
        &mut entries,
        "name",
        string(&non_empty(provider.get("name")).unwrap_or_else(|| provider_id.to_string())),
    );
    let enabled = bool_value(provider.get("enabled"))
        .or_else(|| bool_value(mode.and_then(|mode| mode.get("enabled"))));
    if let Some(enabled) = enabled {
        push(&mut entries, "enabled", JsonValue::Bool(enabled));
    }
    if let Some(reason) = non_empty(provider.get("systemDisabledReason"))
        .or_else(|| non_empty(mode.and_then(|mode| mode.get("systemDisabledReason"))))
    {
        push(&mut entries, "systemDisabledReason", string(&reason));
    }
    push(&mut entries, "endpoints", endpoints);
    let api_format = if prefer_open_code_fields {
        resolve_model_provider_kind_api_format(default_kind)
    } else {
        parse_api_format(provider.get("apiFormat"))
            .or_else(|| mode.and_then(|mode| parse_api_format(mode.get("apiFormat"))))
            .unwrap_or_else(|| resolve_model_provider_kind_api_format(default_kind))
    };
    push(&mut entries, "apiFormat", string(api_format.as_str()));
    let source = non_empty(provider.get("source"))
        .or_else(|| non_empty(mode.and_then(|mode| mode.get("source"))))
        .unwrap_or_else(|| "custom".to_string());
    push(&mut entries, "source", string(&source));
    if let Some(value) = non_empty(provider.get("catalogSourceId"))
        .or_else(|| non_empty(mode.and_then(|mode| mode.get("catalogSourceId"))))
    {
        push(&mut entries, "catalogSourceId", string(&value));
    }
    if let Some(value) = non_empty(provider.get("catalogProviderId"))
        .or_else(|| non_empty(mode.and_then(|mode| mode.get("catalogProviderId"))))
    {
        push(&mut entries, "catalogProviderId", string(&value));
    }
    if let Some(value) = non_empty(provider.get("modelsDevProviderId"))
        .or_else(|| non_empty(mode.and_then(|mode| mode.get("modelsDevProviderId"))))
    {
        push(&mut entries, "modelsDevProviderId", string(&value));
    }
    if let Some(value) = api_key_required {
        push(&mut entries, "apiKeyRequired", JsonValue::Bool(value));
    }
    let headers = provider
        .get("headers")
        .and_then(object)
        .cloned()
        .or_else(|| {
            mode.and_then(|mode| mode.get("headers"))
                .and_then(object)
                .cloned()
        })
        .or_else(|| read_string_record(options.and_then(|options| options.get("headers"))));
    if let Some(headers) = headers {
        push(&mut entries, "headers", JsonValue::Object(headers));
    }
    if let Some(value) = non_empty(provider.get("logoUrl"))
        .or_else(|| non_empty(mode.and_then(|mode| mode.get("logoUrl"))))
    {
        push(&mut entries, "logoUrl", string(&value));
    }
    push(&mut entries, "apiKey", string(&api_key));
    if let Some(value) = non_empty(provider.get("apiKeyUrl"))
        .or_else(|| non_empty(mode.and_then(|mode| mode.get("apiKeyUrl"))))
    {
        push(&mut entries, "apiKeyUrl", string(&value));
    }
    push(&mut entries, "defaultKind", string(default_kind.as_str()));
    push(&mut entries, "models", JsonValue::Array(models));
    if let Some(mappings) = provider_mappings {
        push(&mut entries, "providerMappings", mappings);
    }
    let created_at = if prefer_open_code_fields {
        now
    } else {
        finite(provider.get("createdAt"))
            .or_else(|| finite(mode.and_then(|mode| mode.get("createdAt"))))
            .map(|value| value as i64)
            .unwrap_or(now)
    };
    let updated_at = if prefer_open_code_fields {
        now
    } else {
        finite(provider.get("updatedAt"))
            .or_else(|| finite(mode.and_then(|mode| mode.get("updatedAt"))))
            .map(|value| value as i64)
            .unwrap_or(now)
    };
    push(&mut entries, "createdAt", number(created_at as f64));
    push(&mut entries, "updatedAt", number(updated_at as f64));
    Ok(object_from(entries))
}

fn has_canonical_endpoints(endpoints: Option<&JsonValue>) -> bool {
    let Some(endpoints) = endpoints.and_then(object) else {
        return false;
    };
    non_empty(endpoints.get("baseURL")).is_some() || endpoints.contains_key("paths")
}

fn resolve_provider_default_kind_for_store(provider: &IndexMap<String, JsonValue>) -> ProviderKind {
    let endpoints = provider.get("endpoints");
    if provider.contains_key("defaultKind")
        || provider.contains_key("apiFormat")
        || has_canonical_endpoints(endpoints)
    {
        return resolve_model_provider_default_kind(&provider_shape(provider));
    }
    let legacy = endpoints.and_then(object);
    if non_empty(legacy.and_then(|endpoints| endpoints.get("anthropic"))).is_some() {
        return ProviderKind::Anthropic;
    }
    if non_empty(legacy.and_then(|endpoints| endpoints.get("openai"))).is_some() {
        return ProviderKind::OpenAiCompatible;
    }
    resolve_model_provider_default_kind(&provider_shape(provider))
}

fn resolve_legacy_runtime_base_url_for_kind(
    endpoints: &IndexMap<String, JsonValue>,
    default_kind: ProviderKind,
) -> String {
    let raw_endpoint = if default_kind == ProviderKind::Anthropic {
        non_empty(endpoints.get("anthropic"))
    } else {
        non_empty(endpoints.get("openai"))
    };
    if let Some(raw) = raw_endpoint {
        return normalize_model_provider_base_url_for_kind(&raw, default_kind);
    }
    let fallback = non_empty(endpoints.get("anthropic"))
        .or_else(|| non_empty(endpoints.get("openai")))
        .unwrap_or_default();
    if fallback.is_empty() {
        String::new()
    } else {
        normalize_model_provider_base_url_for_kind(&fallback, default_kind)
    }
}

fn resolve_runtime_base_url_for_store(
    endpoints: &IndexMap<String, JsonValue>,
    default_kind: ProviderKind,
) -> String {
    let shape = ProviderShape {
        api_format: Some(resolve_model_provider_kind_api_format(default_kind)),
        default_kind: Some(default_kind),
        endpoints: EndpointsInput {
            base_url: non_empty(endpoints.get("baseURL")),
            paths: endpoints.get("paths").and_then(object).map(|paths| {
                paths
                    .iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_string()))
                    })
                    .collect()
            }),
        },
    };
    let runtime_base_url = resolve_model_provider_runtime_base_url(&shape, Some(default_kind));
    if !runtime_base_url.is_empty() {
        return runtime_base_url;
    }
    if let Some(base_url) = non_empty(endpoints.get("baseURL")) {
        return normalize_model_provider_base_url_for_kind(&base_url, default_kind);
    }
    resolve_legacy_runtime_base_url_for_kind(endpoints, default_kind)
}

fn normalize_endpoints(endpoints: Option<&JsonValue>, default_kind: ProviderKind) -> JsonValue {
    let Some(endpoints) = endpoints.and_then(object) else {
        return object_from(Vec::new());
    };
    let runtime_base_url = resolve_runtime_base_url_for_store(endpoints, default_kind);
    if runtime_base_url.is_empty() {
        return object_from(Vec::new());
    }
    object_from(vec![
        ("baseURL".to_string(), string(&runtime_base_url)),
        (
            "paths".to_string(),
            object_from(vec![(
                default_kind.as_str().to_string(),
                string(default_endpoint_path(default_kind)),
            )]),
        ),
    ])
}

fn normalize_model_kinds_from_provider(
    provider: &IndexMap<String, JsonValue>,
    model_id: &str,
) -> Vec<ProviderKind> {
    let formats = provider
        .get("modelSupportedFormats")
        .and_then(object)
        .and_then(|formats| formats.get(model_id))
        .and_then(JsonValue::as_array)
        .map(|formats| {
            formats
                .iter()
                .filter_map(|format| format.as_str().and_then(SupportedFormat::parse))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            parse_api_format(provider.get("apiFormat"))
                .map(default_supported_formats_from_api_format)
                .unwrap_or_else(|| {
                    default_supported_formats_from_endpoints(
                        non_empty(
                            provider
                                .get("endpoints")
                                .and_then(object)
                                .and_then(|e| e.get("baseURL")),
                        )
                        .as_deref(),
                        provider
                            .get("endpoints")
                            .and_then(object)
                            .and_then(|endpoints| endpoints.get("paths"))
                            .and_then(object)
                            .map(|paths| {
                                paths
                                    .iter()
                                    .filter_map(|(key, value)| {
                                        value.as_str().map(|value| (key.clone(), value.to_string()))
                                    })
                                    .collect()
                            })
                            .as_ref(),
                    )
                })
        });
    let mut kinds = Vec::new();
    for format in formats {
        let kind = match format {
            SupportedFormat::Anthropic => Some(ProviderKind::Anthropic),
            SupportedFormat::OpenAi => Some(ProviderKind::OpenAiCompatible),
            SupportedFormat::Responses => Some(ProviderKind::OpenAi),
            SupportedFormat::Gemini => None,
        };
        let Some(kind) = kind else {
            continue;
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        kinds.push(resolve_model_provider_default_kind(&provider_shape(
            provider,
        )));
    }
    kinds
}

fn normalize_model_provider_model_config_entry(
    model: &IndexMap<String, JsonValue>,
) -> Option<JsonValue> {
    let id = model.get("id").and_then(JsonValue::as_str)?.trim();
    if id.is_empty() {
        return None;
    }
    // `{...model, ...}` 保留未知字段；这里用原对象做底，只覆盖标准化字段。
    let mut entries = model.clone();
    entries.insert("id".to_string(), string(id));
    match model
        .get("name")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        Some(name) => {
            entries.insert("name".to_string(), string(name));
        }
        None => {
            entries.shift_remove("name");
        }
    }
    let mut kinds = Vec::new();
    for kind in model
        .get("kinds")
        .and_then(JsonValue::as_array)
        .unwrap_or_default()
    {
        if let Some(kind) = parse_kind(Some(kind)) {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
    }
    entries.insert(
        "kinds".to_string(),
        JsonValue::Array(kinds.iter().map(|kind| string(kind.as_str())).collect()),
    );
    let modalities = model.get("modalities").and_then(object);
    let input = parse_modalities(modalities.and_then(|modalities| modalities.get("input")))
        .unwrap_or_default();
    let output = parse_modalities(modalities.and_then(|modalities| modalities.get("output")))
        .unwrap_or_default();
    entries.insert(
        "modalities".to_string(),
        object_from(vec![
            (
                "input".to_string(),
                JsonValue::Array(
                    input
                        .iter()
                        .map(|modality| string(modality.as_str()))
                        .collect(),
                ),
            ),
            (
                "output".to_string(),
                JsonValue::Array(
                    output
                        .iter()
                        .map(|modality| string(modality.as_str()))
                        .collect(),
                ),
            ),
        ]),
    );
    entries.insert(
        "contextWindow".to_string(),
        number(
            crate::resolve_model_provider_context_window(finite(model.get("contextWindow"))) as f64,
        ),
    );
    Some(JsonValue::Object(entries))
}

fn create_model_from_provider(
    provider: &IndexMap<String, JsonValue>,
    model_id: &str,
    default_kind: ProviderKind,
) -> JsonValue {
    let display_name = provider
        .get("modelDisplayNames")
        .and_then(object)
        .and_then(|names| names.get(model_id))
        .and_then(JsonValue::as_str)
        .map(str::to_string);
    create_model_provider_model_config(&CreateModelConfigParams {
        id: model_id.to_string(),
        name: display_name,
        kinds: normalize_model_kinds_from_provider(provider, model_id),
        default_kind: Some(default_kind),
        ..CreateModelConfigParams::default()
    })
}

fn normalize_provider_models(provider: &IndexMap<String, JsonValue>) -> Vec<JsonValue> {
    let models = provider
        .get("models")
        .and_then(JsonValue::as_array)
        .map(|models| models.to_vec())
        .unwrap_or_default();
    let has_model_configs = models.iter().any(|model| model.as_object().is_some());
    let mut result = Vec::new();
    let mut seen = Vec::new();
    if has_model_configs {
        for model in &models {
            let (id, normalized) = if let Some(object) = model.as_object() {
                let Some(id) = object.get("id").and_then(JsonValue::as_str).map(str::trim) else {
                    continue;
                };
                if id.is_empty() {
                    continue;
                }
                let Some(normalized) = normalize_model_provider_model_config_entry(object) else {
                    continue;
                };
                (id.to_string(), normalized)
            } else {
                let Some(id) = model.as_str().map(str::trim).filter(|id| !id.is_empty()) else {
                    continue;
                };
                let default_kind = resolve_model_provider_default_kind(&provider_shape(provider));
                (
                    id.to_string(),
                    create_model_from_provider(provider, id, default_kind),
                )
            };
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
            result.push(normalized);
        }
        return result;
    }
    let legacy = JsonValue::Array(vec![JsonValue::Object(provider.clone())]);
    if let Ok(providers) = parse_legacy_model_provider_list_value(&legacy) {
        if let Some(legacy_provider) = providers.first() {
            let migrated = migrate_legacy_model_provider(legacy_provider);
            if let Some(models) = migrated
                .as_object()
                .and_then(|provider| provider.get("models"))
                .and_then(JsonValue::as_array)
            {
                return models
                    .iter()
                    .filter(|model| model.as_object().is_some())
                    .cloned()
                    .collect();
            }
        }
    }
    let default_kind = resolve_model_provider_default_kind(&provider_shape(provider));
    for model_id in get_model_provider_model_ids(&models) {
        if seen.contains(&model_id) {
            continue;
        }
        seen.push(model_id.clone());
        result.push(create_model_from_provider(
            provider,
            &model_id,
            default_kind,
        ));
    }
    result
}

fn normalize_provider_for_store(provider: &JsonValue) -> Result<JsonValue, String> {
    let Some(object) = provider.as_object() else {
        return Err("provider must be an object".to_string());
    };
    let default_kind = resolve_provider_default_kind_for_store(object);
    let endpoints = normalize_endpoints(object.get("endpoints"), default_kind);
    let mut with_endpoints = object.clone();
    with_endpoints.insert("endpoints".to_string(), endpoints.clone());
    let api_format = resolve_model_provider_api_format(&provider_shape(&with_endpoints));
    with_endpoints.insert("apiFormat".to_string(), string(api_format.as_str()));
    with_endpoints.insert("defaultKind".to_string(), string(default_kind.as_str()));
    let models = normalize_provider_models(&with_endpoints);
    with_endpoints.insert("models".to_string(), JsonValue::Array(models));
    with_endpoints.insert("endpoints".to_string(), endpoints);
    with_endpoints.shift_remove("modelDisplayNames");
    with_endpoints.shift_remove("modelSupportedFormats");
    let mappings = with_endpoints
        .get("providerMappings")
        .and_then(JsonValue::as_object)
        .and_then(|mappings| {
            let without_claude: IndexMap<String, JsonValue> = mappings
                .iter()
                .filter(|(key, _)| key.as_str() != "claude")
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            strip_mode_prefixed_provider_mappings(Some(&without_claude))
        });
    match mappings {
        Some(mappings) => {
            with_endpoints.insert("providerMappings".to_string(), mappings);
        }
        None => {
            with_endpoints.shift_remove("providerMappings");
        }
    }
    Ok(JsonValue::Object(with_endpoints))
}

fn apply_provider_store_migrations(providers: Vec<JsonValue>) -> Result<Vec<JsonValue>, String> {
    let mut normalized = Vec::new();
    for provider in providers {
        let Some(object) = provider.as_object() else {
            continue;
        };
        let id = object.get("id").and_then(JsonValue::as_str).unwrap_or("");
        let api_key = object
            .get("apiKey")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        if id == RETIRED_ZAPI_PROVIDER_ID
            || (id.starts_with("default-") && api_key.trim().is_empty())
        {
            continue;
        }
        normalized.push(normalize_provider_for_store(&provider)?);
    }
    Ok(normalized)
}

/// `readLegacyModeConfigProviders` 的纯逻辑：config JSON → v2 provider 列表。
pub fn read_mode_config_providers(
    config: &JsonValue,
    env: &LegacyConfigReaderEnv<'_>,
) -> Result<Vec<JsonValue>, String> {
    let Some(root) = config.as_object() else {
        return Err("config must be an object".to_string());
    };
    let Some(provider_map) = root.get("provider") else {
        return Ok(Vec::new());
    };
    let Some(provider_map) = provider_map.as_object() else {
        return Err("config.provider must be an object".to_string());
    };
    let mut providers = Vec::new();
    for (provider_id, provider) in provider_map {
        let Some(provider) = provider.as_object() else {
            return Err("provider must be an object".to_string());
        };
        providers.push(normalize_open_code_provider(provider_id, provider, env)?);
    }
    let providers = apply_provider_store_migrations(providers)?;
    // 读取结果不暴露 tombstone：持久化/远端合并仍需要它们，但可选模型目录不能看到。
    Ok(providers
        .into_iter()
        .map(|provider| {
            let Some(mut object) = provider.as_object().cloned() else {
                return provider;
            };
            if let Some(models) = object.get("models").and_then(JsonValue::as_array) {
                let filtered = models
                    .iter()
                    .filter(|model| {
                        model
                            .as_object()
                            .and_then(|model| model.get("deleted"))
                            .and_then(JsonValue::as_bool)
                            != Some(true)
                    })
                    .cloned()
                    .collect();
                object.insert("models".to_string(), JsonValue::Array(filtered));
            }
            JsonValue::Object(object)
        })
        .collect())
}

/// 解析 config JSON 文本并读取 Provider 列表。
pub fn read_mode_config_providers_from_str(
    config_json: &str,
    env: &LegacyConfigReaderEnv<'_>,
) -> Result<Vec<JsonValue>, String> {
    let config = JsonValue::from_json_str(config_json).map_err(|error| error.to_string())?;
    read_mode_config_providers(&config, env)
}
