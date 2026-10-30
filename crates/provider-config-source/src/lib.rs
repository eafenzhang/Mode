//! Provider 配置只读文件源。
//!
//! 组合 `@mode/provider-node` 的 Personal 文件与 Mode Built-in Release 读取路径，
//! 产出与 `ProviderConfigService.read()` 等价的组合快照。不做轮询、写盘、remote
//! 同步、registry/resolver 或 Account overlay。规范见
//! `docs/specs/provider-config-source.md`。

mod builtin;
mod personal;
mod resolve;

use std::fmt;

use provider_config::{JsonValue, ModelConfigRules, ProviderConfigMap, ProviderTemplateMap};

pub use builtin::{
    decode_mode_builtin_release, read_mode_builtin_layer, serialize_mode_builtin_release,
    BuiltinLayer, ModeBuiltinRelease,
};
pub use personal::{
    decode_personal_document, empty_personal_layer, read_personal_layer, ModelSelection,
    ModelSelectionOptions, PersonalLayer,
};
pub use resolve::{resolve_path, resolve_path_with_cwd, source_key_for_path, source_key_with_cwd};

/// 只读源错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// schema / 解码失败。
    Invalid(String),
    /// Bundled 与 Active 均不可用。
    Unavailable {
        bundled: Option<String>,
        active: Option<String>,
    },
}

impl SourceError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::Invalid(message) => write!(formatter, "{message}"),
            SourceError::Unavailable { bundled, active } => {
                write!(
                    formatter,
                    "Bundled 与 Active Mode Built-in Release 均不可用 (bundled: {}, active: {})",
                    bundled.as_deref().unwrap_or("missing"),
                    active.as_deref().unwrap_or("missing"),
                )
            }
        }
    }
}

impl std::error::Error for SourceError {}

/// `ProviderConfigService.read()` 的只读等价快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigSnapshot {
    pub revision: String,
    pub mode_builtin_revision: String,
    pub personal_revision: String,
    pub mode_builtin_providers: ProviderConfigMap,
    pub mode_builtin_provider_templates: ProviderTemplateMap,
    pub personal_providers: ProviderConfigMap,
    pub mode_builtin_model_rules: ModelConfigRules,
    pub personal_models: ModelConfigRules,
    pub personal_provider_order: Vec<String>,
}

impl ProviderConfigSnapshot {
    pub fn to_json(&self) -> JsonValue {
        JsonValue::object_from([
            (
                "revision".to_owned(),
                JsonValue::String(self.revision.clone()),
            ),
            (
                "modeBuiltinRevision".to_owned(),
                JsonValue::String(self.mode_builtin_revision.clone()),
            ),
            (
                "personalRevision".to_owned(),
                JsonValue::String(self.personal_revision.clone()),
            ),
            (
                "modeBuiltinProviders".to_owned(),
                self.mode_builtin_providers.to_json(),
            ),
            (
                "modeBuiltinProviderTemplates".to_owned(),
                self.mode_builtin_provider_templates.to_json(),
            ),
            (
                "personalProviders".to_owned(),
                self.personal_providers.to_json(),
            ),
            (
                "modeBuiltinModelRules".to_owned(),
                self.mode_builtin_model_rules.to_json(),
            ),
            ("personalModels".to_owned(), self.personal_models.to_json()),
            (
                "personalProviderOrder".to_owned(),
                JsonValue::Array(
                    self.personal_provider_order
                        .iter()
                        .cloned()
                        .map(JsonValue::String)
                        .collect(),
                ),
            ),
        ])
    }

    pub fn to_json_string(&self) -> String {
        self.to_json().to_json_string()
    }
}

/// 组合 Builtin 与 Personal 层。
pub fn read_snapshot(
    bundled_path: &str,
    personal_path: impl AsRef<std::path::Path>,
    active_path: Option<&str>,
) -> Result<ProviderConfigSnapshot, SourceError> {
    let builtin = read_mode_builtin_layer(bundled_path, active_path)?;
    let personal = read_personal_layer(personal_path);
    Ok(compose_snapshot(builtin, personal))
}

fn compose_snapshot(builtin: BuiltinLayer, personal: PersonalLayer) -> ProviderConfigSnapshot {
    // 与 TS `JSON.stringify([buildinRevision, personalRevision])` 逐字节一致。
    let revision = JsonValue::Array(vec![
        JsonValue::String(builtin.revision.clone()),
        JsonValue::String(personal.revision.clone()),
    ])
    .to_json_string();
    ProviderConfigSnapshot {
        revision,
        mode_builtin_revision: builtin.revision,
        personal_revision: personal.revision,
        mode_builtin_providers: builtin.providers,
        mode_builtin_provider_templates: builtin.provider_templates,
        personal_providers: personal.providers,
        mode_builtin_model_rules: builtin.models,
        personal_models: personal.models,
        personal_provider_order: personal.provider_order.unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_revision_is_json_array_of_layer_revisions() {
        let dir = std::env::temp_dir().join(format!("pcs-lib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bundled = dir.join("bundled.json");
        std::fs::write(
            &bundled,
            r#"{"schemaVersion":1,"revision":3,"config":{"providerConfigRules":{"templateRules":[],"providerRules":[]},"modelConfigRules":{"modelRules":[],"modelApiRules":[],"providerSiteRules":[],"templateModelRules":[],"builtinProviderModelRules":[]}}}"#,
        )
        .unwrap();
        let personal = dir.join("provider_config.json");
        let snapshot = read_snapshot(bundled.to_string_lossy().as_ref(), &personal, None).unwrap();
        assert_eq!(
            snapshot.revision,
            serde_json::to_string(&[
                snapshot.mode_builtin_revision.clone(),
                snapshot.personal_revision.clone()
            ])
            .unwrap()
        );
        assert_eq!(snapshot.personal_provider_order, Vec::<String>::new());
        std::fs::remove_dir_all(&dir).ok();
    }
}
