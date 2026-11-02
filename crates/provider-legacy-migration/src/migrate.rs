//! `migrateLegacyModelProviderConfig`：旧 Provider → `mode.model-providers.v2` 条目。
//!
//! URL 归一化与折叠规则逐行对齐 TS；输出用保序 `JsonValue`，字段省略语义与
//! `JSON.stringify` 一致（`undefined` 不落盘）。

use provider_config::JsonValue;
use serde_json::Number;
use url::Url;

use crate::types::{
    ApiFormat, LegacyEndpoints, LegacyModelProvider, ProviderKind, SupportedFormat,
};

const LEGACY_MODEL_CONTEXT_WINDOW: i64 = 200_000;
const GEMINI_UNSUPPORTED_REASON: &str =
    "legacy gemini format is not supported by mode.model-providers.v2";

fn string(value: &str) -> JsonValue {
    JsonValue::String(value.to_string())
}

fn array(values: Vec<JsonValue>) -> JsonValue {
    JsonValue::Array(values)
}

fn object(entries: Vec<(String, JsonValue)>) -> JsonValue {
    JsonValue::object_from(entries)
}

fn push(entries: &mut Vec<(String, JsonValue)>, key: &str, value: JsonValue) {
    entries.push((key.to_string(), value));
}

fn path_with_search(url: &Url) -> String {
    let pathname = url.path();
    let path = if pathname == "/" {
        ""
    } else {
        pathname.trim_end_matches('/')
    };
    match url.query() {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        Some(_) => format!("{path}?"),
        None => path.to_string(),
    }
}

/// `collapseDuplicatedAbsoluteRuntimeBaseUrl`。
pub(crate) fn collapse_duplicated_absolute_runtime_base_url(value: &str) -> String {
    let normalized = value.trim().trim_end_matches('/');
    if normalized.is_empty() {
        return String::new();
    }
    let Ok(parsed) = Url::parse(normalized) else {
        return normalized.to_string();
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return normalized.to_string();
    }
    let Some(host) = parsed.host_str() else {
        return normalized.to_string();
    };
    let host_with_port = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let marker = format!("{}://{}", parsed.scheme(), host_with_port);
    let Some(rest) = normalized.get(marker.len()..) else {
        return normalized.to_string();
    };
    let Some(offset) = rest.find(&marker) else {
        return normalized.to_string();
    };
    let second = marker.len() + offset;
    let first_url = normalized[..second].trim_end_matches('/');
    let second_url = normalized[second..].trim_end_matches('/');
    if first_url == second_url {
        first_url.to_string()
    } else {
        normalized.to_string()
    }
}

fn kind_suffixes(kind: ProviderKind) -> &'static [&'static str] {
    match kind {
        ProviderKind::Anthropic => &["/v1/messages", "/messages"],
        ProviderKind::OpenAi => &["/responses"],
        ProviderKind::OpenAiCompatible => &["/chat/completions"],
    }
}

/// `normalizeModelProviderBaseUrlForKind`。
pub fn normalize_model_provider_base_url_for_kind(base_url: &str, kind: ProviderKind) -> String {
    let mut normalized = collapse_duplicated_absolute_runtime_base_url(base_url);
    normalized = normalized.trim_end_matches('/').to_string();
    let lower = normalized.to_lowercase();
    for suffix in kind_suffixes(kind) {
        if lower.ends_with(suffix) {
            normalized.truncate(normalized.len() - suffix.len());
            break;
        }
    }
    normalized.trim_end_matches('/').to_string()
}

fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

