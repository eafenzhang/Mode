//! Personal Provider Config 只读层。
//!
//! 真源：`packages/provider-node/src/provider-config-file-codec.ts`、
//! `packages/provider-node/src/personal-provider-config-repository.ts`。
//!
//! revision 不是原始文件字节的哈希，而是 `JSON.stringify(encodeProviderConfigFile(update))`
//! 的 sha256；读取时先解码再按规范形状计算。

use std::path::Path;

use provider_config::{
    parse_manual_model_config, parse_model_config, parse_personal_model_config_rules,
    parse_personal_provider_config_map, Field, JsonValue, ManualModelConfig, ModelConfig,
    ModelConfigRules, ModelInputFormatConfig, ModelOptionSpecsConfig, ModelPropertiesConfig,
    ModelRule, ProviderConfigMap,
};
use serde_json::Number;

use crate::resolve::sha256_hex;
use crate::SourceError;

const CURRENT_SCHEMA_VERSION: i64 = 1;
const MODE_BUILTIN_RETIRED_ZAPI_PROVIDER_ID: &str = "builtin:zapi";

/// `modelSelectionSchema.options`（存在即为 `Some`，即使为空对象）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelectionOptions {
    pub reasoning_level: Option<String>,
}

/// `modelSelectionSchema` 的解析结果（字段已 trim）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
    pub options: Option<ModelSelectionOptions>,
}

impl ModelSelection {
    fn parse(value: &JsonValue) -> Result<Self, SourceError> {
        let obj = value
            .as_object()
            .ok_or_else(|| SourceError::invalid("defaultModelSelection 必须是对象"))?;
        reject_unknown_keys(
            obj.keys(),
            &["providerId", "modelId", "options"],
            "defaultModelSelection",
        )?;
        let provider_id = parse_trimmed_non_empty(
            obj.get("providerId")
                .ok_or_else(|| SourceError::invalid("defaultModelSelection.providerId 缺失"))?,
            "defaultModelSelection.providerId",
        )?;
        let model_id = parse_trimmed_non_empty(
            obj.get("modelId")
                .ok_or_else(|| SourceError::invalid("defaultModelSelection.modelId 缺失"))?,
            "defaultModelSelection.modelId",
        )?;
        let options = match obj.get("options") {
            None => None,
            Some(options) => {
                let options = options.as_object().ok_or_else(|| {
                    SourceError::invalid("defaultModelSelection.options 必须是对象")
                })?;
                reject_unknown_keys(
                    options.keys(),
                    &["reasoningLevel"],
                    "defaultModelSelection.options",
                )?;
                let reasoning_level = match options.get("reasoningLevel") {
                    None => None,
                    Some(value) => Some(parse_trimmed_non_empty(
                        value,
                        "defaultModelSelection.options.reasoningLevel",
                    )?),
                };
                Some(ModelSelectionOptions { reasoning_level })
            }
        };
        Ok(Self {
            provider_id,
            model_id,
            options,
        })
    }

    fn to_json(&self) -> JsonValue {
        let mut entries = vec![
            (
                "providerId".to_owned(),
                JsonValue::String(self.provider_id.clone()),
            ),
            (
                "modelId".to_owned(),
                JsonValue::String(self.model_id.clone()),
            ),
        ];
        if let Some(options) = &self.options {
            let mut option_entries = Vec::new();
            if let Some(reasoning_level) = &options.reasoning_level {
                option_entries.push((
                    "reasoningLevel".to_owned(),
                    JsonValue::String(reasoning_level.clone()),
                ));
            }
            entries.push(("options".to_owned(), JsonValue::object_from(option_entries)));
        }
        JsonValue::object_from(entries)
    }
}

/// Personal 层快照：对应 `ProviderConfigLayerSnapshot`（无 providerTemplates）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalLayer {
    pub revision: String,
    pub providers: ProviderConfigMap,
    pub models: ModelConfigRules,
    pub provider_order: Option<Vec<String>>,
    pub default_model_selection: Option<ModelSelection>,
}

