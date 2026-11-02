//! `legacyModelProviderListSchema` 的解析：宽进（忽略未知键）、严出（字段类型与枚举）。

use indexmap::IndexMap;
use provider_config::JsonValue;
use serde_json::Number;

use crate::types::{
    ApiFormat, LegacyEndpoints, LegacyModelProvider, ProviderSource, SupportedFormat,
    SystemDisabledReason,
};

fn object<'a>(value: &'a JsonValue, path: &str) -> Result<&'a IndexMap<String, JsonValue>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{path} must be an object"))
}

fn required<'a>(
    object: &'a IndexMap<String, JsonValue>,
    key: &str,
    path: &str,
) -> Result<&'a JsonValue, String> {
    object
        .get(key)
        .ok_or_else(|| format!("{path}.{key} is required"))
}

fn string(value: &JsonValue, path: &str) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{path} must be a string"))
}

fn optional_string(
    object: &IndexMap<String, JsonValue>,
    key: &str,
    path: &str,
) -> Result<Option<String>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => Ok(Some(string(value, &format!("{path}.{key}"))?)),
    }
}

fn optional_bool(
    object: &IndexMap<String, JsonValue>,
    key: &str,
    path: &str,
) -> Result<Option<bool>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{path}.{key} must be a boolean")),
    }
}

fn number(value: &JsonValue, path: &str) -> Result<Number, String> {
    value
        .as_number()
        .cloned()
        .ok_or_else(|| format!("{path} must be a number"))
}

fn string_map(value: &JsonValue, path: &str) -> Result<IndexMap<String, String>, String> {
    let object = object(value, path)?;
    let mut result = IndexMap::new();
    for (key, child) in object {
        let text = child
            .as_str()
            .ok_or_else(|| format!("{path}.{key} must be a string"))?;
        result.insert(key.clone(), text.to_string());
    }
    Ok(result)
}

fn optional_string_map(
    object: &IndexMap<String, JsonValue>,
    key: &str,
    path: &str,
) -> Result<Option<IndexMap<String, String>>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => Ok(Some(string_map(value, &format!("{path}.{key}"))?)),
    }
}

fn parse_endpoints(value: &JsonValue, path: &str) -> Result<LegacyEndpoints, String> {
    let object = object(value, path)?;
    let mut endpoints = LegacyEndpoints::default();
    for (key, target) in [
        ("anthropic", &mut endpoints.anthropic),
        ("openai", &mut endpoints.openai),
        ("gemini", &mut endpoints.gemini),
    ] {
        match object.get(key) {
            None => {}
            Some(child) => {
                *target = child
                    .as_str()
                    .ok_or_else(|| format!("{path}.{key} must be a string"))?
                    .to_string();
            }
        }
    }
    Ok(endpoints)
}

fn parse_supported_format(value: &JsonValue, path: &str) -> Result<SupportedFormat, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{path} must be a string"))?;
    SupportedFormat::parse(text).ok_or_else(|| format!("{path} is not a supported format"))
}

fn parse_model_supported_formats(
    value: &JsonValue,
    path: &str,
) -> Result<IndexMap<String, Vec<SupportedFormat>>, String> {
    let object = object(value, path)?;
    let mut result = IndexMap::new();
    for (key, child) in object {
        let array = child
            .as_array()
            .ok_or_else(|| format!("{path}.{key} must be an array"))?;
        let mut formats = Vec::with_capacity(array.len());
        for (index, item) in array.iter().enumerate() {
            formats.push(parse_supported_format(
                item,
                &format!("{path}.{key}[{index}]"),
            )?);
        }
        result.insert(key.clone(), formats);
    }
    Ok(result)
}

fn validate_claude_mapping(value: &JsonValue, path: &str) -> Result<(), String> {
    let object = object(value, path)?;
    for key in ["haiku", "sonnet", "opus", "reasoning"] {
        required(object, key, path).and_then(|child| string(child, &format!("{path}.{key}")))?;
    }
    Ok(())
}

