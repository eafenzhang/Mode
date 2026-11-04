//! 旧 `config.json` 文件的 IO/JSON/schema 错误边界。
//!
//! 真源：`packages/services/src/model-provider/legacyModeConfigProviderReader.ts` 的
//! `readRawModeConfigFile` / `readModeConfigProviders`。本模块只做文件读取、分阶段
//! 错误与已知字段形状校验；Provider 归一化仍由 [`crate::read_mode_config_providers`]
//! 负责。zod 嵌套模型字段/枚举成员的校验已按声明顺序补齐（2026-11-04），
//! 首个 issue 路径与 TS 逐字对齐。

use std::fmt;
use std::io::ErrorKind;
use std::path::Path;

use provider_config::JsonValue;

use crate::reader::{read_mode_config_providers, LegacyConfigReaderEnv};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyConfigFileError {
    Io { path: String, code: String },
    Json { path: String },
    Schema { path: String, field: String },
}

impl fmt::Display for LegacyConfigFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, code } => write!(
                formatter,
                "旧 Provider 配置读取失败 stage=io path={path} code={code}"
            ),
            Self::Json { path } => {
                write!(formatter, "旧 Provider 配置解析失败 stage=json path={path}")
            }
            Self::Schema { path, field } => write!(
                formatter,
                "旧 Provider 配置校验失败 stage=schema path={path} field={field}"
            ),
        }
    }
}

impl std::error::Error for LegacyConfigFileError {}

/// 读取并解析旧 `config.json`；文件不存在返回 `None`，未声明 `provider` 也返回 `None`。
pub fn read_legacy_mode_config_providers(
    path: &Path,
    env: &LegacyConfigReaderEnv<'_>,
) -> Result<Option<Vec<JsonValue>>, LegacyConfigFileError> {
    read_legacy_mode_config_providers_with(path, env, |path| std::fs::read(path))
}

pub fn read_legacy_mode_config_providers_with<F>(
    path: &Path,
    env: &LegacyConfigReaderEnv<'_>,
    read: F,
) -> Result<Option<Vec<JsonValue>>, LegacyConfigFileError>
where
    F: FnOnce(&Path) -> std::io::Result<Vec<u8>>,
{
    let path_text = path.to_string_lossy().to_string();
    let bytes = match read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(LegacyConfigFileError::Io {
                path: path_text,
                code: io_code(&error),
            })
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let parsed = JsonValue::from_json_str(&text).map_err(|_| LegacyConfigFileError::Json {
        path: path_text.clone(),
    })?;
    validate_config_file(&parsed).map_err(|field| LegacyConfigFileError::Schema {
        path: path_text.clone(),
        field,
    })?;
    let has_provider = parsed
        .as_object()
        .map(|root| root.contains_key("provider"))
        .unwrap_or(false);
    if !has_provider {
        return Ok(None);
    }
    let providers =
        read_mode_config_providers(&parsed, env).map_err(|_| LegacyConfigFileError::Schema {
            path: path_text,
            field: String::new(),
        })?;
    Ok(Some(providers))
}

fn io_code(error: &std::io::Error) -> String {
    match error.kind() {
        ErrorKind::PermissionDenied => "EACCES".to_string(),
        _ => error
            .raw_os_error()
            .map(|code| code.to_string())
            .unwrap_or_else(|| format!("{:?}", error.kind())),
    }
}

/// 已知字段形状校验；返回首个非法字段路径（根不是对象时为空串）。
fn validate_config_file(value: &JsonValue) -> Result<(), String> {
    let Some(root) = value.as_object() else {
        return Err(String::new());
    };
    if let Some(schema) = root.get("$schema") {
        if schema.as_str().is_none() {
            return Err("$schema".to_string());
        }
    }
    let Some(provider) = root.get("provider") else {
        return Ok(());
    };
    let Some(providers) = provider.as_object() else {
        return Err("provider".to_string());
    };
    for (provider_id, value) in providers {
        let field = format!("provider.{provider_id}");
        validate_provider(value, &field)?;
    }
    Ok(())
}