impl PersonalLayer {
    /// 规范编码；revision 由该文本的 sha256 得到。
    fn encode(&self) -> JsonValue {
        let mut config = Vec::new();
        if let Some(order) = &self.provider_order {
            config.push((
                "providerOrder".to_owned(),
                JsonValue::Array(order.iter().cloned().map(JsonValue::String).collect()),
            ));
        }
        config.push((
            "providerConfigRules".to_owned(),
            JsonValue::object_from([("providerRules".to_owned(), self.providers.to_json())]),
        ));
        config.push((
            "modelConfigRules".to_owned(),
            personal_model_rules_json(&self.models),
        ));
        if let Some(selection) = &self.default_model_selection {
            config.push(("defaultModelSelection".to_owned(), selection.to_json()));
        }
        JsonValue::object_from([
            (
                "schemaVersion".to_owned(),
                JsonValue::Number(Number::from(CURRENT_SCHEMA_VERSION)),
            ),
            ("config".to_owned(), JsonValue::object_from(config)),
        ])
    }

    fn compute_revision(&self) -> String {
        sha256_hex(self.encode().to_json_string().as_bytes())
    }

    /// 与 TS 快照字面量一致的 JSON。
    pub fn to_json(&self) -> JsonValue {
        let mut entries = vec![
            (
                "revision".to_owned(),
                JsonValue::String(self.revision.clone()),
            ),
            ("providers".to_owned(), self.providers.to_json()),
            ("models".to_owned(), self.models.to_json()),
        ];
        if let Some(order) = &self.provider_order {
            entries.push((
                "providerOrder".to_owned(),
                JsonValue::Array(order.iter().cloned().map(JsonValue::String).collect()),
            ));
        }
        if let Some(selection) = &self.default_model_selection {
            entries.push(("defaultModelSelection".to_owned(), selection.to_json()));
        }
        JsonValue::object_from(entries)
    }

    pub fn to_json_string(&self) -> String {
        self.to_json().to_json_string()
    }
}

/// `NodePersonalProviderConfigRepository.read()` 的只读等价：任何读取/解析失败都降级为空层。
pub fn read_personal_layer(path: impl AsRef<Path>) -> PersonalLayer {
    let Ok(bytes) = std::fs::read(path.as_ref()) else {
        return empty_personal_layer();
    };
    // Node `readFile(..., "utf8")` 对非法序列使用 U+FFFD 替换，不是直接失败。
    let text = String::from_utf8_lossy(&bytes);
    let Ok(value) = JsonValue::from_json_str(&text) else {
        return empty_personal_layer();
    };
    decode_personal_document(&value).unwrap_or_else(|_| empty_personal_layer())
}

/// 严格解码 Personal 文件文档；错误不会降级。
pub fn decode_personal_document(value: &JsonValue) -> Result<PersonalLayer, SourceError> {
    let version = read_schema_version(value)?;
    if version > CURRENT_SCHEMA_VERSION {
        return Err(SourceError::invalid(format!(
            "Provider Config schemaVersion {version} 高于当前支持的 {CURRENT_SCHEMA_VERSION}"
        )));
    }
    if version < CURRENT_SCHEMA_VERSION {
        return Err(SourceError::invalid(format!(
            "缺少 Provider Config schemaVersion {version} 到 {} 的迁移器",
            version + 1
        )));
    }

    let obj = value
        .as_object()
        .ok_or_else(|| SourceError::invalid("Provider Config 必须是对象"))?;
    reject_unknown_keys(obj.keys(), &["schemaVersion", "config"], "Provider Config")?;
    let config_value = obj
        .get("config")
        .ok_or_else(|| SourceError::invalid("Provider Config.config 缺失"))?;
    let config = config_value
        .as_object()
        .ok_or_else(|| SourceError::invalid("Provider Config.config 必须是对象"))?;
    reject_unknown_keys(
        config.keys(),
        &[
            "providerOrder",
            "providerConfigRules",
            "modelConfigRules",
            "defaultModelSelection",
        ],
        "Provider Config.config",
    )?;

    let provider_order = match config.get("providerOrder") {
        None => None,
        Some(value) => Some(parse_provider_order(value)?),
    };
    let provider_rules = config
        .get("providerConfigRules")
        .ok_or_else(|| SourceError::invalid("Provider Config.config.providerConfigRules 缺失"))?;
    let model_rules = config
        .get("modelConfigRules")
        .ok_or_else(|| SourceError::invalid("Provider Config.config.modelConfigRules 缺失"))?;
    let default_model_selection = match config.get("defaultModelSelection") {
        None => None,
        Some(value) => Some(ModelSelection::parse(value)?),
    };

    let providers = parse_personal_provider_config_map(provider_rules, &[])
        .map_err(|error| SourceError::invalid(format!("providerConfigRules 不合法: {error:?}")))?;
    let normalized_model_rules = normalize_legacy_manual_rules(model_rules);
    let models = parse_personal_model_config_rules(&normalized_model_rules, &[])
        .map_err(|error| SourceError::invalid(format!("modelConfigRules 不合法: {error:?}")))?;

    let mut layer = PersonalLayer {
        revision: String::new(),
        providers,
        models,
        provider_order,
        default_model_selection,
    };
    layer.revision = layer.compute_revision();
    Ok(layer)
}

