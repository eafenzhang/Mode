//! Personal Provider Config（schemaVersion=1）文件 encode/decode。
//!
//! 真源：`packages/provider-node/src/provider-config-file-codec.ts`。
//! 只做纯 JSON 编解码；文件读取、原子写与锁由调用方负责。

use model_selection::{parse_model_selection, ModelSelection};
use provider_config::{
    parse_manual_model_config, parse_personal_model_config_rules,
    parse_personal_provider_config_map, JsonValue, ModelConfigRules, ProviderConfigMap,
};

pub const CURRENT_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigLayerUpdate {
    pub providers: ProviderConfigMap,
    pub models: ModelConfigRules,
    pub provider_order: Option<Vec<String>>,
    pub default_model_selection: Option<ModelSelection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderConfigCodecError {
    UnsupportedVersion {
        message: String,
        version: Option<i64>,
    },
    Invalid(String),
}

impl std::fmt::Display for ProviderConfigCodecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion { message, .. } => formatter.write_str(message),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ProviderConfigCodecError {}

fn invalid(message: impl Into<String>) -> ProviderConfigCodecError {
    ProviderConfigCodecError::Invalid(message.into())
}

fn unsupported(message: impl Into<String>, version: Option<i64>) -> ProviderConfigCodecError {
    ProviderConfigCodecError::UnsupportedVersion {
        message: message.into(),
        version,
    }
}

fn object(value: &JsonValue) -> Option<&indexmap::IndexMap<String, JsonValue>> {
    value.as_object()
}

fn read_schema_version(input: &JsonValue) -> Result<i64, ProviderConfigCodecError> {
    let Some(object) = object(input) else {
        return Err(unsupported("Provider Config 缺少 schemaVersion", None));
    };
    let Some(version) = object.get("schemaVersion") else {
        return Err(unsupported("Provider Config 缺少 schemaVersion", None));
    };
    let Some(number) = version.as_number().and_then(|number| number.as_f64()) else {
        return Err(unsupported(
            "Provider Config schemaVersion 必须是非负整数",
            None,
        ));
    };
    if !number.is_finite() || number.fract() != 0.0 || number < 0.0 {
        return Err(unsupported(
            "Provider Config schemaVersion 必须是非负整数",
            None,
        ));
    }
    let version = number as i64;
    if version > CURRENT_SCHEMA_VERSION {
        return Err(unsupported(
            format!(
                "Provider Config schemaVersion {version} 高于当前支持的 {CURRENT_SCHEMA_VERSION}"
            ),
            Some(version),
        ));
    }
    if version < CURRENT_SCHEMA_VERSION {
        return Err(unsupported(
            format!(
                "缺少 Provider Config schemaVersion {version} 到 {} 的迁移器",
                version + 1
            ),
            Some(version),
        ));
    }
    Ok(version)
}

fn normalize_legacy_manual_rules(input: &JsonValue) -> JsonValue {
    let Some(map) = object(input) else {
        return input.clone();
    };
    let Some(rules) = map
        .get("manualProviderModelRules")
        .and_then(JsonValue::as_array)
    else {
        return input.clone();
    };
    let mut normalized = Vec::with_capacity(rules.len());
    for rule in rules {
        let Some(rule_object) = object(rule) else {
            normalized.push(rule.clone());
            continue;
        };
        let Some(config) = rule_object.get("config") else {
            normalized.push(rule.clone());
            continue;
        };
        if parse_manual_model_config(config, &[]).is_ok() {
            normalized.push(rule.clone());
            continue;
        }
        let Some(extracted) = extract_legacy_manual_config(config) else {
            normalized.push(rule.clone());
            continue;
        };
        if parse_manual_model_config(&extracted, &[]).is_ok() {
            let mut next = rule_object.clone();
            next.insert("config".to_string(), extracted);
            normalized.push(JsonValue::Object(next));
        } else {
            normalized.push(rule.clone());
        }
    }
    let mut next = map.clone();
    next.insert(
        "manualProviderModelRules".to_string(),
        JsonValue::Array(normalized),
    );
    JsonValue::Object(next)
}

fn keys_are_subset(object: &indexmap::IndexMap<String, JsonValue>, allowed: &[&str]) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

fn extract_legacy_manual_config(config: &JsonValue) -> Option<JsonValue> {
    let config = object(config)?;
    if !keys_are_subset(config, &["enabled", "properties", "optionSpecs"]) {
        return None;
    }
    let properties = object(config.get("properties")?)?;
    if !keys_are_subset(
        properties,
        &[
            "requiresMfjsToolSchema",
            "contextWindow",
            "inputFormat",
            "outputFormat",
            "supportsToolCall",
            "supportsJsonSchemaOutput",
            "supportsNativeWebSearch",
            "supportsMidConversationSystem",
        ],
    ) {
        return None;
    }
    let option_specs = object(config.get("optionSpecs")?)?;
    if !keys_are_subset(option_specs, &["reasoningLevel", "maxOutputTokens"]) {
        return None;
    }
    if let Some(input_format) = properties.get("inputFormat").and_then(JsonValue::as_object) {
        if !keys_are_subset(
            input_format,
            &[
                "supportsText",
                "supportsImage",
                "supportsVideo",
                "supportsAudio",
                "supportsPdf",
            ],
        ) {
            return None;
        }
    }
    if let Some(output_format) = properties
        .get("outputFormat")
        .and_then(JsonValue::as_object)
    {
        if !keys_are_subset(output_format, &["supportsText"]) {
            return None;
        }
    }
    let reasoning = option_specs.get("reasoningLevel")?.as_object()?;
    if !keys_are_subset(reasoning, &["values", "map"]) {
        return None;
    }
    let max_output = option_specs.get("maxOutputTokens")?.as_object()?;
    if !keys_are_subset(max_output, &["max", "map"]) {
        return None;
    }

    let mut next_properties = indexmap::IndexMap::new();
    for key in [
        "contextWindow",
        "supportsJsonSchemaOutput",
        "supportsNativeWebSearch",
        "supportsMidConversationSystem",
    ] {
        if let Some(value) = properties.get(key) {
            next_properties.insert(key.to_string(), value.clone());
        }
    }
    if let Some(input_format) = properties.get("inputFormat").and_then(JsonValue::as_object) {
        let mut picked = indexmap::IndexMap::new();
        for key in ["supportsImage", "supportsVideo", "supportsPdf"] {
            if let Some(value) = input_format.get(key) {
                picked.insert(key.to_string(), value.clone());
            }
        }
        next_properties.insert("inputFormat".to_string(), JsonValue::Object(picked));
    }

    let mut next_option_specs = indexmap::IndexMap::new();
    if let Some(reasoning) = option_specs.get("reasoningLevel") {
        next_option_specs.insert("reasoningLevel".to_string(), reasoning.clone());
    }
    let mut next_max = indexmap::IndexMap::new();
    if let Some(max) = max_output.get("max") {
        next_max.insert("max".to_string(), max.clone());
    }
    next_option_specs.insert("maxOutputTokens".to_string(), JsonValue::Object(next_max));

    let mut entries = Vec::new();
    if let Some(enabled) = config.get("enabled") {
        entries.push(("enabled".to_string(), enabled.clone()));
    }
    entries.push(("properties".to_string(), JsonValue::Object(next_properties)));
    entries.push((
        "optionSpecs".to_string(),
        JsonValue::Object(next_option_specs),
    ));
    Some(JsonValue::object_from(entries))
}

fn parse_selection(
    value: Option<&JsonValue>,
) -> Result<Option<ModelSelection>, ProviderConfigCodecError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let raw = value.to_json_string();
    parse_model_selection(&raw)
        .map(Some)
        .ok_or_else(|| invalid("Provider Config defaultModelSelection 无效"))
}

