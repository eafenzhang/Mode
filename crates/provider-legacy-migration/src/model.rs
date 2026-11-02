//! v2 Provider 模型的纯 helper（`legacyModelProviderSerialized.ts` 的导出函数）。
//!
//! 这些函数只做字段投影与 URL 归一化，不读文件、不访问网络。

use indexmap::IndexMap;
use provider_config::JsonValue;
use serde_json::Number;
use url::Url;

use crate::types::{ApiFormat, Modality, ProviderKind, SupportedFormat};

pub const MODEL_PROVIDER_NEW_MODEL_CONTEXT_WINDOW: i64 = 200_000;

/// v2 端点形状（`ModelProviderEndpoints`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EndpointsInput {
    pub base_url: Option<String>,
    pub paths: Option<IndexMap<String, String>>,
}

impl EndpointsInput {
    fn path_for(&self, kind: ProviderKind) -> Option<&str> {
        self.paths
            .as_ref()
            .and_then(|paths| paths.get(kind.as_str()))
            .map(String::as_str)
    }
}

/// `resolveModelProviderApiFormat` / `resolveModelProviderDefaultKind` 的输入。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderShape {
    pub api_format: Option<ApiFormat>,
    pub default_kind: Option<ProviderKind>,
    pub endpoints: EndpointsInput,
}

pub fn map_api_format_to_kind(api_format: ApiFormat) -> ProviderKind {
    match api_format {
        ApiFormat::AnthropicMessages => ProviderKind::Anthropic,
        ApiFormat::OpenAiResponses => ProviderKind::OpenAi,
        ApiFormat::OpenAiChatCompletions => ProviderKind::OpenAiCompatible,
    }
}

/// `resolveModelProviderKindApiFormat`。
pub fn resolve_model_provider_kind_api_format(kind: ProviderKind) -> ApiFormat {
    match kind {
        ProviderKind::Anthropic => ApiFormat::AnthropicMessages,
        ProviderKind::OpenAi => ApiFormat::OpenAiResponses,
        ProviderKind::OpenAiCompatible => ApiFormat::OpenAiChatCompletions,
    }
}

/// `resolveModelProviderApiFormat`。
pub fn resolve_model_provider_api_format(provider: &ProviderShape) -> ApiFormat {
    if let Some(api_format) = provider.api_format {
        return api_format;
    }
    if let Some(default_kind) = provider.default_kind {
        return resolve_model_provider_kind_api_format(default_kind);
    }
    if provider
        .endpoints
        .path_for(ProviderKind::Anthropic)
        .is_some()
    {
        return ApiFormat::AnthropicMessages;
    }
    if provider.endpoints.path_for(ProviderKind::OpenAi).is_some() {
        return ApiFormat::OpenAiResponses;
    }
    if provider
        .endpoints
        .path_for(ProviderKind::OpenAiCompatible)
        .is_some()
    {
        return ApiFormat::OpenAiChatCompletions;
    }
    ApiFormat::AnthropicMessages
}

/// `resolveModelProviderDefaultKind`。
pub fn resolve_model_provider_default_kind(provider: &ProviderShape) -> ProviderKind {
    if let Some(default_kind) = provider.default_kind {
        return default_kind;
    }
    if provider
        .endpoints
        .path_for(ProviderKind::OpenAiCompatible)
        .is_some()
    {
        return ProviderKind::OpenAiCompatible;
    }
    if provider.endpoints.path_for(ProviderKind::OpenAi).is_some() {
        return ProviderKind::OpenAi;
    }
    if provider
        .endpoints
        .path_for(ProviderKind::Anthropic)
        .is_some()
    {
        return ProviderKind::Anthropic;
    }
    map_api_format_to_kind(resolve_model_provider_api_format(provider))
}

/// `getDefaultModelProviderEndpointPathForKind`。
pub fn default_endpoint_path(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "/v1/messages",
        ProviderKind::OpenAi => "/responses",
        ProviderKind::OpenAiCompatible => "/chat/completions",
    }
}

fn collapse_duplicated_absolute_runtime_base_url(value: &str) -> String {
    crate::migrate::collapse_duplicated_absolute_runtime_base_url(value)
}

/// `normalizeModelProviderConfiguredBaseUrl`：只折叠重复绝对 URL 并去尾斜杠。
pub fn normalize_model_provider_configured_base_url(base_url: &str) -> String {
    collapse_duplicated_absolute_runtime_base_url(base_url)
        .trim_end_matches('/')
        .to_string()
}

fn is_absolute_http_url(value: &str) -> bool {
    Url::parse(value)
        .map(|url| matches!(url.scheme(), "http" | "https"))
        .unwrap_or(false)
}

