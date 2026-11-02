//! 旧 Provider 数据迁移：`mode.model-providers.v1` → v2、旧 `config.json` 读取与
//! Personal 层导入。
//!
//! 真源：`packages/services/src/model-provider/legacyModelProviderSerialized.ts`、
//! `legacyModeConfigProviderReader.ts`、`legacyPersonalProviderConfigImporter.ts`。
//! 规范见 `docs/specs/provider-legacy-migration.md`。

mod import;
mod migrate;
mod model;
mod parse;
mod reader;
mod types;

pub use import::import_legacy_personal_provider_config;
pub use migrate::{migrate_legacy_model_provider, normalize_model_provider_base_url_for_kind};
pub use model::{
    create_model_provider_model_config, default_endpoint_path,
    default_supported_formats_from_api_format, default_supported_formats_from_endpoints,
    get_model_provider_model_ids, map_api_format_to_kind,
    normalize_model_provider_configured_base_url, resolve_model_provider_api_format,
    resolve_model_provider_context_window, resolve_model_provider_default_kind,
    resolve_model_provider_kind_api_format, resolve_model_provider_runtime_base_url,
    strip_legacy_claude_provider_mappings, strip_model_provider_reasoning_patches,
    CreateModelConfigParams, EndpointsInput, ProviderShape,
    MODEL_PROVIDER_NEW_MODEL_CONTEXT_WINDOW,
};
pub use parse::{parse_legacy_model_provider_list, parse_legacy_model_provider_list_value};
pub use provider_config::JsonValue;
pub use reader::{
    read_mode_config_providers, read_mode_config_providers_from_str, LegacyConfigReaderEnv,
};
pub use types::{
    ApiFormat, LegacyEndpoints, LegacyModelProvider, Modality, ProviderKind, ProviderSource,
    SupportedFormat, SystemDisabledReason,
};

/// 解析旧 provider 列表并对每条执行迁移；任何一条不合法即整体失败。
pub fn migrate_legacy_model_provider_list(text: &str) -> Result<Vec<JsonValue>, String> {
    let providers = parse_legacy_model_provider_list(text)?;
    Ok(providers
        .iter()
        .map(migrate_legacy_model_provider)
        .collect())
}
