//! ProviderSettingsView 投影（provider-settings 阶段 1）。
//!
//! 规范见 `docs/specs/provider-settings-channel.md`。只做纯投影：快照 + resolver →
//! 与 TS `createProviderSettingsView` 同构的 JSON；写入面与频道在后续阶段。

use provider_config::{Field, Issue, ProviderGroup, ProviderVisibility};
use provider_config_source::ProviderConfigSnapshot;
use provider_resolver::{ProviderConfigResolver, ProviderConfigResolverInput};
use serde_json::{json, Map, Value as Json};

fn field_string(field: &Field<String>) -> String {
    match field {
        Field::Value(value) => value.clone(),
        _ => String::new(),
    }
}

fn is_hidden(config: &provider_config::ProviderConfig) -> bool {
    matches!(&config.visibility, Field::Value(ProviderVisibility::Hidden))
}

/// provider-config 的 `JsonValue` 与 serde_json 不是同一类型；按规范文本转换。
fn to_serde(value: &provider_config::JsonValue) -> Json {
    serde_json::from_str(&value.to_json_string()).unwrap_or(Json::Null)
}

fn issue_json(issue: &Issue) -> Json {
    json!({
        "code": issue.code,
        "path": issue.path,
        "message": issue.message,
    })
}

fn template_name_map_json(map: &provider_config::ProviderTemplateNameMap) -> Json {
    let mut object = Map::new();
    if let Some(value) = &map.zh_cn {
        object.insert("zh-CN".to_string(), json!(value));
    }
    if let Some(value) = &map.en_us {
        object.insert("en-US".to_string(), json!(value));
    }
    Json::Object(object)
}