fn parse_provider_order(
    value: Option<&JsonValue>,
) -> Result<Option<Vec<String>>, ProviderConfigCodecError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(values) = value.as_array() else {
        return Err(invalid("Provider Config providerOrder 必须是数组"));
    };
    let mut order = Vec::with_capacity(values.len());
    for item in values {
        let Some(text) = item.as_str() else {
            return Err(invalid("Provider Config providerOrder 项必须是字符串"));
        };
        if text.is_empty() {
            return Err(invalid("Provider Config providerOrder 项不能为空"));
        }
        order.push(text.to_string());
    }
    Ok(Some(order))
}

pub fn decode_provider_config_file(
    input: &JsonValue,
) -> Result<ProviderConfigLayerUpdate, ProviderConfigCodecError> {
    let version = read_schema_version(input)?;
    debug_assert_eq!(version, CURRENT_SCHEMA_VERSION);
    let root = object(input).expect("version 已确认对象");
    if !keys_are_subset(root, &["schemaVersion", "config"]) {
        return Err(invalid("Provider Config 包含未知顶层字段"));
    }
    let config = root
        .get("config")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| invalid("Provider Config config 必须是对象"))?;
    if !keys_are_subset(
        config,
        &[
            "providerOrder",
            "providerConfigRules",
            "modelConfigRules",
            "defaultModelSelection",
        ],
    ) {
        return Err(invalid("Provider Config config 包含未知字段"));
    }
    let provider_rules = config
        .get("providerConfigRules")
        .ok_or_else(|| invalid("Provider Config 缺少 providerConfigRules"))?;
    let model_rules = config
        .get("modelConfigRules")
        .ok_or_else(|| invalid("Provider Config 缺少 modelConfigRules"))?;
    let providers = parse_personal_provider_config_map(
        provider_rules,
        &["config".to_string(), "providerConfigRules".to_string()],
    )
    .map_err(|error| invalid(format!("providerConfigRules: {error:?}")))?;
    let models = parse_personal_model_config_rules(
        &normalize_legacy_manual_rules(model_rules),
        &["config".to_string(), "modelConfigRules".to_string()],
    )
    .map_err(|error| invalid(format!("modelConfigRules: {error:?}")))?;
    Ok(ProviderConfigLayerUpdate {
        providers,
        models,
        provider_order: parse_provider_order(config.get("providerOrder"))?,
        default_model_selection: parse_selection(config.get("defaultModelSelection"))?,
    })
}