/// `buildEndpointsFromLegacyRuntimeBaseUrls`。
fn build_endpoints_from_legacy_runtime_base_urls(entries: &[(ProviderKind, String)]) -> JsonValue {
    let normalized: Vec<(ProviderKind, String)> = entries
        .iter()
        .filter_map(|(kind, raw_url)| {
            let url = normalize_model_provider_base_url_for_kind(raw_url, *kind);
            (!url.is_empty()).then_some((*kind, url))
        })
        .collect();
    if normalized.is_empty() {
        return object(Vec::new());
    }
    let parsed: Vec<(ProviderKind, String, Option<Url>)> = normalized
        .into_iter()
        .map(|(kind, url)| {
            let parsed = Url::parse(&url).ok();
            (kind, url, parsed)
        })
        .collect();
    let first_origin = parsed
        .first()
        .and_then(|entry| entry.2.as_ref().map(origin));
    let can_share_origin = first_origin.as_ref().is_some_and(|first| {
        parsed
            .iter()
            .all(|entry| entry.2.as_ref().is_some_and(|url| origin(url) == *first))
    });

    let mut output = Vec::new();
    if can_share_origin {
        if let Some(first) = &first_origin {
            push(&mut output, "baseURL", string(first));
        }
    }
    let paths: Vec<(String, JsonValue)> = parsed
        .iter()
        .map(|(kind, url, parsed)| {
            let value = if can_share_origin {
                parsed
                    .as_ref()
                    .map(path_with_search)
                    .unwrap_or_else(|| url.clone())
            } else {
                url.clone()
            };
            (kind.as_str().to_string(), string(&value))
        })
        .collect();
    push(&mut output, "paths", JsonValue::object_from(paths));
    object(output)
}

fn resolve_legacy_default_kind(provider: &LegacyModelProvider) -> ProviderKind {
    if let Some(api_format) = provider.api_format {
        return match api_format {
            ApiFormat::AnthropicMessages => ProviderKind::Anthropic,
            ApiFormat::OpenAiResponses => ProviderKind::OpenAi,
            ApiFormat::OpenAiChatCompletions => ProviderKind::OpenAiCompatible,
        };
    }
    if !provider.endpoints.anthropic.trim().is_empty() {
        return ProviderKind::Anthropic;
    }
    if !provider.endpoints.openai.trim().is_empty() {
        return ProviderKind::OpenAiCompatible;
    }
    ProviderKind::Anthropic
}

fn default_formats_from_legacy_endpoints(endpoints: &LegacyEndpoints) -> Vec<SupportedFormat> {
    let mut formats = Vec::new();
    if !endpoints.anthropic.trim().is_empty() {
        formats.push(SupportedFormat::Anthropic);
    }
    if !endpoints.openai.trim().is_empty() {
        formats.push(SupportedFormat::OpenAi);
    }
    formats
}

fn default_formats_from_api_format(api_format: ApiFormat) -> Vec<SupportedFormat> {
    match api_format {
        ApiFormat::AnthropicMessages => vec![SupportedFormat::Anthropic],
        ApiFormat::OpenAiResponses => vec![SupportedFormat::Responses],
        ApiFormat::OpenAiChatCompletions => vec![SupportedFormat::OpenAi],
    }
}

fn format_to_kind(format: SupportedFormat) -> Option<ProviderKind> {
    match format {
        SupportedFormat::Anthropic => Some(ProviderKind::Anthropic),
        SupportedFormat::OpenAi => Some(ProviderKind::OpenAiCompatible),
        SupportedFormat::Responses => Some(ProviderKind::OpenAi),
        SupportedFormat::Gemini => None,
    }
}

fn unique_kinds(kinds: impl IntoIterator<Item = ProviderKind>) -> Vec<ProviderKind> {
    let mut unique = Vec::new();
    for kind in kinds {
        if !unique.contains(&kind) {
            unique.push(kind);
        }
    }
    unique
}

fn create_model_config(
    id: &str,
    name: Option<&str>,
    kinds: Vec<ProviderKind>,
    default_kind: Option<ProviderKind>,
    disabled_reason: Option<&str>,
) -> JsonValue {
    let mut entries = Vec::new();
    push(&mut entries, "id", string(id.trim()));
    if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
        push(&mut entries, "name", string(name));
    }
    push(
        &mut entries,
        "kinds",
        array(kinds.iter().map(|kind| string(kind.as_str())).collect()),
    );
    if let Some(kind) = default_kind {
        push(&mut entries, "defaultKind", string(kind.as_str()));
    }
    push(
        &mut entries,
        "modalities",
        object(vec![
            ("input".to_string(), array(vec![string("text")])),
            ("output".to_string(), array(vec![string("text")])),
        ]),
    );
    push(
        &mut entries,
        "contextWindow",
        JsonValue::Number(Number::from(LEGACY_MODEL_CONTEXT_WINDOW)),
    );
    if let Some(reason) = disabled_reason.filter(|reason| !reason.is_empty()) {
        push(&mut entries, "disabledReason", string(reason));
    }
    object(entries)
}