const PROVIDER_KINDS: &[&str] = &["anthropic", "openai", "openai-compatible"];
const API_FORMATS: &[&str] = &[
    "anthropic-messages",
    "openai-chat-completions",
    "openai-responses",
];
const PROVIDER_SOURCES: &[&str] = &["builtin", "models-dev", "custom", "workspace"];
const CATALOG_SOURCE_IDS: &[&str] = &["china-llm-mode-dev"];
const SYSTEM_DISABLED_REASONS: &[&str] = &[
    "coding_plan_not_authenticated",
    "coding_plan_not_connected",
    "coding_plan_auth_failed",
    "coding_plan_not_entitled",
    "oauth_provider_inactive",
];
const MODALITIES: &[&str] = &["text", "image", "video", "audio", "pdf"];

fn check_string(
    object: &indexmap::IndexMap<String, JsonValue>,
    key: &str,
    field: &str,
) -> Result<(), String> {
    if let Some(value) = object.get(key) {
        if value.as_str().is_none() {
            return Err(format!("{field}.{key}"));
        }
    }
    Ok(())
}

fn check_bool(
    object: &indexmap::IndexMap<String, JsonValue>,
    key: &str,
    field: &str,
) -> Result<(), String> {
    if let Some(value) = object.get(key) {
        if value.as_bool().is_none() {
            return Err(format!("{field}.{key}"));
        }
    }
    Ok(())
}

fn check_number(
    object: &indexmap::IndexMap<String, JsonValue>,
    key: &str,
    field: &str,
) -> Result<(), String> {
    if let Some(value) = object.get(key) {
        if value.as_number().is_none() {
            return Err(format!("{field}.{key}"));
        }
    }
    Ok(())
}

fn check_positive_number(
    object: &indexmap::IndexMap<String, JsonValue>,
    key: &str,
    field: &str,
) -> Result<(), String> {
    if let Some(value) = object.get(key) {
        let positive = value
            .as_number()
            .and_then(|number| number.as_f64())
            .map(|number| number.is_finite() && number > 0.0)
            .unwrap_or(false);
        if !positive {
            return Err(format!("{field}.{key}"));
        }
    }
    Ok(())
}

fn check_object(
    object: &indexmap::IndexMap<String, JsonValue>,
    key: &str,
    field: &str,
) -> Result<(), String> {
    if let Some(value) = object.get(key) {
        if value.as_object().is_none() {
            return Err(format!("{field}.{key}"));
        }
    }
    Ok(())
}

fn check_enum(
    object: &indexmap::IndexMap<String, JsonValue>,
    key: &str,
    field: &str,
    allowed: &[&str],
) -> Result<(), String> {
    if let Some(value) = object.get(key) {
        let valid = value
            .as_str()
            .map(|text| allowed.contains(&text))
            .unwrap_or(false);
        if !valid {
            return Err(format!("{field}.{key}"));
        }
    }
    Ok(())
}

fn check_string_array(
    object: &indexmap::IndexMap<String, JsonValue>,
    key: &str,
    field: &str,
) -> Result<(), String> {
    if let Some(value) = object.get(key) {
        let Some(items) = value.as_array() else {
            return Err(format!("{field}.{key}"));
        };
        for (index, item) in items.iter().enumerate() {
            if item.as_str().is_none() {
                return Err(format!("{field}.{key}.{index}"));
            }
        }
    }
    Ok(())
}

fn check_enum_array(
    object: &indexmap::IndexMap<String, JsonValue>,
    key: &str,
    field: &str,
    allowed: &[&str],
) -> Result<(), String> {
    if let Some(value) = object.get(key) {
        let Some(items) = value.as_array() else {
            return Err(format!("{field}.{key}"));
        };
        for (index, item) in items.iter().enumerate() {
            let valid = item
                .as_str()
                .map(|text| allowed.contains(&text))
                .unwrap_or(false);
            if !valid {
                return Err(format!("{field}.{key}.{index}"));
            }
        }
    }
    Ok(())
}

fn check_string_record(
    object: &indexmap::IndexMap<String, JsonValue>,
    key: &str,
    field: &str,
) -> Result<(), String> {
    if let Some(value) = object.get(key) {
        let Some(record) = value.as_object() else {
            return Err(format!("{field}.{key}"));
        };
        for (name, item) in record {
            if item.as_str().is_none() {
                return Err(format!("{field}.{key}.{name}"));
            }
        }
    }
    Ok(())
}