/// 空层：`emptyUpdate()` 的规范编码 + sha256。
pub fn empty_personal_layer() -> PersonalLayer {
    let mut layer = PersonalLayer {
        revision: String::new(),
        providers: ProviderConfigMap::empty(),
        models: ModelConfigRules::empty(),
        provider_order: Some(Vec::new()),
        default_model_selection: None,
    };
    layer.revision = layer.compute_revision();
    layer
}

/// `toPersonalJSON()`：zod 会在编码边界按 `personalModelConfigRulesSchema` 重新解析，
/// 因此 manual 规则的 config 采用 **manual schema 的字段顺序**（不是 ModelConfig 顺序）。
/// `provider_config::ModelConfigRules::to_personal_json` 只做结构投影、不重排 manual
/// config；这里按 TS 行为单独构造，保持 provider-config 语义不变。
fn personal_model_rules_json(models: &ModelConfigRules) -> JsonValue {
    let mut provider_rules = Vec::new();
    let mut manual_rules = Vec::new();
    for rule in models.rules() {
        match rule {
            ModelRule::Provider {
                provider_id,
                model_id,
                config,
            } => provider_rules.push(JsonValue::object_from([
                ("modelId".to_owned(), JsonValue::String(model_id.clone())),
                ("config".to_owned(), config.to_json()),
                (
                    "providerId".to_owned(),
                    JsonValue::String(provider_id.clone()),
                ),
            ])),
            ModelRule::Manual {
                provider_id,
                model_id,
                config,
            } => manual_rules.push(JsonValue::object_from([
                ("modelId".to_owned(), JsonValue::String(model_id.clone())),
                (
                    "config".to_owned(),
                    ManualModelConfig {
                        config: config.clone(),
                    }
                    .to_json(),
                ),
                (
                    "providerId".to_owned(),
                    JsonValue::String(provider_id.clone()),
                ),
            ])),
            _ => {}
        }
    }
    JsonValue::object_from([
        (
            "providerModelRules".to_owned(),
            JsonValue::Array(provider_rules),
        ),
        (
            "manualProviderModelRules".to_owned(),
            JsonValue::Array(manual_rules),
        ),
    ])
}

fn read_schema_version(value: &JsonValue) -> Result<i64, SourceError> {
    let Some(obj) = value.as_object() else {
        return Err(SourceError::invalid("Provider Config 缺少 schemaVersion"));
    };
    let Some(version) = obj.get("schemaVersion") else {
        return Err(SourceError::invalid("Provider Config 缺少 schemaVersion"));
    };
    let version = js_integer(version)
        .ok_or_else(|| SourceError::invalid("Provider Config schemaVersion 必须是非负整数"))?;
    if version < 0 {
        return Err(SourceError::invalid(
            "Provider Config schemaVersion 必须是非负整数",
        ));
    }
    Ok(version)
}

fn parse_provider_order(value: &JsonValue) -> Result<Vec<String>, SourceError> {
    let values = value
        .as_array()
        .ok_or_else(|| SourceError::invalid("providerOrder 必须是字符串数组"))?;
    let mut order = Vec::with_capacity(values.len());
    for entry in values {
        let text = entry
            .as_str()
            .ok_or_else(|| SourceError::invalid("providerOrder 必须是字符串数组"))?;
        if text.is_empty() {
            return Err(SourceError::invalid("providerOrder 每项长度必须 ≥ 1"));
        }
        order.push(text.to_owned());
    }
    Ok(order)
}

fn parse_trimmed_non_empty(value: &JsonValue, what: &str) -> Result<String, SourceError> {
    let text = value
        .as_str()
        .ok_or_else(|| SourceError::invalid(format!("{what} 必须是字符串")))?;
    let trimmed = js_trim(text);
    if trimmed.is_empty() {
        return Err(SourceError::invalid(format!("{what} 不能为空")));
    }
    Ok(trimmed.to_owned())
}

/// JS `String.prototype.trim()`（含 U+FEFF）。
fn js_trim(value: &str) -> &str {
    value.trim_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}')
}