/// `migrateLegacyModelProviderConfig`。
pub fn migrate_legacy_model_provider(provider: &LegacyModelProvider) -> JsonValue {
    let default_kind = resolve_legacy_default_kind(provider);
    let mut legacy_entries: Vec<(ProviderKind, String)> = Vec::new();
    let anthropic_endpoint = provider.endpoints.anthropic.trim();
    if !anthropic_endpoint.is_empty() {
        legacy_entries.push((ProviderKind::Anthropic, anthropic_endpoint.to_string()));
    }
    let openai_endpoint = provider.endpoints.openai.trim();
    if !openai_endpoint.is_empty() {
        let kind = if default_kind == ProviderKind::OpenAi {
            ProviderKind::OpenAi
        } else {
            ProviderKind::OpenAiCompatible
        };
        legacy_entries.push((kind, openai_endpoint.to_string()));
    }
    let endpoints = build_endpoints_from_legacy_runtime_base_urls(&legacy_entries);
    let migrated_models: Vec<JsonValue> = provider
        .models
        .iter()
        .map(|model_id| {
            let formats = provider
                .model_supported_formats
                .as_ref()
                .and_then(|formats| formats.get(model_id).cloned())
                .unwrap_or_else(|| match provider.api_format {
                    Some(api_format) => default_formats_from_api_format(api_format),
                    None => default_formats_from_legacy_endpoints(&provider.endpoints),
                });
            let kinds = unique_kinds(formats.iter().filter_map(|format| format_to_kind(*format)));
            let disabled_reason = (kinds.is_empty() && formats.contains(&SupportedFormat::Gemini))
                .then_some(GEMINI_UNSUPPORTED_REASON);
            let model_default_kind = if kinds.contains(&default_kind) {
                Some(default_kind)
            } else {
                kinds.first().copied()
            };
            create_model_config(
                model_id,
                provider
                    .model_display_names
                    .as_ref()
                    .and_then(|names| names.get(model_id))
                    .map(String::as_str),
                kinds,
                model_default_kind,
                disabled_reason,
            )
        })
        .collect();

    let mut output = Vec::new();
    push(&mut output, "id", string(&provider.id));
    push(&mut output, "name", string(&provider.name));
    if let Some(enabled) = provider.enabled {
        push(&mut output, "enabled", JsonValue::Bool(enabled));
    }
    if let Some(reason) = provider.system_disabled_reason {
        push(&mut output, "systemDisabledReason", string(reason.as_str()));
    }
    push(&mut output, "endpoints", endpoints);
    if let Some(api_format) = provider.api_format {
        push(&mut output, "apiFormat", string(api_format.as_str()));
    }
    if let Some(source) = provider.source {
        push(&mut output, "source", string(source.as_str()));
    }
    if let Some(models_dev_provider_id) = &provider.models_dev_provider_id {
        push(
            &mut output,
            "modelsDevProviderId",
            string(models_dev_provider_id),
        );
    }
    if let Some(api_key_required) = provider.api_key_required {
        push(
            &mut output,
            "apiKeyRequired",
            JsonValue::Bool(api_key_required),
        );
    }
    if let Some(headers) = &provider.headers {
        push(
            &mut output,
            "headers",
            JsonValue::object_from(
                headers
                    .iter()
                    .map(|(key, value)| (key.clone(), string(value))),
            ),
        );
    }
    if let Some(logo_url) = &provider.logo_url {
        push(&mut output, "logoUrl", string(logo_url));
    }
    push(&mut output, "apiKey", string(&provider.api_key));
    if let Some(api_key_url) = &provider.api_key_url {
        push(&mut output, "apiKeyUrl", string(api_key_url));
    }
    push(&mut output, "defaultKind", string(default_kind.as_str()));
    push(&mut output, "models", array(migrated_models));
    if let Some(provider_mappings) =
        crate::model::strip_legacy_claude_provider_mappings(provider.provider_mappings.as_ref())
    {
        push(&mut output, "providerMappings", provider_mappings);
    }
    push(
        &mut output,
        "createdAt",
        JsonValue::Number(provider.created_at.clone()),
    );
    push(
        &mut output,
        "updatedAt",
        JsonValue::Number(provider.updated_at.clone()),
    );
    object(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_duplicated_absolute_url() {
        assert_eq!(
            normalize_model_provider_base_url_for_kind(
                "https://api.example.com/v1/messages/https://api.example.com/v1/messages",
                ProviderKind::Anthropic
            ),
            "https://api.example.com"
        );
    }
}