pub fn encode_provider_config_file(update: &ProviderConfigLayerUpdate) -> JsonValue {
    let mut config = indexmap::IndexMap::new();
    if let Some(provider_order) = &update.provider_order {
        config.insert(
            "providerOrder".to_string(),
            JsonValue::Array(
                provider_order
                    .iter()
                    .map(|id| JsonValue::String(id.clone()))
                    .collect(),
            ),
        );
    }
    config.insert(
        "providerConfigRules".to_string(),
        JsonValue::object_from(vec![(
            "providerRules".to_string(),
            update.providers.to_json(),
        )]),
    );
    config.insert(
        "modelConfigRules".to_string(),
        update.models.to_personal_json(),
    );
    if let Some(selection) = &update.default_model_selection {
        if let Ok(value) = serde_json::to_value(selection) {
            if let Ok(ordered) = JsonValue::from_json_str(&value.to_string()) {
                config.insert("defaultModelSelection".to_string(), ordered);
            }
        }
    }
    JsonValue::object_from(vec![
        (
            "schemaVersion".to_string(),
            JsonValue::Number(serde_json::Number::from(CURRENT_SCHEMA_VERSION)),
        ),
        ("config".to_string(), JsonValue::Object(config)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_version() {
        let error = decode_provider_config_file(&JsonValue::object_from(Vec::new())).unwrap_err();
        assert_eq!(
            error,
            unsupported("Provider Config 缺少 schemaVersion", None)
        );
    }

    #[test]
    fn rejects_zero_version_without_migration() {
        let input = JsonValue::from_json_str(r#"{"schemaVersion":0,"config":{}}"#).unwrap();
        let error = decode_provider_config_file(&input).unwrap_err();
        assert_eq!(
            error,
            unsupported(
                "缺少 Provider Config schemaVersion 0 到 1 的迁移器",
                Some(0)
            )
        );
    }
}