fn reject_unknown_keys<'a>(
    keys: impl Iterator<Item = &'a String>,
    allowed: &[&str],
    what: &str,
) -> Result<(), SourceError> {
    if let Some(key) = keys
        .into_iter()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(SourceError::invalid(format!("{what} 含未知字段: {key}")));
    }
    Ok(())
}

/// `Number.isInteger` 口径；超出 i64 的整数值饱和到 i64 边界（仅用于版本比较）。
fn js_integer(value: &JsonValue) -> Option<i64> {
    let JsonValue::Number(number) = value else {
        return None;
    };
    if let Some(value) = number.as_i64() {
        return Some(value);
    }
    if let Some(value) = number.as_u64() {
        return Some(i64::try_from(value).unwrap_or(i64::MAX));
    }
    let value = number.as_f64()?;
    if !value.is_finite() || value.fract() != 0.0 {
        return None;
    }
    if value > i64::MAX as f64 {
        Some(i64::MAX)
    } else if value < i64::MIN as f64 {
        Some(i64::MIN)
    } else {
        Some(value as i64)
    }
}

// ---------------------------------------------------------------------------
// Legacy manual 归一
// ---------------------------------------------------------------------------

fn normalize_legacy_manual_rules(input: &JsonValue) -> JsonValue {
    let Some(obj) = input.as_object() else {
        return input.clone();
    };
    let Some(JsonValue::Array(rules)) = obj.get("manualProviderModelRules") else {
        return input.clone();
    };
    let normalized = rules
        .iter()
        .map(normalize_legacy_manual_rule)
        .collect::<Vec<_>>();
    let mut cloned = input.clone();
    if let JsonValue::Object(map) = &mut cloned {
        map.insert(
            "manualProviderModelRules".to_owned(),
            JsonValue::Array(normalized),
        );
    }
    cloned
}

fn normalize_legacy_manual_rule(rule: &JsonValue) -> JsonValue {
    let Some(obj) = rule.as_object() else {
        return rule.clone();
    };
    let Some(config) = obj.get("config") else {
        return rule.clone();
    };
    if parse_manual_model_config(config, &[]).is_ok() {
        return rule.clone();
    }
    let Some(extracted) = extract_legacy_manual_config(config) else {
        return rule.clone();
    };
    let mut cloned = rule.clone();
    if let JsonValue::Object(map) = &mut cloned {
        map.insert("config".to_owned(), extracted);
    }
    cloned
}

fn extract_legacy_manual_config(config: &JsonValue) -> Option<JsonValue> {
    if let Some(model) = legacy_complete_model(config) {
        return Some(ManualModelConfig { config: model }.to_json());
    }
    if let Some(model) = legacy_editable_model(config) {
        return Some(ManualModelConfig { config: model }.to_json());
    }
    None
}

/// `legacyCompleteManualSchema`：完整 complete 形状，`enabled` 放宽为 nullable/optional。
fn legacy_complete_model(config: &JsonValue) -> Option<ModelConfig> {
    // Sparse 解析负责顶层/嵌套未知字段与类型；complete 校验负责必填与非空。
    let parsed = parse_model_config(config, &[]).ok()?;
    let properties = parsed.properties.value()?.clone();
    if !properties.validate_complete().is_empty() {
        return None;
    }
    let option_specs = parsed.option_specs.value()?.clone();
    if !option_specs.validate_complete().is_empty() {
        return None;
    }
    let input_format = properties.input_format.value()?.clone();
    let manual_properties = ModelPropertiesConfig {
        requires_mfjs_tool_schema: Field::Absent,
        context_window: properties.context_window.clone(),
        input_format: Field::Value(ModelInputFormatConfig {
            supports_text: Field::Absent,
            supports_image: input_format.supports_image.clone(),
            supports_video: input_format.supports_video.clone(),
            supports_audio: Field::Absent,
            supports_pdf: input_format.supports_pdf.clone(),
        }),
        output_format: Field::Absent,
        supports_tool_call: Field::Absent,
        supports_json_schema_output: properties.supports_json_schema_output.clone(),
        supports_native_web_search: properties.supports_native_web_search.clone(),
        supports_mid_conversation_system: properties.supports_mid_conversation_system.clone(),
    };
    let reasoning_level = option_specs.reasoning_level.value()?.clone();
    let max = option_specs.max_output_tokens.value()?.max.clone();
    let manual_option_specs = ModelOptionSpecsConfig {
        reasoning_level: Field::Value(reasoning_level),
        max_output_tokens: Field::Value(provider_config::LimitOptionSpecConfig {
            max,
            map: Field::Absent,
        }),
    };
    Some(ModelConfig {
        enabled: parsed.enabled,
        properties: Field::Value(manual_properties),
        option_specs: Field::Value(manual_option_specs),
    })
}