/// zod object.passthrough() 语义：非对象报自身路径；已知键按声明顺序校验。
fn validate_provider(value: &JsonValue, field: &str) -> Result<(), String> {
    let Some(provider) = value.as_object() else {
        return Err(field.to_string());
    };
    // 顺序 = modeOpenCodeProviderConfigSchema 的声明顺序（首个 issue 逐字对齐）。
    check_string(provider, "api", field)?;
    check_string(provider, "name", field)?;
    check_string_array(provider, "env", field)?;
    check_string(provider, "id", field)?;
    check_enum(provider, "kind", field, PROVIDER_KINDS)?;
    check_string(provider, "npm", field)?;
    check_string_array(provider, "whitelist", field)?;
    check_string_array(provider, "blacklist", field)?;
    check_object(provider, "options", field)?;
    check_bool(provider, "enabled", field)?;
    check_enum(
        provider,
        "systemDisabledReason",
        field,
        SYSTEM_DISABLED_REASONS,
    )?;
    if let Some(endpoints) = provider.get("endpoints") {
        validate_endpoints(endpoints, &format!("{field}.endpoints"))?;
    }
    check_enum(provider, "apiFormat", field, API_FORMATS)?;
    check_enum(provider, "source", field, PROVIDER_SOURCES)?;
    check_enum(provider, "catalogSourceId", field, CATALOG_SOURCE_IDS)?;
    check_string(provider, "catalogProviderId", field)?;
    check_string(provider, "modelsDevProviderId", field)?;
    check_bool(provider, "apiKeyRequired", field)?;
    check_string_record(provider, "headers", field)?;
    check_string(provider, "logoUrl", field)?;
    check_string(provider, "apiKeyUrl", field)?;
    check_enum(provider, "defaultKind", field, PROVIDER_KINDS)?;
    check_object(provider, "providerMappings", field)?;
    check_number(provider, "createdAt", field)?;
    check_number(provider, "updatedAt", field)?;
    if let Some(models) = provider.get("models") {
        let Some(models) = models.as_object() else {
            return Err(format!("{field}.models"));
        };
        for (model_id, model) in models {
            validate_model(model, &format!("{field}.models.{model_id}"))?;
        }
    }
    if let Some(mode) = provider.get("mode") {
        validate_provider_mode(mode, &format!("{field}.mode"))?;
    }
    Ok(())
}

fn validate_model(value: &JsonValue, field: &str) -> Result<(), String> {
    let Some(model) = value.as_object() else {
        return Err(field.to_string());
    };
    // 顺序 = modeOpenCodeModelConfigSchema 的声明顺序。
    check_string(model, "id", field)?;
    check_string(model, "name", field)?;
    if let Some(reasoning) = model.get("reasoning") {
        // z.union([boolean, object])：任一分支失败时 issue 落在 union 自身路径。
        if reasoning.as_bool().is_none() && !is_valid_reasoning_extension(reasoning) {
            return Err(format!("{field}.reasoning"));
        }
    }
    if let Some(limit) = model.get("limit") {
        let limit_field = format!("{field}.limit");
        let Some(limit) = limit.as_object() else {
            return Err(limit_field);
        };
        for key in ["context", "input", "output"] {
            check_positive_number(limit, key, &limit_field)?;
        }
    }
    check_positive_number(model, "contextWindow", field)?;
    check_positive_number(model, "maxOutputTokens", field)?;
    if let Some(modalities) = model.get("modalities") {
        validate_modalities(modalities, &format!("{field}.modalities"))?;
    }
    check_object(model, "options", field)?;
    check_string_record(model, "headers", field)?;
    if let Some(variants) = model.get("variants") {
        let variants_field = format!("{field}.variants");
        let Some(variants) = variants.as_object() else {
            return Err(variants_field);
        };
        for (name, item) in variants {
            if item.as_object().is_none() {
                return Err(format!("{variants_field}.{name}"));
            }
        }
    }
    check_enum_array(model, "kinds", field, PROVIDER_KINDS)?;
    check_enum(model, "defaultKind", field, PROVIDER_KINDS)?;
    if let Some(map) = model.get("modelIdByKind") {
        validate_kind_string_map(map, &format!("{field}.modelIdByKind"))?;
    }
    check_string(model, "disabledReason", field)?;
    check_bool(model, "supportsTools", field)?;
    check_bool(model, "supportsStructuredOutput", field)?;
    if let Some(spec) = model.get("reasoningSpec") {
        validate_reasoning_spec(spec, &format!("{field}.reasoningSpec"))?;
    }
    check_bool(model, "hasMaxOutputTokens", field)?;
    check_number(model, "priority", field)?;
    check_bool(model, "modified", field)?;
    check_bool(model, "deleted", field)?;
    if let Some(mode) = model.get("mode") {
        validate_model_mode(mode, &format!("{field}.mode"))?;
    }
    Ok(())
}