fn join_base_url_and_path(base_url: &str, path: &str) -> String {
    let trimmed_base = base_url.trim();
    let trimmed_path = path.trim();
    if trimmed_path.is_empty() {
        return trimmed_base.to_string();
    }
    if is_absolute_http_url(trimmed_path) {
        return trimmed_path.to_string();
    }
    if trimmed_base.is_empty() {
        return trimmed_path.to_string();
    }
    if trimmed_base.ends_with('/') && trimmed_path.starts_with('/') {
        return format!(
            "{}{}",
            &trimmed_base[..trimmed_base.len() - 1],
            trimmed_path
        );
    }
    if !trimmed_base.ends_with('/') && !trimmed_path.starts_with('/') {
        return format!("{trimmed_base}/{trimmed_path}");
    }
    format!("{trimmed_base}{trimmed_path}")
}

/// `resolveModelProviderRuntimeBaseUrl`。
pub fn resolve_model_provider_runtime_base_url(
    provider: &ProviderShape,
    kind: Option<ProviderKind>,
) -> String {
    let base_url = provider.endpoints.base_url.as_deref().unwrap_or("").trim();
    if base_url.is_empty() && provider.endpoints.paths.is_none() {
        return String::new();
    }
    let kind =
        kind.unwrap_or_else(|| map_api_format_to_kind(resolve_model_provider_api_format(provider)));
    let Some(path) = provider.endpoints.path_for(kind) else {
        return String::new();
    };
    crate::migrate::normalize_model_provider_base_url_for_kind(
        &join_base_url_and_path(base_url, path),
        kind,
    )
}

/// `getDefaultModelSupportedFormatsFromApiFormat`。
pub fn default_supported_formats_from_api_format(api_format: ApiFormat) -> Vec<SupportedFormat> {
    match api_format {
        ApiFormat::AnthropicMessages => vec![SupportedFormat::Anthropic],
        ApiFormat::OpenAiResponses => vec![SupportedFormat::Responses],
        ApiFormat::OpenAiChatCompletions => vec![SupportedFormat::OpenAi],
    }
}

/// `getDefaultModelSupportedFormatsFromEndpoints`。
pub fn default_supported_formats_from_endpoints(
    base_url: Option<&str>,
    paths: Option<&IndexMap<String, String>>,
) -> Vec<SupportedFormat> {
    let has_base = base_url.is_some_and(|base| !base.trim().is_empty());
    if !has_base && paths.is_none() {
        return Vec::new();
    }
    let mut formats = Vec::new();
    if paths.is_some_and(|paths| paths.contains_key("anthropic")) {
        formats.push(SupportedFormat::Anthropic);
    }
    if paths.is_some_and(|paths| paths.contains_key("openai-compatible")) {
        formats.push(SupportedFormat::OpenAi);
    }
    if paths.is_some_and(|paths| paths.contains_key("openai")) {
        formats.push(SupportedFormat::Responses);
    }
    formats
}

/// `resolveModelProviderContextWindow`。
pub fn resolve_model_provider_context_window(context_window: Option<f64>) -> i64 {
    let Some(value) = context_window.filter(|value| value.is_finite()) else {
        return MODEL_PROVIDER_NEW_MODEL_CONTEXT_WINDOW;
    };
    let normalized = value.floor() as i64;
    if normalized > 0 {
        normalized
    } else {
        MODEL_PROVIDER_NEW_MODEL_CONTEXT_WINDOW
    }
}

fn json_string(value: &str) -> JsonValue {
    JsonValue::String(value.to_string())
}

fn json_number(value: f64) -> Option<JsonValue> {
    // JSON.stringify 对整数值输出 `128` 而不是 `128.0`；保持同一文本形态。
    if value.is_finite() && value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_992.0 {
        return Some(JsonValue::Number(Number::from(value as i64)));
    }
    Number::from_f64(value).map(JsonValue::Number)
}