/// `legacyEditableManualSchema`：manual 形状 + `properties.requiresMfjsToolSchema` 必填 boolean。
fn legacy_editable_model(config: &JsonValue) -> Option<ModelConfig> {
    let obj = config.as_object()?;
    if obj
        .keys()
        .any(|key| !["enabled", "properties", "optionSpecs"].contains(&key.as_str()))
    {
        return None;
    }
    let properties = obj.get("properties")?.as_object()?;
    if properties.keys().any(|key| {
        ![
            "contextWindow",
            "supportsJsonSchemaOutput",
            "supportsNativeWebSearch",
            "supportsMidConversationSystem",
            "inputFormat",
            "requiresMfjsToolSchema",
        ]
        .contains(&key.as_str())
    }) {
        return None;
    }
    match properties.get("requiresMfjsToolSchema")? {
        JsonValue::Bool(_) => {}
        _ => return None,
    }

    let mut manual_properties = (*properties).clone();
    manual_properties.shift_remove("requiresMfjsToolSchema");
    let mut manual = (*obj).clone();
    manual.insert(
        "properties".to_owned(),
        JsonValue::Object(manual_properties),
    );
    parse_manual_model_config(&JsonValue::Object(manual), &[])
        .ok()
        .map(ManualModelConfig::into_model)
}