fn is_valid_reasoning_extension(value: &JsonValue) -> bool {
    let Some(extension) = value.as_object() else {
        return false;
    };
    let bool_ok = |key: &str| {
        extension
            .get(key)
            .map(|item| item.as_bool().is_some())
            .unwrap_or(true)
    };
    let string_ok = |key: &str| {
        extension
            .get(key)
            .map(|item| item.as_str().is_some())
            .unwrap_or(true)
    };
    let mut valid = bool_ok("enabled") && string_ok("defaultVariant");
    if let Some(variants) = extension.get("variants") {
        let Some(items) = variants.as_array() else {
            return false;
        };
        valid = valid && items.iter().all(|item| item.as_str().is_some());
    }
    if let Some(aliases) = extension.get("aliases") {
        let Some(record) = aliases.as_object() else {
            return false;
        };
        valid = valid && record.values().all(|item| item.as_str().is_some());
    }
    valid
}

fn validate_modalities(value: &JsonValue, field: &str) -> Result<(), String> {
    let Some(modalities) = value.as_object() else {
        return Err(field.to_string());
    };
    for key in ["input", "output"] {
        if let Some(items) = modalities.get(key) {
            let Some(items) = items.as_array() else {
                return Err(format!("{field}.{key}"));
            };
            for (index, item) in items.iter().enumerate() {
                let valid = item
                    .as_str()
                    .map(|text| MODALITIES.contains(&text))
                    .unwrap_or(false);
                if !valid {
                    return Err(format!("{field}.{key}.{index}"));
                }
            }
        }
    }
    Ok(())
}

fn validate_kind_string_map(value: &JsonValue, field: &str) -> Result<(), String> {
    let Some(record) = value.as_object() else {
        return Err(field.to_string());
    };
    for (kind, item) in record {
        if !PROVIDER_KINDS.contains(&kind.as_str()) {
            // z.partialRecord 对枚举外键报错（路径到键本身）。
            return Err(format!("{field}.{kind}"));
        }
        let valid = item.as_str().map(|text| !text.is_empty()).unwrap_or(false);
        if !valid {
            return Err(format!("{field}.{kind}"));
        }
    }
    Ok(())
}

fn validate_endpoints(value: &JsonValue, field: &str) -> Result<(), String> {
    let Some(endpoints) = value.as_object() else {
        return Err(field.to_string());
    };
    for key in ["anthropic", "openai", "gemini", "baseURL"] {
        check_string(endpoints, key, field)?;
    }
    if let Some(paths) = endpoints.get("paths") {
        let paths_field = format!("{field}.paths");
        let Some(paths) = paths.as_object() else {
            return Err(paths_field);
        };
        for (kind, item) in paths {
            if !PROVIDER_KINDS.contains(&kind.as_str()) || item.as_str().is_none() {
                return Err(format!("{paths_field}.{kind}"));
            }
        }
    }
    Ok(())
}