fn unique_modalities(values: Vec<Modality>) -> Vec<Modality> {
    let mut unique = Vec::new();
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

fn unique_kinds(values: Vec<ProviderKind>) -> Vec<ProviderKind> {
    let mut unique = Vec::new();
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

/// `createModelProviderModelConfig` 的入参。
#[derive(Debug, Clone, Default)]
pub struct CreateModelConfigParams {
    pub id: String,
    pub name: Option<String>,
    pub kinds: Vec<ProviderKind>,
    pub default_kind: Option<ProviderKind>,
    pub context_window: Option<f64>,
    pub max_output_tokens: Option<f64>,
    pub input_modalities: Option<Vec<Modality>>,
    pub output_modalities: Option<Vec<Modality>>,
    pub reasoning: Option<JsonValue>,
    pub priority: Option<f64>,
    pub disabled_reason: Option<String>,
    pub supports_tools: Option<bool>,
    pub supports_structured_output: Option<bool>,
    pub modified: Option<bool>,
    pub deleted: Option<bool>,
}

/// `createModelProviderModelConfig`。
pub fn create_model_provider_model_config(params: &CreateModelConfigParams) -> JsonValue {
    let input_modalities = unique_modalities(
        params
            .input_modalities
            .clone()
            .unwrap_or_else(|| vec![Modality::Text]),
    );
    let output_modalities = unique_modalities(
        params
            .output_modalities
            .clone()
            .unwrap_or_else(|| vec![Modality::Text]),
    );
    let mut entries: Vec<(String, JsonValue)> = Vec::new();
    entries.push(("id".to_string(), json_string(params.id.trim())));
    if let Some(name) = params
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        entries.push(("name".to_string(), json_string(name)));
    }
    let kinds = unique_kinds(params.kinds.clone());
    entries.push((
        "kinds".to_string(),
        JsonValue::Array(
            kinds
                .iter()
                .map(|kind| json_string(kind.as_str()))
                .collect(),
        ),
    ));
    if let Some(default_kind) = params.default_kind {
        entries.push((
            "defaultKind".to_string(),
            json_string(default_kind.as_str()),
        ));
    }
    let modalities = vec![
        (
            "input".to_string(),
            JsonValue::Array(
                input_modalities
                    .iter()
                    .map(|modality| json_string(modality.as_str()))
                    .collect(),
            ),
        ),
        (
            "output".to_string(),
            JsonValue::Array(
                output_modalities
                    .iter()
                    .map(|modality| json_string(modality.as_str()))
                    .collect(),
            ),
        ),
    ];
    entries.push(("modalities".to_string(), JsonValue::object_from(modalities)));
    entries.push((
        "contextWindow".to_string(),
        JsonValue::Number(Number::from(resolve_model_provider_context_window(
            params.context_window,
        ))),
    ));
    if let Some(max_output_tokens) = params.max_output_tokens.filter(|value| *value != 0.0) {
        if let Some(number) = json_number(max_output_tokens) {
            entries.push(("maxOutputTokens".to_string(), number));
        }
    }
    if let Some(reasoning) = &params.reasoning {
        entries.push(("reasoning".to_string(), reasoning.clone()));
    }
    if let Some(priority) = params.priority.filter(|value| value.is_finite()) {
        if let Some(number) = json_number(priority) {
            entries.push(("priority".to_string(), number));
        }
    }
    if let Some(reason) = params
        .disabled_reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
    {
        entries.push(("disabledReason".to_string(), json_string(reason)));
    }
    if let Some(value) = params.supports_tools {
        entries.push(("supportsTools".to_string(), JsonValue::Bool(value)));
    }
    if let Some(value) = params.supports_structured_output {
        entries.push((
            "supportsStructuredOutput".to_string(),
            JsonValue::Bool(value),
        ));
    }
    if let Some(value) = params.modified {
        entries.push(("modified".to_string(), JsonValue::Bool(value)));
    }
    if let Some(value) = params.deleted {
        entries.push(("deleted".to_string(), JsonValue::Bool(value)));
    }
    JsonValue::object_from(entries)
}

fn strip_legacy_claude_provider_mappings_inner(
    mappings: &IndexMap<String, JsonValue>,
) -> JsonValue {
    let remaining: IndexMap<String, JsonValue> = mappings
        .iter()
        .filter(|(key, _)| key.as_str() != "claude")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    JsonValue::Object(remaining)
}

/// `stripLegacyClaudeProviderMappings`。
pub fn strip_legacy_claude_provider_mappings(
    mappings: Option<&IndexMap<String, JsonValue>>,
) -> Option<JsonValue> {
    mappings.map(strip_legacy_claude_provider_mappings_inner)
}

/// `stripModelProviderReasoningPatches`：保留占位 level，清空所有 patch。
pub fn strip_model_provider_reasoning_patches(reasoning: &JsonValue) -> JsonValue {
    let empty = IndexMap::new();
    let object = reasoning.as_object().unwrap_or(&empty);
    let mut entries: Vec<(String, JsonValue)> = Vec::new();
    if let Some(default_level) = object
        .get("defaultLevel")
        .and_then(JsonValue::as_str)
        .filter(|level| !level.is_empty())
    {
        entries.push(("defaultLevel".to_string(), json_string(default_level)));
    }
    let levels = object.get("levels").and_then(JsonValue::as_object);
    let stripped_levels: IndexMap<String, JsonValue> = levels
        .map(|levels| {
            levels
                .keys()
                .map(|key| (key.clone(), JsonValue::Object(IndexMap::new())))
                .collect()
        })
        .unwrap_or_default();
    entries.push(("levels".to_string(), JsonValue::Object(stripped_levels)));
    JsonValue::object_from(entries)
}

/// `getModelProviderModelIds`：tombstone 视为不存在，id 去空白并丢弃空串。
pub fn get_model_provider_model_ids(models: &[JsonValue]) -> Vec<String> {
    let mut ids = Vec::new();
    for model in models {
        let id = match model {
            JsonValue::String(value) => value.trim(),
            JsonValue::Object(object) => {
                if object.get("deleted").and_then(JsonValue::as_bool) == Some(true) {
                    continue;
                }
                object
                    .get("id")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("")
                    .trim()
            }
            _ => continue,
        };
        if !id.is_empty() {
            ids.push(id.to_string());
        }
    }
    ids
}