/// `retired-zhipu-provider-migration` 的只读守卫：Release 中不允许已退出的 Provider。
pub(crate) fn reject_retired_provider(providers: &ProviderConfigMap) -> Result<(), SourceError> {
    if providers
        .get(MODE_BUILTIN_RETIRED_ZAPI_PROVIDER_ID)
        .is_some()
    {
        return Err(SourceError::invalid(format!(
            "Mode Built-in Release 包含已退出的 Provider: {MODE_BUILTIN_RETIRED_ZAPI_PROVIDER_ID}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_REVISION: &str = "5bc65dc77f66a74f6bac0df7929af0b78c79685322a9f637f00d979c4d5d14e2";

    fn parse(text: &str) -> JsonValue {
        JsonValue::from_json_str(text).expect("合法 JSON")
    }

    #[test]
    fn empty_layer_matches_ts_revision() {
        let layer = empty_personal_layer();
        assert_eq!(layer.revision, EMPTY_REVISION);
        assert_eq!(layer.provider_order, Some(Vec::new()));
        assert_eq!(
            layer.to_json_string(),
            format!(
                "{{\"revision\":\"{EMPTY_REVISION}\",\"providers\":[],\"models\":{{\"modelRules\":[],\"modelApiRules\":[],\"providerSiteRules\":[],\"templateModelRules\":[],\"builtinProviderModelRules\":[],\"manualProviderModelRules\":[]}},\"providerOrder\":[]}}"
            )
        );
    }

    #[test]
    fn missing_file_recovers_to_empty_layer() {
        let path = std::env::temp_dir().join("provider-config-source-does-not-exist.json");
        assert_eq!(read_personal_layer(path), empty_personal_layer());
    }

    #[test]
    fn corrupt_json_recovers_to_empty_layer() {
        let dir = std::env::temp_dir().join("pcs-unit-corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("provider_config.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(read_personal_layer(&path), empty_personal_layer());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn low_and_high_schema_versions_recover_to_empty_layer() {
        for version in ["0", "2", "-1", "1.5", "\"1\""] {
            let text = format!(
                "{{\"schemaVersion\":{version},\"config\":{{\"providerConfigRules\":{{\"providerRules\":[]}},\"modelConfigRules\":{{\"providerModelRules\":[],\"manualProviderModelRules\":[]}}}}}}"
            );
            let value = parse(&text);
            assert!(
                decode_personal_document(&value).is_err(),
                "version {version}"
            );
        }
    }

    #[test]
    fn provider_order_and_selection_round_trip_with_trim() {
        let value = parse(
            r#"{"schemaVersion":1,"config":{"providerOrder":["p2","p1"],"providerConfigRules":{"providerRules":[]},"modelConfigRules":{"providerModelRules":[],"manualProviderModelRules":[]},"defaultModelSelection":{"providerId":"  p1 ","modelId":" m1 ","options":{"reasoningLevel":" high "}}}}"#,
        );
        let layer = decode_personal_document(&value).unwrap();
        assert_eq!(layer.provider_order, Some(vec!["p2".into(), "p1".into()]));
        let selection = layer.default_model_selection.as_ref().unwrap();
        assert_eq!(selection.provider_id, "p1");
        assert_eq!(selection.model_id, "m1");
        assert_eq!(
            selection
                .options
                .as_ref()
                .and_then(|options| options.reasoning_level.as_deref()),
            Some("high")
        );
        assert!(layer.to_json_string().contains("\"defaultModelSelection\":{\"providerId\":\"p1\",\"modelId\":\"m1\",\"options\":{\"reasoningLevel\":\"high\"}}"));
    }

    fn legacy_complete_config() -> JsonValue {
        parse(
            r#"{
              "enabled": true,
              "properties": {
                "requiresMfjsToolSchema": false,
                "contextWindow": 128000,
                "inputFormat": {"supportsText": true, "supportsImage": true, "supportsVideo": false, "supportsAudio": false, "supportsPdf": true},
                "outputFormat": {"supportsText": true},
                "supportsToolCall": true,
                "supportsJsonSchemaOutput": true,
                "supportsNativeWebSearch": false,
                "supportsMidConversationSystem": true
              },
              "optionSpecs": {
                "reasoningLevel": {"values": ["low", "high"], "map": "{\"effort\": reasoningLevel}"},
                "maxOutputTokens": {"max": 8192, "map": "{\"tokens\": maxOutputTokens}"}
              }
            }"#,
        )
    }

    fn legacy_editable_config() -> JsonValue {
        parse(
            r#"{
              "enabled": true,
              "properties": {
                "requiresMfjsToolSchema": false,
                "contextWindow": 64000,
                "supportsJsonSchemaOutput": true,
                "supportsNativeWebSearch": false,
                "supportsMidConversationSystem": true,
                "inputFormat": {"supportsImage": true, "supportsVideo": false, "supportsPdf": true}
              },
              "optionSpecs": {
                "reasoningLevel": {"values": ["low", "high"], "map": "{\"effort\": reasoningLevel}"},
                "maxOutputTokens": {"max": 4096}
              }
            }"#,
        )
    }

    #[test]
    fn legacy_complete_manual_is_normalized() {
        let config = legacy_complete_config();
        let normalized = extract_legacy_manual_config(&config).expect("legacy complete 命中");
        assert_eq!(
            normalized.to_json_string(),
            r#"{"enabled":true,"properties":{"contextWindow":128000,"supportsJsonSchemaOutput":true,"supportsNativeWebSearch":false,"supportsMidConversationSystem":true,"inputFormat":{"supportsImage":true,"supportsVideo":false,"supportsPdf":true}},"optionSpecs":{"reasoningLevel":{"values":["low","high"],"map":"{\"effort\": reasoningLevel}"},"maxOutputTokens":{"max":8192}}}"#
        );
    }

    #[test]
    fn legacy_editable_manual_is_normalized() {
        let config = legacy_editable_config();
        let normalized = extract_legacy_manual_config(&config).expect("legacy editable 命中");
        assert_eq!(
            normalized.to_json_string(),
            r#"{"enabled":true,"properties":{"contextWindow":64000,"supportsJsonSchemaOutput":true,"supportsNativeWebSearch":false,"supportsMidConversationSystem":true,"inputFormat":{"supportsImage":true,"supportsVideo":false,"supportsPdf":true}},"optionSpecs":{"reasoningLevel":{"values":["low","high"],"map":"{\"effort\": reasoningLevel}"},"maxOutputTokens":{"max":4096}}}"#
        );
    }

    #[test]
    fn legacy_normalization_keeps_invalid_config_untouched() {
        let input = parse(
            r#"{"providerModelRules":[],"manualProviderModelRules":[{"providerId":"p","modelId":"m","config":{}}]}"#,
        );
        let normalized = normalize_legacy_manual_rules(&input);
        assert_eq!(normalized.to_json_string(), input.to_json_string());
        assert!(parse_personal_model_config_rules(&normalized, &[]).is_err());
    }

    #[test]
    fn retired_provider_is_rejected() {
        let value = parse(
            r#"{"providerRules":[{"providerId":"builtin:zapi","config":{"group":"standard-personal"}}]}"#,
        );
        let providers = parse_personal_provider_config_map(&value, &[]).unwrap();
        assert!(reject_retired_provider(&providers).is_err());
    }
}