fn validate_provider_mode(value: &JsonValue, field: &str) -> Result<(), String> {
    let Some(mode) = value.as_object() else {
        return Err(field.to_string());
    };
    check_bool(mode, "enabled", field)?;
    check_enum(mode, "systemDisabledReason", field, SYSTEM_DISABLED_REASONS)?;
    if let Some(endpoints) = mode.get("endpoints") {
        validate_endpoints(endpoints, &format!("{field}.endpoints"))?;
    }
    check_enum(mode, "apiFormat", field, API_FORMATS)?;
    check_enum(mode, "source", field, PROVIDER_SOURCES)?;
    check_enum(mode, "catalogSourceId", field, CATALOG_SOURCE_IDS)?;
    check_string(mode, "catalogProviderId", field)?;
    check_string(mode, "modelsDevProviderId", field)?;
    check_bool(mode, "apiKeyRequired", field)?;
    check_string_record(mode, "headers", field)?;
    check_string(mode, "logoUrl", field)?;
    check_string(mode, "apiKeyUrl", field)?;
    check_enum(mode, "defaultKind", field, PROVIDER_KINDS)?;
    check_object(mode, "providerMappings", field)?;
    check_number(mode, "createdAt", field)?;
    check_number(mode, "updatedAt", field)?;
    if let Some(deleted) = mode.get("deletedModels") {
        let Some(items) = deleted.as_array() else {
            return Err(format!("{field}.deletedModels"));
        };
        for (index, item) in items.iter().enumerate() {
            let valid = item.as_str().map(|text| !text.is_empty()).unwrap_or(false);
            if !valid {
                return Err(format!("{field}.deletedModels.{index}"));
            }
        }
    }
    Ok(())
}

fn validate_model_mode(value: &JsonValue, field: &str) -> Result<(), String> {
    let Some(mode) = value.as_object() else {
        return Err(field.to_string());
    };
    check_enum_array(mode, "kinds", field, PROVIDER_KINDS)?;
    check_enum(mode, "defaultKind", field, PROVIDER_KINDS)?;
    if let Some(map) = mode.get("modelIdByKind") {
        validate_kind_string_map(map, &format!("{field}.modelIdByKind"))?;
    }
    check_string(mode, "disabledReason", field)?;
    check_bool(mode, "supportsTools", field)?;
    check_bool(mode, "supportsStructuredOutput", field)?;
    if let Some(spec) = mode.get("reasoning") {
        validate_reasoning_spec(spec, &format!("{field}.reasoning"))?;
    }
    check_bool(mode, "hasMaxOutputTokens", field)?;
    check_number(mode, "priority", field)?;
    check_bool(mode, "modified", field)?;
    check_bool(mode, "deleted", field)?;
    Ok(())
}

fn validate_reasoning_spec(value: &JsonValue, field: &str) -> Result<(), String> {
    let Some(spec) = value.as_object() else {
        return Err(field.to_string());
    };
    if let Some(default_level) = spec.get("defaultLevel") {
        let valid = default_level
            .as_str()
            .map(|text| !text.is_empty())
            .unwrap_or(false);
        if !valid {
            return Err(format!("{field}.defaultLevel"));
        }
    }
    let Some(levels) = spec.get("levels") else {
        return Err(format!("{field}.levels"));
    };
    let Some(levels) = levels.as_object() else {
        return Err(format!("{field}.levels"));
    };
    for (level, patches) in levels {
        if level.is_empty() {
            return Err(format!("{field}.levels.{level}"));
        }
        let level_field = format!("{field}.levels.{level}");
        let Some(patches) = patches.as_object() else {
            return Err(level_field);
        };
        for (kind, patch) in patches {
            if !PROVIDER_KINDS.contains(&kind.as_str()) {
                return Err(format!("{level_field}.{kind}"));
            }
            validate_options_patch(patch, &format!("{level_field}.{kind}"))?;
        }
    }
    Ok(())
}

fn validate_options_patch(value: &JsonValue, field: &str) -> Result<(), String> {
    let Some(patch) = value.as_object() else {
        return Err(field.to_string());
    };
    for operation in ["set", "unset"] {
        if let Some(items) = patch.get(operation) {
            let operation_field = format!("{field}.{operation}");
            let Some(items) = items.as_array() else {
                return Err(operation_field);
            };
            for (index, item) in items.iter().enumerate() {
                let item_field = format!("{operation_field}.{index}");
                let Some(item) = item.as_object() else {
                    return Err(item_field);
                };
                let Some(path) = item.get("path") else {
                    return Err(format!("{item_field}.path"));
                };
                let Some(segments) = path.as_array() else {
                    return Err(format!("{item_field}.path"));
                };
                if segments.is_empty() {
                    return Err(format!("{item_field}.path"));
                }
                for (segment_index, segment) in segments.iter().enumerate() {
                    let valid = segment
                        .as_str()
                        .map(|text| !text.is_empty())
                        .unwrap_or(false);
                    if !valid {
                        return Err(format!("{item_field}.path.{segment_index}"));
                    }
                }
            }
        }
    }
    Ok(())
}
