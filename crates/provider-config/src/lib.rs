//! `@mode/provider` 纯配置模型的 Rust 移植。
//!
//! 只做解析、校验与保序序列化；不做文件 IO、网络、registry/resolver、
//! config-service、Account overlay 或 revision。规范见
//! `docs/specs/provider-config.md`。

mod account_state;
mod cel;
mod json;
mod model;
mod provider;
mod regex;
mod rules;
mod url;
mod value;

pub use account_state::{
    AccountProviderAvailability, AccountProviderState, AccountProviderStates,
    AccountProviderUnavailableReason,
};
pub use json::JsonValue;
pub use model::{
    parse_manual_model_config, parse_model_config, translate_issues, translate_issues_at,
    EnumOptionSpecConfig, LimitOptionSpecConfig, ManualModelConfig, ModelConfig,
    ModelInputFormatConfig, ModelOptionSpecsConfig, ModelOutputFormatConfig,
    ModelPropertiesConfig,
};
pub use provider::{
    parse_account_provider_config_map, parse_mode_builtin_provider_config_rules,
    parse_personal_provider_config_map, parse_provider_config, parse_provider_config_map,
    parse_provider_template_map, ApiKeyAccess, ApiKeyAccessType, ProviderAccess, ProviderApiConfig,
    ProviderApiType, ProviderConfig, ProviderConfigMap, ProviderConfigRule, ProviderGroup,
    ProviderLogo, ProviderTemplate, ProviderTemplateMap, ProviderTemplateNameMap,
    ProviderVisibility, ZhipuAccountAccess, ZhipuAccountMode, ZhipuAccountType,
};
pub use rules::{
    parse_mode_builtin_model_config_rules, parse_personal_model_config_rules, ModelConfigRules,
    ModelRule,
};
pub use value::{Field, Issue, ParseError, ParseResult};

/// 便捷入口：从 JSON 文本解析后再调用指定解析器。
pub fn parse_json_text(text: &str) -> Result<JsonValue, serde_json::Error> {
    JsonValue::from_json_str(text)
}
