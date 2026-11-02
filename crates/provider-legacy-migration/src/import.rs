//! 旧 Effective Store → Personal Provider 层导入（纯转换）。
//!
//! 真源：`packages/services/src/model-provider/legacyPersonalProviderConfigImporter.ts`。
//! 输出与 `ProviderConfigMap.toJSON()` / `ModelConfigRules.rules()` 的语义一致，
//! 不负责落盘与配置服务编排。

use indexmap::IndexMap;
use provider_config::JsonValue;
use serde_json::Number;

use crate::reader::provider_shape;
use crate::{
    normalize_model_provider_configured_base_url, resolve_model_provider_api_format,
    resolve_model_provider_runtime_base_url,
};

const BIGMODEL_TEMPLATE_ID: &str = "bigmodel-api";
const ZAI_TEMPLATE_ID: &str = "zai-api";

fn object(value: &JsonValue) -> Option<&IndexMap<String, JsonValue>> {
    value.as_object()
}

fn trimmed(value: Option<&JsonValue>) -> Option<String> {
    value?.as_str().map(str::trim).map(str::to_string)
}

fn non_empty(value: Option<&JsonValue>) -> Option<String> {
    trimmed(value).filter(|value| !value.is_empty())
}

fn read_bool(value: Option<&JsonValue>) -> Option<bool> {
    value?.as_bool()
}

fn number(value: f64) -> JsonValue {
    JsonValue::Number(Number::from(value as i64))
}

fn string(value: &str) -> JsonValue {
    JsonValue::String(value.to_string())
}

fn object_from(entries: Vec<(String, JsonValue)>) -> JsonValue {
    JsonValue::object_from(entries)
}

fn is_positive_integer(value: Option<&JsonValue>) -> Option<i64> {
    let value = value?.as_number()?.as_f64()?;
    if value.is_finite() && value.fract() == 0.0 && value > 0.0 && value <= i64::MAX as f64 {
        Some(value as i64)
    } else {
        None
    }
}

fn model_members(provider: &IndexMap<String, JsonValue>) -> Vec<(String, Option<i64>)> {
    let mut members = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let Some(models) = provider.get("models").and_then(JsonValue::as_array) else {
        return members;
    };
    for entry in models {
        let (model_id, context_window, deleted) = match entry {
            JsonValue::String(value) => (value.trim().to_string(), None, false),
            JsonValue::Object(model) => {
                let model_id = model
                    .get("id")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let context_window = is_positive_integer(model.get("contextWindow"));
                let deleted = read_bool(model.get("deleted")) == Some(true);
                (model_id, context_window, deleted)
            }
            _ => continue,
        };
        if model_id.is_empty() || seen.contains(&model_id) || deleted {
            continue;
        }
        seen.push(model_id.clone());
        members.push((model_id, context_window));
    }
    members
}

fn access_object(api_key: Option<&str>, api_key_url: Option<&JsonValue>) -> JsonValue {
    let mut entries = vec![("type".to_string(), string("api-key"))];
    if let Some(api_key) = api_key {
        entries.push(("apiKey".to_string(), string(api_key)));
    }
    if let Some(api_key_url) = api_key_url.filter(|value| !value.is_null()) {
        entries.push(("apiKeyManagementUrl".to_string(), api_key_url.clone()));
    }
    object_from(entries)
}

fn api_object(provider: &IndexMap<String, JsonValue>) -> JsonValue {
    let shape = provider_shape(provider);
    let api_format = resolve_model_provider_api_format(&shape);
    let base_url = resolve_model_provider_runtime_base_url(&shape, None);
    let base_url = if base_url.is_empty() {
        let endpoint_base_url = provider
            .get("endpoints")
            .and_then(object)
            .and_then(|endpoints| endpoints.get("baseURL"))
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        normalize_model_provider_configured_base_url(endpoint_base_url)
    } else {
        base_url
    };
    let mut entries = vec![
        ("type".to_string(), string(api_format.as_str())),
        ("baseUrl".to_string(), string(&base_url)),
    ];
    if let Some(headers) = provider.get("headers").and_then(object) {
        entries.push(("headers".to_string(), JsonValue::Object(headers.clone())));
    }
    object_from(entries)
}

fn config_object(provider: &IndexMap<String, JsonValue>) -> JsonValue {
    let model_ids: Vec<JsonValue> = model_members(provider)
        .into_iter()
        .map(|(model_id, _)| string(&model_id))
        .collect();
    let api_key = non_empty(provider.get("apiKey"));
    let mut entries = vec![("group".to_string(), string("standard-personal"))];
    entries.push((
        "access".to_string(),
        access_object(api_key.as_deref(), provider.get("apiKeyUrl")),
    ));
    entries.push(("api".to_string(), api_object(provider)));
    if !model_ids.is_empty() {
        entries.push((
            "personalModelIds".to_string(),
            JsonValue::Array(model_ids.clone()),
        ));
        entries.push(("modelOrder".to_string(), JsonValue::Array(model_ids)));
    }
    object_from(entries)
}

