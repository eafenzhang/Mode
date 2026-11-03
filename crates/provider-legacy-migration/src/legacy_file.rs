//! 旧 `config.json` 文件的 IO/JSON/schema 错误边界。
//!
//! 真源：`packages/services/src/model-provider/legacyModeConfigProviderReader.ts` 的
//! `readRawModeConfigFile` / `readModeConfigProviders`。本模块只做文件读取、分阶段
//! 错误与已知字段形状校验；Provider 归一化仍由 [`crate::read_mode_config_providers`]
//! 负责。完整 zod 嵌套模型字段的对拍留待后续切片。

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
        let Some(provider) = value.as_object() else {
            return Err(field);
        };
        validate_provider(provider, &field)?;
    }
    Ok(())
}

fn validate_provider(
    provider: &indexmap::IndexMap<String, JsonValue>,
    field: &str,
) -> Result<(), String> {
    for key in [
        "api",
        "name",
        "id",
        "npm",
        "kind",
        "systemDisabledReason",
        "apiFormat",
        "source",
        "catalogSourceId",
        "catalogProviderId",
        "modelsDevProviderId",
        "logoUrl",
        "apiKeyUrl",
        "defaultKind",
    ] {
        if let Some(value) = provider.get(key) {
            if value.as_str().is_none() {
                return Err(format!("{field}.{key}"));
            }
        }
    }
    for key in ["enabled", "apiKeyRequired"] {
        if let Some(value) = provider.get(key) {
            if value.as_bool().is_none() {
                return Err(format!("{field}.{key}"));
            }
        }
    }
    for key in ["createdAt", "updatedAt"] {
        if let Some(value) = provider.get(key) {
            if value.as_number().is_none() {
                return Err(format!("{field}.{key}"));
            }
        }
    }
    for key in ["env", "whitelist", "blacklist"] {
        if let Some(value) = provider.get(key) {
            let Some(items) = value.as_array() else {
                return Err(format!("{field}.{key}"));
            };
            if items.iter().any(|item| item.as_str().is_none()) {
                return Err(format!("{field}.{key}"));
            }
        }
    }
    for key in ["options", "providerMappings", "endpoints"] {
        if let Some(value) = provider.get(key) {
            if value.as_object().is_none() {
                return Err(format!("{field}.{key}"));
            }
        }
    }
    if let Some(headers) = provider.get("headers") {
        let Some(headers) = headers.as_object() else {
            return Err(format!("{field}.headers"));
        };
        for (name, value) in headers {
            if value.as_str().is_none() {
                return Err(format!("{field}.headers.{name}"));
            }
        }
    }
    if let Some(models) = provider.get("models") {
        let Some(models) = models.as_object() else {
            return Err(format!("{field}.models"));
        };
        for (model_id, value) in models {
            if value.as_object().is_none() {
                return Err(format!("{field}.models.{model_id}"));
            }
        }
    }
    if let Some(mode) = provider.get("mode") {
        let Some(mode) = mode.as_object() else {
            return Err(format!("{field}.mode"));
        };
        if let Some(enabled) = mode.get("enabled") {
            if enabled.as_bool().is_none() {
                return Err(format!("{field}.mode.enabled"));
            }
        }
        if let Some(headers) = mode.get("headers") {
            let Some(headers) = headers.as_object() else {
                return Err(format!("{field}.mode.headers"));
            };
            for (name, value) in headers {
                if value.as_str().is_none() {
                    return Err(format!("{field}.mode.headers.{name}"));
                }
            }
        }
        if let Some(models) = mode.get("deletedModels") {
            let Some(models) = models.as_array() else {
                return Err(format!("{field}.mode.deletedModels"));
            };
            if models
                .iter()
                .any(|item| item.as_str().map(|value| value.is_empty()).unwrap_or(true))
            {
                return Err(format!("{field}.mode.deletedModels"));
            }
        }
    }
    Ok(())
}