/// TS `createProviderSettingsView`：只输出非 hidden provider；`accountState` 因官方
/// 账号体系下线暂不输出（运行时 acount states 为空）。
pub fn build_provider_settings_view(snapshot: &ProviderConfigSnapshot, revision: u64) -> Json {
    let account_providers = provider_config::ProviderConfigMap::empty();
    let resolution = ProviderConfigResolver.resolve(&ProviderConfigResolverInput {
        mode_builtin_providers: &snapshot.mode_builtin_providers,
        mode_builtin_provider_templates: Some(&snapshot.mode_builtin_provider_templates),
        personal_providers: &snapshot.personal_providers,
        mode_builtin_model_rules: &snapshot.mode_builtin_model_rules,
        personal_models: &snapshot.personal_models,
        account_providers: &account_providers,
        account_states: None,
        personal_provider_order: Some(&snapshot.personal_provider_order),
    });
    let executable_ids: std::collections::HashSet<&str> = resolution
        .registry_providers
        .iter()
        .map(|provider| provider.provider_id.as_str())
        .collect();

    let templates: Vec<Json> = snapshot
        .mode_builtin_provider_templates
        .values
        .iter()
        .map(|(template_id, template)| {
            json!({
                "templateId": template_id,
                "templateNameMap": template_name_map_json(&template.template_name_map),
                "config": to_serde(&template.config.to_json()),
            })
        })
        .collect();

    let visible: Vec<&provider_resolver::ResolvedProvider> = resolution
        .resolved_providers
        .iter()
        .filter(|provider| !is_hidden(&provider.config))
        .collect();
    let provider_order: Vec<Json> = visible
        .iter()
        .filter(|provider| {
            matches!(
                &provider.config.group,
                Field::Value(ProviderGroup::StandardPersonal)
            )
        })
        .map(|provider| json!(provider.provider_id))
        .collect();

    let providers: Vec<Json> = visible
        .iter()
        .map(|provider| {
            let mut value = Map::new();
            value.insert("providerId".to_string(), json!(provider.provider_id));
            value.insert(
                "providerName".to_string(),
                json!(field_string(&provider.provider_name)),
            );
            value.insert(
                "templateId".to_string(),
                json!(field_string(&provider.template_id)),
            );
            value.insert("enabled".to_string(), json!(provider.enabled));
            value.insert(
                "executable".to_string(),
                json!(executable_ids.contains(provider.provider_id.as_str())),
            );
            if let Some(template) = &provider.template_config {
                value.insert("templateConfig".to_string(), to_serde(&template.to_json()));
            }
            if let Some(builtin) = &provider.effective_builtin_config {
                value.insert(
                    "effectiveBuiltinConfig".to_string(),
                    to_serde(&builtin.to_json()),
                );
            }
            if let Some(personal) = snapshot.personal_providers.get(&provider.provider_id) {
                value.insert(
                    "personalConfig".to_string(),
                    to_serde(&personal.config.to_json()),
                );
            }
            value.insert(
                "effectiveConfig".to_string(),
                to_serde(&provider.config.to_json()),
            );
            value.insert(
                "issues".to_string(),
                Json::Array(provider.provider_issues.iter().map(issue_json).collect()),
            );
            value.insert(
                "models".to_string(),
                Json::Array(
                    provider
                        .models
                        .iter()
                        .map(|model| {
                            let mut model_value = Map::new();
                            model_value.insert("kind".to_string(), json!("candidate"));
                            model_value.insert("modelId".to_string(), json!(model.model_id));
                            model_value.insert(
                                "builtin".to_string(),
                                json!(matches!(
                                    model.source,
                                    provider_resolver::ProviderModelSource::Builtin
                                )),
                            );
                            model_value.insert(
                                "effectiveBuiltinConfig".to_string(),
                                to_serde(&model.effective_builtin_config.to_json()),
                            );
                            if let Some(exact) = snapshot
                                .personal_models
                                .get_exact(&provider.provider_id, &model.model_id)
                            {
                                model_value.insert(
                                    "personalExactConfig".to_string(),
                                    to_serde(&exact.to_json()),
                                );
                            }
                            if let Some((kind, _)) = snapshot
                                .personal_models
                                .get_exact_rule(&provider.provider_id, &model.model_id)
                            {
                                model_value.insert(
                                    "useRecommendedConfig".to_string(),
                                    json!(kind != provider_config::ExactModelRuleKind::Manual),
                                );
                            }
                            model_value.insert(
                                "effectiveConfig".to_string(),
                                to_serde(&model.config.to_json()),
                            );
                            model_value.insert("enabled".to_string(), json!(model.enabled));
                            model_value.insert("executable".to_string(), json!(model.executable));
                            model_value.insert("selectable".to_string(), json!(model.selectable));
                            model_value.insert(
                                "issues".to_string(),
                                Json::Array(model.issues.iter().map(issue_json).collect()),
                            );
                            Json::Object(model_value)
                        })
                        .collect(),
                ),
            );
            Json::Object(value)
        })
        .collect();

    json!({
        "revision": revision,
        "providerTemplates": templates,
        "providerOrder": provider_order,
        "providers": providers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use provider_config::{ModelConfigRules, ProviderConfigMap, ProviderTemplateMap};

    fn empty_snapshot() -> ProviderConfigSnapshot {
        ProviderConfigSnapshot {
            revision: "[]".to_string(),
            mode_builtin_revision: String::new(),
            personal_revision: String::new(),
            mode_builtin_providers: ProviderConfigMap::empty(),
            mode_builtin_provider_templates: ProviderTemplateMap::empty(),
            personal_providers: ProviderConfigMap::empty(),
            mode_builtin_model_rules: ModelConfigRules::empty(),
            personal_models: ModelConfigRules::empty(),
            personal_provider_order: Vec::new(),
        }
    }

    #[test]
    fn empty_snapshot_projects_empty_view() {
        let view = build_provider_settings_view(&empty_snapshot(), 7);
        assert_eq!(view["revision"], 7);
        assert_eq!(view["providerTemplates"], Json::Array(vec![]));
        assert_eq!(view["providerOrder"], Json::Array(vec![]));
        assert_eq!(view["providers"], Json::Array(vec![]));
    }

    #[test]
    fn builtin_templates_are_projected() {
        let bundled = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/provider/mode-builtin.json");
        let snapshot = provider_config_source::read_snapshot(
            &bundled.to_string_lossy(),
            std::env::temp_dir().join("provider-settings-missing-personal.json"),
            None,
        )
        .expect("snapshot");
        let view = build_provider_settings_view(&snapshot, 1);
        let templates = view["providerTemplates"].as_array().expect("templates");
        assert!(!templates.is_empty());
        let first = &templates[0];
        assert!(first["templateId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()));
        assert!(first["config"].is_object());
    }
}