fn provider_rule(
    provider_id: &str,
    template_id: Option<&str>,
    provider_name: Option<&str>,
    enabled: Option<bool>,
    config: JsonValue,
) -> JsonValue {
    let mut entries = vec![("providerId".to_string(), string(provider_id))];
    if let Some(template_id) = template_id {
        entries.push(("templateId".to_string(), string(template_id)));
    }
    if let Some(enabled) = enabled {
        entries.push(("enabled".to_string(), JsonValue::Bool(enabled)));
    }
    if let Some(provider_name) = provider_name {
        entries.push(("providerName".to_string(), string(provider_name)));
    }
    entries.push(("config".to_string(), config));
    object_from(entries)
}

fn model_rule(provider_id: &str, model_id: &str, context_window: i64) -> JsonValue {
    object_from(vec![
        ("type".to_string(), string("provider-model")),
        ("providerId".to_string(), string(provider_id)),
        ("modelId".to_string(), string(model_id)),
        (
            "config".to_string(),
            object_from(vec![(
                "properties".to_string(),
                object_from(vec![(
                    "contextWindow".to_string(),
                    number(context_window as f64),
                )]),
            )]),
        ),
    ])
}

fn upsert_provider(rules: &mut Vec<JsonValue>, rule: JsonValue, provider_id: &str) {
    if let Some(index) = rules.iter().position(|existing| {
        existing
            .as_object()
            .and_then(|existing| existing.get("providerId"))
            .and_then(JsonValue::as_str)
            == Some(provider_id)
    }) {
        rules[index] = rule;
    } else {
        rules.push(rule);
    }
}

fn upsert_model_rule(
    rules: &mut Vec<JsonValue>,
    replacement: JsonValue,
    provider_id: &str,
    model_id: &str,
) {
    let mut result = Vec::with_capacity(rules.len() + 1);
    let mut replaced = false;
    for rule in rules.drain(..) {
        let Some(rule_object) = rule.as_object() else {
            result.push(rule);
            continue;
        };
        let is_exact = matches!(
            rule_object.get("type").and_then(JsonValue::as_str),
            Some("provider-model" | "manual-provider-model")
        );
        let same_identity = rule_object.get("providerId").and_then(JsonValue::as_str)
            == Some(provider_id)
            && rule_object.get("modelId").and_then(JsonValue::as_str) == Some(model_id);
        if is_exact && same_identity {
            if !replaced {
                result.push(replacement.clone());
                replaced = true;
            }
            continue;
        }
        result.push(rule);
    }
    if !replaced {
        result.push(replacement);
    }
    *rules = result;
}

fn import_one(
    provider: &IndexMap<String, JsonValue>,
    provider_rules: &mut Vec<JsonValue>,
    model_rules: &mut Vec<JsonValue>,
) {
    let Some(provider_id) = non_empty(provider.get("id")) else {
        return;
    };
    let api_template_id = match provider_id.as_str() {
        "builtin:bigmodel" => Some(BIGMODEL_TEMPLATE_ID),
        "builtin:zai" => Some(ZAI_TEMPLATE_ID),
        _ => None,
    };
    if let Some(api_template_id) = api_template_id {
        let api_key = trimmed(provider.get("apiKey")).unwrap_or_default();
        if !api_key.is_empty() {
            let config = object_from(vec![
                ("group".to_string(), string("standard-personal")),
                (
                    "access".to_string(),
                    object_from(vec![
                        ("type".to_string(), string("api-key")),
                        ("apiKey".to_string(), string(&api_key)),
                    ]),
                ),
            ]);
            upsert_provider(
                provider_rules,
                provider_rule(api_template_id, Some(api_template_id), None, None, config),
                api_template_id,
            );
        }
        return;
    }
    if provider_id.starts_with("builtin:") || provider_id.starts_with("account:") {
        return;
    }
    if let Some(source) = provider.get("source").and_then(JsonValue::as_str) {
        if source != "custom" {
            return;
        }
    }
    let provider_name = non_empty(provider.get("name")).filter(|name| *name != provider_id);
    let rule = provider_rule(
        &provider_id,
        None,
        provider_name.as_deref(),
        read_bool(provider.get("enabled")),
        config_object(provider),
    );
    upsert_provider(provider_rules, rule, &provider_id);
    for (model_id, context_window) in model_members(provider) {
        let Some(context_window) = context_window else {
            continue;
        };
        upsert_model_rule(
            model_rules,
            model_rule(&provider_id, &model_id, context_window),
            &provider_id,
            &model_id,
        );
    }
}

/// `importLegacyPersonalProviderConfig`：返回 `{ providers, models }` 两层更新。
pub fn import_legacy_personal_provider_config(legacy_providers: &[JsonValue]) -> JsonValue {
    let mut providers = Vec::new();
    let mut models = Vec::new();
    for provider in legacy_providers {
        if let Some(provider) = provider.as_object() {
            import_one(provider, &mut providers, &mut models);
        }
    }
    object_from(vec![
        ("providers".to_string(), JsonValue::Array(providers)),
        ("models".to_string(), JsonValue::Array(models)),
    ])
}