fn parse_provider_mappings(
    value: &JsonValue,
    path: &str,
) -> Result<IndexMap<String, JsonValue>, String> {
    let object = object(value, path)?;
    if let Some(claude) = object.get("claude") {
        validate_claude_mapping(claude, &format!("{path}.claude"))?;
    }
    Ok(object.clone())
}

fn parse_optional_enum<T>(
    object: &IndexMap<String, JsonValue>,
    key: &str,
    path: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<T>, String> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| format!("{path}.{key} must be a string"))?;
    parse(text)
        .map(Some)
        .ok_or_else(|| format!("{path}.{key} is not supported"))
}

fn parse_provider(value: &JsonValue, path: &str) -> Result<LegacyModelProvider, String> {
    let object = object(value, path)?;
    let models = match object.get("models") {
        None => Vec::new(),
        Some(value) => {
            let array = value
                .as_array()
                .ok_or_else(|| format!("{path}.models must be an array"))?;
            let mut models = Vec::with_capacity(array.len());
            for (index, item) in array.iter().enumerate() {
                models.push(string(item, &format!("{path}.models[{index}]"))?);
            }
            models
        }
    };
    let model_supported_formats = match object.get("modelSupportedFormats") {
        None => None,
        Some(value) => Some(parse_model_supported_formats(
            value,
            &format!("{path}.modelSupportedFormats"),
        )?),
    };
    let provider_mappings = match object.get("providerMappings") {
        None => None,
        Some(value) => Some(parse_provider_mappings(
            value,
            &format!("{path}.providerMappings"),
        )?),
    };
    Ok(LegacyModelProvider {
        id: string(required(object, "id", path)?, &format!("{path}.id"))?,
        name: string(required(object, "name", path)?, &format!("{path}.name"))?,
        enabled: optional_bool(object, "enabled", path)?,
        system_disabled_reason: parse_optional_enum(
            object,
            "systemDisabledReason",
            path,
            SystemDisabledReason::parse,
        )?,
        endpoints: parse_endpoints(
            required(object, "endpoints", path)?,
            &format!("{path}.endpoints"),
        )?,
        api_format: parse_optional_enum(object, "apiFormat", path, ApiFormat::parse)?,
        source: parse_optional_enum(object, "source", path, ProviderSource::parse)?,
        models_dev_provider_id: optional_string(object, "modelsDevProviderId", path)?,
        api_key_required: optional_bool(object, "apiKeyRequired", path)?,
        headers: optional_string_map(object, "headers", path)?,
        logo_url: optional_string(object, "logoUrl", path)?,
        api_key: string(required(object, "apiKey", path)?, &format!("{path}.apiKey"))?,
        api_key_url: optional_string(object, "apiKeyUrl", path)?,
        models,
        model_display_names: optional_string_map(object, "modelDisplayNames", path)?,
        model_supported_formats,
        provider_mappings,
        created_at: number(
            required(object, "createdAt", path)?,
            &format!("{path}.createdAt"),
        )?,
        updated_at: number(
            required(object, "updatedAt", path)?,
            &format!("{path}.updatedAt"),
        )?,
    })
}

/// 解析 `legacyModelProviderListSchema`（JSON 文本；整体不合法时返回错误）。
pub fn parse_legacy_model_provider_list(text: &str) -> Result<Vec<LegacyModelProvider>, String> {
    let value = JsonValue::from_json_str(text).map_err(|error| error.to_string())?;
    parse_legacy_model_provider_list_value(&value)
}

pub fn parse_legacy_model_provider_list_value(
    value: &JsonValue,
) -> Result<Vec<LegacyModelProvider>, String> {
    let array = value
        .as_array()
        .ok_or_else(|| "provider list must be an array".to_string())?;
    array
        .iter()
        .enumerate()
        .map(|(index, item)| parse_provider(item, &format!("[{index}]")))
        .collect()
}
