//! ProviderSettingsView 投影（provider-settings 阶段 1）。
//!
//! 规范见 `docs/specs/provider-settings-channel.md`。只做纯投影：快照 + resolver →
//! 与 TS `createProviderSettingsView` 同构的 JSON；写入面与频道在后续阶段。

use provider_config::{Field, Issue, ProviderGroup, ProviderVisibility};
use provider_config_file_codec::ProviderConfigLayerUpdate;
use provider_config_source::PersonalLayer;
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

pub struct CreatePersonalProviderInput {
    pub template_id: Option<String>,
    pub provider_name: Option<String>,
    /// 默认 `en-US`；与 TS `resolveProviderTemplateName` 同口径。
    pub locale: Option<String>,
    pub initial_config: Option<provider_config::ProviderConfig>,
}

pub struct CreatedPersonalProvider {
    pub provider_id: String,
    pub update: ProviderConfigLayerUpdate,
}

fn normalize_provider_id_seed(value: &str) -> String {
    let seed: String = value
        .trim()
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = seed.trim_matches('-');
    if trimmed.is_empty() {
        "new-provider".to_string()
    } else {
        trimmed.to_string()
    }
}

fn next_personal_provider_id(
    occupied: &std::collections::HashSet<String>,
    template_id: Option<&str>,
) -> String {
    let base = template_id
        .map(normalize_provider_id_seed)
        .unwrap_or_else(|| "new-provider".to_string());
    if !occupied.contains(&base) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !occupied.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

fn next_personal_provider_label(seed: &str, labels: &[String]) -> String {
    let base = if seed.trim().is_empty() {
        "new-provider".to_string()
    } else {
        seed.trim().to_string()
    };
    let existing: std::collections::HashSet<String> =
        labels.iter().map(|label| label.to_lowercase()).collect();
    if !existing.contains(&base.to_lowercase()) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base} {suffix}");
        if !existing.contains(&candidate.to_lowercase()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn template_name(template: &provider_config::ProviderTemplate, locale: Option<&str>) -> String {
    let localized = match locale {
        Some("zh-CN") => template.template_name_map.zh_cn.as_deref(),
        _ => None,
    };
    localized
        .or(template.template_name_map.en_us.as_deref())
        .or(template.template_name_map.zh_cn.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&template.template_id)
        .to_string()
}

/// TS `createPersonalProvider`：模板实例或空 Personal Provider。
pub fn create_personal_provider(
    snapshot: &ProviderConfigSnapshot,
    current: &PersonalLayer,
    input: CreatePersonalProviderInput,
) -> Result<CreatedPersonalProvider, String> {
    let template_id = input
        .template_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let template = match &template_id {
        Some(template_id) => Some(
            snapshot
                .mode_builtin_provider_templates
                .get(template_id)
                .ok_or_else(|| format!("Provider Template 不存在: {template_id}"))?,
        ),
        None => None,
    };
    let initial = input.initial_config.unwrap_or_default();
    if !initial.group.is_absent() {
        return Err("initialConfig 不能包含 group".to_string());
    }
    if !initial.builtin_model_ids.is_absent() {
        return Err("initialConfig 不能包含 builtinModelIds".to_string());
    }
    let mut occupied: std::collections::HashSet<String> =
        snapshot.mode_builtin_providers.keys().into_iter().collect();
    occupied.extend(current.providers.keys());
    let provider_id = next_personal_provider_id(&occupied, template_id.as_deref());
    let label_seed = input
        .provider_name
        .clone()
        .unwrap_or_else(|| match &template {
            Some(template) => template_name(template, input.locale.as_deref()),
            None => "new-provider".to_string(),
        });
    let existing_labels: Vec<String> = snapshot
        .mode_builtin_providers
        .rules()
        .chain(current.providers.rules())
        .filter_map(|rule| match &rule.provider_name {
            Field::Value(value) if !value.trim().is_empty() => Some(value.clone()),
            _ => None,
        })
        .collect();
    let label = next_personal_provider_label(&label_seed, &existing_labels);

    let access = if template.is_some() {
        Field::Absent
    } else {
        Field::Value(provider_config::ProviderAccess::ApiKey(
            provider_config::ApiKeyAccess {
                access_type: provider_config::ApiKeyAccessType::ApiKey,
                api_key: Field::Absent,
                api_key_management_url: Field::Absent,
            },
        ))
    };
    let base = provider_config::ProviderConfig {
        group: Field::Value(ProviderGroup::StandardPersonal),
        access,
        personal_model_ids: Field::Value(Vec::new()),
        model_order: Field::Value(Vec::new()),
        ..provider_config::ProviderConfig::default()
    };
    let config = base.overlay(&initial);
    let rule = provider_config::ProviderConfigRule {
        provider_id: provider_id.clone(),
        template_id: template_id
            .as_deref()
            .map(|value| Field::Value(value.to_string()))
            .unwrap_or(Field::Absent),
        provider_name: Field::Value(label),
        enabled: Field::Absent,
        config,
    };
    let providers = current.providers.set_rule(rule);
    let personal_ids: Vec<String> = providers
        .entries()
        .filter(|(_, rule)| {
            matches!(
                &rule.config.group,
                Field::Value(ProviderGroup::StandardPersonal)
            )
        })
        .map(|(provider_id, _)| provider_id.clone())
        .collect();
    let requested: Vec<String> = current
        .provider_order
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|candidate| candidate != &provider_id)
        .chain(std::iter::once(provider_id.clone()))
        .collect();
    let provider_order = provider_resolver::resolve_owned_order(&[], &personal_ids, &requested);
    Ok(CreatedPersonalProvider {
        provider_id,
        update: ProviderConfigLayerUpdate {
            providers,
            models: current.models.clone(),
            provider_order: Some(provider_order),
            default_model_selection: current
                .default_model_selection
                .as_ref()
                .map(to_layer_selection),
        },
    })
}

pub struct ProviderModelMembershipRef<'a> {
    pub provider_id: &'a str,
    pub inherited_model_ids: &'a [String],
    pub personal_revision: &'a str,
}

pub struct SaveOverlayMetadata {
    pub template_id: Option<String>,
    pub enabled: Option<bool>,
    pub provider_name: Option<String>,
}

fn unique_strings(values: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            result.push(value.clone());
        }
    }
    result
}

fn field_builtin_model_ids(config: Option<&provider_config::ProviderConfig>) -> Vec<String> {
    match config.and_then(|config| match &config.builtin_model_ids {
        Field::Value(values) => Some(values.clone()),
        _ => None,
    }) {
        Some(values) => unique_strings(&values),
        None => Vec::new(),
    }
}

/// TS `normalizePersonalProviderMembership`：personalModelIds 去掉 builtin 成员，
/// modelOrder 按 builtin/personal 成员归一。
fn normalize_personal_provider_membership(
    personal: &provider_config::ProviderConfig,
    builtin: Option<&provider_config::ProviderConfig>,
    inherited_model_ids: Option<&[String]>,
) -> provider_config::ProviderConfig {
    let inherited = inherited_model_ids
        .map(|values| values.to_vec())
        .unwrap_or_else(|| field_builtin_model_ids(builtin));
    let builtin_ids = unique_strings(&inherited);
    let builtin_set: std::collections::HashSet<String> = builtin_ids.iter().cloned().collect();
    let personal_ids: Vec<String> = match &personal.personal_model_ids {
        Field::Value(values) => unique_strings(values)
            .into_iter()
            .filter(|model_id| !builtin_set.contains(model_id))
            .collect(),
        _ => Vec::new(),
    };
    let mut normalized = personal.clone();
    normalized.personal_model_ids = Field::Value(personal_ids.clone());
    if let Field::Value(requested) = &personal.model_order {
        normalized.model_order = Field::Value(provider_resolver::resolve_owned_order(
            &builtin_ids,
            &personal_ids,
            requested,
        ));
    }
    normalized
}

/// TS `withModelMembershipFrom`：把成员字段拷到目标 config。
fn with_model_membership_from(
    config: &provider_config::ProviderConfig,
    source: &provider_config::ProviderConfig,
) -> provider_config::ProviderConfig {
    let mut next = config.clone();
    next.personal_model_ids = source.personal_model_ids.clone();
    next.model_order = source.model_order.clone();
    next
}

/// TS `assertProviderLabelMutationIsUnique`：只拦截本次变更新引入的重名。
fn assert_provider_label_mutation_is_unique(
    provider_id: &str,
    current: &provider_config::ProviderConfigMap,
    next: &provider_config::ProviderConfigMap,
) -> Result<(), String> {
    let label = |map: &provider_config::ProviderConfigMap| {
        map.get_rule(provider_id)
            .and_then(|rule| match &rule.provider_name {
                Field::Value(value) if !value.trim().is_empty() => {
                    Some(value.trim().to_lowercase())
                }
                _ => None,
            })
    };
    let current_label = label(current);
    let Some(next_key) = label(next) else {
        return Ok(());
    };
    if Some(&next_key) == current_label.as_ref() {
        return Ok(());
    }
    for rule in next.rules() {
        if rule.provider_id == provider_id {
            continue;
        }
        if let Field::Value(candidate) = &rule.provider_name {
            if candidate.trim().to_lowercase() == next_key {
                return Err(format!("Provider 名称已存在: {}", candidate.trim()));
            }
        }
    }
    Ok(())
}

fn assert_membership_current(
    membership: Option<&ProviderModelMembershipRef<'_>>,
    provider_id: &str,
    personal_revision: &str,
) -> Result<(), String> {
    let Some(membership) = membership else {
        return Ok(());
    };
    if membership.provider_id != provider_id {
        return Err("Provider Membership 与目标 Provider 不匹配".to_string());
    }
    if membership.personal_revision != personal_revision {
        return Err("Personal Provider Config 已更新，请刷新后重试".to_string());
    }
    Ok(())
}

/// TS `savePersonalProviderOverlay`：普通保存只更新现有配置，不存在即拒绝。
#[allow(clippy::too_many_arguments)]
pub fn save_personal_provider_overlay(
    snapshot: &ProviderConfigSnapshot,
    current: &PersonalLayer,
    provider_id: &str,
    config: &provider_config::ProviderConfig,
    metadata: Option<&SaveOverlayMetadata>,
    membership: Option<&ProviderModelMembershipRef<'_>>,
) -> Result<ProviderConfigLayerUpdate, String> {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return Err("providerId 不能为空".to_string());
    }
    assert_membership_current(membership, provider_id, &current.revision)?;
    let builtin_rule = snapshot.mode_builtin_providers.get_rule(provider_id);
    let current_personal = current.providers.get_rule(provider_id);
    if let Some(builtin) = builtin_rule {
        if matches!(
            &builtin.config.access,
            Field::Value(provider_config::ProviderAccess::ZhipuAccount(_))
        ) {
            if metadata.and_then(|metadata| metadata.enabled) == Some(false) {
                return Err(format!("Account Provider 不允许禁用: {provider_id}"));
            }
            if !config.access.is_absent() {
                return Err(format!(
                    "固定 Account Provider 的 Access 只能由 Mode Built-in Config 声明: {provider_id}"
                ));
            }
        }
    }
    if current_personal.is_none() && builtin_rule.is_none() {
        return Err(format!("Personal Provider 尚未创建: {provider_id}"));
    }
    let normalized = if let Some(builtin) = builtin_rule {
        if let Field::Value(group) = &config.group {
            let same = matches!(&builtin.config.group, Field::Value(builtin_group) if builtin_group == group);
            if !same {
                return Err(format!(
                    "Personal Overlay 不能改写 Built-in Provider group: {provider_id}"
                ));
            }
        }
        config.without_group()
    } else {
        let group = match &config.group {
            Field::Value(group) => *group,
            _ => match current_personal.map(|rule| &rule.config.group) {
                Some(Field::Value(group)) => *group,
                _ => {
                    return Err(format!(
                        "Personal-only Provider 必须使用 standard-personal group: {provider_id}"
                    ))
                }
            },
        };
        if group != ProviderGroup::StandardPersonal {
            return Err(format!(
                "Personal-only Provider 必须使用 standard-personal group: {provider_id}"
            ));
        }
        config.overlay(&provider_config::ProviderConfig {
            group: Field::Value(ProviderGroup::StandardPersonal),
            ..provider_config::ProviderConfig::default()
        })
    };
    let template_baseline = current_personal
        .and_then(|rule| match &rule.template_id {
            Field::Value(template_id) => snapshot.mode_builtin_provider_templates.get(template_id),
            _ => None,
        })
        .map(|template| &template.config);
    let membership_baseline = builtin_rule.map(|rule| &rule.config).or(template_baseline);
    let next_config = match current_personal.map(|rule| &rule.config) {
        Some(personal_config) => {
            let source = normalize_personal_provider_membership(
                personal_config,
                membership_baseline,
                membership.map(|membership| membership.inherited_model_ids),
            );
            with_model_membership_from(&normalized, &source)
        }
        None => normalized,
    };
    let template_id = match metadata.and_then(|metadata| metadata.template_id.clone()) {
        Some(template_id) => Field::Value(template_id),
        None => current_personal
            .map(|rule| rule.template_id.clone())
            .unwrap_or(Field::Absent),
    };
    let enabled = match metadata.and_then(|metadata| metadata.enabled) {
        Some(enabled) => Field::Value(enabled),
        None => current_personal
            .map(|rule| rule.enabled.clone())
            .unwrap_or(Field::Absent),
    };
    let provider_name = match metadata.and_then(|metadata| metadata.provider_name.clone()) {
        Some(name) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                Field::Null
            } else {
                Field::Value(trimmed.to_string())
            }
        }
        None => current_personal
            .map(|rule| rule.provider_name.clone())
            .unwrap_or(Field::Absent),
    };
    let providers = current
        .providers
        .set_rule(provider_config::ProviderConfigRule {
            provider_id: provider_id.to_string(),
            template_id,
            enabled,
            provider_name,
            config: next_config,
        });
    let current_effective = snapshot.mode_builtin_providers.overlay(&current.providers);
    let next_effective = snapshot.mode_builtin_providers.overlay(&providers);
    assert_provider_label_mutation_is_unique(provider_id, &current_effective, &next_effective)?;
    Ok(ProviderConfigLayerUpdate {
        providers,
        models: current.models.clone(),
        provider_order: current.provider_order.clone(),
        default_model_selection: current
            .default_model_selection
            .as_ref()
            .map(to_layer_selection),
    })
}

/// 删除 Personal Provider：同时清理它的精确模型规则与顺序项。
pub fn delete_personal_provider(
    current: &PersonalLayer,
    provider_id: &str,
) -> Result<ProviderConfigLayerUpdate, String> {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return Err("providerId 不能为空".to_string());
    }
    Ok(ProviderConfigLayerUpdate {
        providers: current.providers.delete(provider_id),
        models: current.models.delete_exact_for_provider(provider_id),
        provider_order: current.provider_order.as_ref().map(|order| {
            order
                .iter()
                .filter(|candidate| *candidate != provider_id)
                .cloned()
                .collect()
        }),
        default_model_selection: current
            .default_model_selection
            .as_ref()
            .map(to_layer_selection),
    })
}

/// 重排 Personal Provider 顺序：只保留 standard-personal 成员，未列出的按原顺序追加。
pub fn reorder_personal_providers(
    current: &PersonalLayer,
    provider_ids: &[String],
) -> ProviderConfigLayerUpdate {
    let personal_ids: Vec<String> = current
        .providers
        .entries()
        .filter(|(_, rule)| {
            matches!(
                &rule.config.group,
                Field::Value(ProviderGroup::StandardPersonal)
            )
        })
        .map(|(provider_id, _)| provider_id.clone())
        .collect();
    ProviderConfigLayerUpdate {
        providers: current.providers.clone(),
        models: current.models.clone(),
        provider_order: Some(provider_resolver::resolve_owned_order(
            &[],
            &personal_ids,
            provider_ids,
        )),
        default_model_selection: current
            .default_model_selection
            .as_ref()
            .map(to_layer_selection),
    }
}

/// Personal 层与文件 codec 各有一份同构 Selection 类型。
fn to_layer_selection(
    selection: &provider_config_source::ModelSelection,
) -> model_selection::ModelSelection {
    model_selection::ModelSelection {
        provider_id: selection.provider_id.clone(),
        model_id: selection.model_id.clone(),
        options: selection
            .options
            .as_ref()
            .map(|options| model_selection::SelectionOptions {
                reasoning_level: options.reasoning_level.clone(),
            }),
    }
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

    fn personal_rule(provider_id: &str) -> provider_config::ProviderConfigRule {
        provider_config::ProviderConfigRule {
            provider_id: provider_id.to_string(),
            template_id: Field::Absent,
            provider_name: Field::Value(provider_id.to_uppercase()),
            enabled: Field::Value(true),
            config: provider_config::ProviderConfig {
                group: Field::Value(ProviderGroup::StandardPersonal),
                ..provider_config::ProviderConfig::default()
            },
        }
    }

    fn personal_layer() -> PersonalLayer {
        let mut providers = provider_config::ProviderConfigMap::empty();
        providers = providers.set_rule(personal_rule("p1"));
        providers = providers.set_rule(personal_rule("p2"));
        let models = provider_config::ModelConfigRules::new(vec![
            provider_config::ModelRule::Provider {
                provider_id: "p1".to_string(),
                model_id: "m1".to_string(),
                config: provider_config::ModelConfig::default(),
            },
            provider_config::ModelRule::Provider {
                provider_id: "p2".to_string(),
                model_id: "m1".to_string(),
                config: provider_config::ModelConfig::default(),
            },
        ]);
        PersonalLayer {
            revision: "rev-1".to_string(),
            providers,
            models,
            provider_order: Some(vec!["p2".to_string(), "p1".to_string()]),
            default_model_selection: Some(provider_config_source::ModelSelection {
                provider_id: "p1".to_string(),
                model_id: "m1".to_string(),
                options: None,
            }),
        }
    }

    #[test]
    fn delete_personal_provider_cleans_rules_and_order() {
        let layer = personal_layer();
        let update = delete_personal_provider(&layer, "p1").expect("delete");
        assert!(update.providers.get("p1").is_none());
        assert!(update.providers.get("p2").is_some());
        assert!(update.models.get_exact_rule("p1", "m1").is_none());
        assert!(update.models.get_exact_rule("p2", "m1").is_some());
        assert_eq!(
            update.provider_order.as_deref(),
            Some(&["p2".to_string()][..])
        );
        // 默认模型选择原样保留。
        assert_eq!(
            update
                .default_model_selection
                .as_ref()
                .map(|selection| selection.provider_id.as_str()),
            Some("p1")
        );
        assert!(delete_personal_provider(&layer, "  ").is_err());
    }

    #[test]
    fn reorder_providers_keeps_members_and_appends_rest() {
        let layer = personal_layer();
        let update = reorder_personal_providers(&layer, &["p2".to_string(), "missing".to_string()]);
        assert_eq!(
            update.provider_order.as_deref(),
            Some(&["p2".to_string(), "p1".to_string()][..])
        );
        // 空请求按原 personal 顺序。
        let update = reorder_personal_providers(&layer, &[]);
        assert_eq!(
            update.provider_order.as_deref(),
            Some(&["p1".to_string(), "p2".to_string()][..])
        );
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

#[cfg(test)]
mod create_provider_tests {
    use super::*;

    use provider_config::{
        ModelConfigRules, ProviderConfigMap, ProviderTemplate, ProviderTemplateMap,
        ProviderTemplateNameMap,
    };

    fn snapshot_with_template() -> ProviderConfigSnapshot {
        let mut templates = ProviderTemplateMap::empty();
        templates.values.insert(
            "tmpl".to_string(),
            ProviderTemplate {
                template_id: "tmpl".to_string(),
                template_name_map: ProviderTemplateNameMap {
                    zh_cn: None,
                    en_us: Some("Template One".to_string()),
                },
                config: provider_config::ProviderConfig::default(),
            },
        );
        let mut builtin = ProviderConfigMap::empty();
        builtin = builtin.set_rule(provider_config::ProviderConfigRule {
            provider_id: "builtin-1".to_string(),
            template_id: Field::Absent,
            provider_name: Field::Value("Builtin".to_string()),
            enabled: Field::Value(true),
            config: provider_config::ProviderConfig::default(),
        });
        ProviderConfigSnapshot {
            revision: "[]".to_string(),
            mode_builtin_revision: String::new(),
            personal_revision: String::new(),
            mode_builtin_providers: builtin,
            mode_builtin_provider_templates: templates,
            personal_providers: ProviderConfigMap::empty(),
            mode_builtin_model_rules: ModelConfigRules::empty(),
            personal_models: ModelConfigRules::empty(),
            personal_provider_order: Vec::new(),
        }
    }

    fn empty_personal() -> PersonalLayer {
        PersonalLayer {
            revision: "rev-1".to_string(),
            providers: ProviderConfigMap::empty(),
            models: ModelConfigRules::empty(),
            provider_order: None,
            default_model_selection: None,
        }
    }

    #[test]
    fn create_from_template_uses_template_identity_and_defaults() {
        let snapshot = snapshot_with_template();
        let current = empty_personal();
        let created = create_personal_provider(
            &snapshot,
            &current,
            CreatePersonalProviderInput {
                template_id: Some("tmpl".to_string()),
                provider_name: None,
                locale: None,
                initial_config: None,
            },
        )
        .expect("create");
        assert_eq!(created.provider_id, "tmpl");
        let rule = created.update.providers.get("tmpl").expect("rule");
        assert_eq!(rule.provider_name, Field::Value("Template One".to_string()));
        assert_eq!(rule.template_id, Field::Value("tmpl".to_string()));
        assert_eq!(
            rule.config.group,
            Field::Value(ProviderGroup::StandardPersonal)
        );
        // 模板实例不注入默认 ApiKey access。
        assert!(rule.config.access.is_absent());
        assert_eq!(
            rule.config.personal_model_ids,
            Field::Value(Vec::<String>::new())
        );
        assert_eq!(
            created.update.provider_order.as_deref(),
            Some(&["tmpl".to_string()][..])
        );
    }

    #[test]
    fn create_dedupes_id_and_label_and_validates_input() {
        let snapshot = snapshot_with_template();
        let first = create_personal_provider(
            &snapshot,
            &empty_personal(),
            CreatePersonalProviderInput {
                template_id: Some("tmpl".to_string()),
                provider_name: None,
                locale: None,
                initial_config: None,
            },
        )
        .expect("first");
        let layer = PersonalLayer {
            revision: "rev-1".to_string(),
            providers: first.update.providers.clone(),
            models: ModelConfigRules::empty(),
            provider_order: first.update.provider_order.clone(),
            default_model_selection: None,
        };
        let second = create_personal_provider(
            &snapshot,
            &layer,
            CreatePersonalProviderInput {
                template_id: Some("tmpl".to_string()),
                provider_name: None,
                locale: None,
                initial_config: None,
            },
        )
        .expect("second");
        assert_eq!(second.provider_id, "tmpl-2");
        let rule = second.update.providers.get("tmpl-2").expect("rule");
        assert_eq!(
            rule.provider_name,
            Field::Value("Template One 2".to_string())
        );

        assert!(create_personal_provider(
            &snapshot,
            &layer,
            CreatePersonalProviderInput {
                template_id: Some("missing".to_string()),
                provider_name: None,
                locale: None,
                initial_config: None,
            }
        )
        .is_err());
        assert!(create_personal_provider(
            &snapshot,
            &layer,
            CreatePersonalProviderInput {
                template_id: None,
                provider_name: None,
                locale: None,
                initial_config: Some(provider_config::ProviderConfig {
                    group: Field::Value(ProviderGroup::StandardPersonal),
                    ..provider_config::ProviderConfig::default()
                }),
            }
        )
        .is_err());
    }

    #[test]
    fn create_without_template_gets_api_key_access() {
        let snapshot = snapshot_with_template();
        let created = create_personal_provider(
            &snapshot,
            &empty_personal(),
            CreatePersonalProviderInput {
                template_id: None,
                provider_name: Some("Mine".to_string()),
                locale: None,
                initial_config: None,
            },
        )
        .expect("create");
        assert_eq!(created.provider_id, "new-provider");
        let rule = created.update.providers.get("new-provider").expect("rule");
        assert_eq!(rule.provider_name, Field::Value("Mine".to_string()));
        assert!(matches!(
            rule.config.access,
            Field::Value(provider_config::ProviderAccess::ApiKey(_))
        ));
    }
}
#[cfg(test)]
mod save_overlay_tests {
    use super::*;
    use provider_config::{
        ModelConfigRules, ProviderConfig, ProviderConfigMap, ProviderTemplate, ProviderTemplateMap,
        ProviderTemplateNameMap,
    };

    fn snapshot(access: provider_config::ProviderAccess) -> ProviderConfigSnapshot {
        let mut builtin = ProviderConfigMap::empty();
        builtin = builtin.set_rule(provider_config::ProviderConfigRule {
            provider_id: "builtin-1".into(),
            template_id: Field::Absent,
            provider_name: Field::Value("Builtin".into()),
            enabled: Field::Value(true),
            config: ProviderConfig {
                group: Field::Value(ProviderGroup::StandardPersonal),
                access: Field::Value(access),
                ..ProviderConfig::default()
            },
        });
        ProviderConfigSnapshot {
            revision: "[]".into(),
            mode_builtin_revision: String::new(),
            personal_revision: String::new(),
            mode_builtin_providers: builtin,
            mode_builtin_provider_templates: ProviderTemplateMap::empty(),
            personal_providers: ProviderConfigMap::empty(),
            mode_builtin_model_rules: ModelConfigRules::empty(),
            personal_models: ModelConfigRules::empty(),
            personal_provider_order: Vec::new(),
        }
    }
    fn layer(rule: Option<provider_config::ProviderConfigRule>) -> PersonalLayer {
        PersonalLayer {
            revision: "rev-1".into(),
            providers: rule
                .map(|rule| ProviderConfigMap::empty().set_rule(rule))
                .unwrap_or_else(ProviderConfigMap::empty),
            models: ModelConfigRules::empty(),
            provider_order: None,
            default_model_selection: None,
        }
    }
    fn api_config(key: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            access: Field::Value(provider_config::ProviderAccess::ApiKey(
                provider_config::ApiKeyAccess {
                    access_type: provider_config::ApiKeyAccessType::ApiKey,
                    api_key: key
                        .map(|value| Field::Value(value.into()))
                        .unwrap_or(Field::Absent),
                    api_key_management_url: Field::Absent,
                },
            )),
            ..ProviderConfig::default()
        }
    }
    fn api_access() -> provider_config::ProviderAccess {
        provider_config::ProviderAccess::ApiKey(provider_config::ApiKeyAccess {
            access_type: provider_config::ApiKeyAccessType::ApiKey,
            api_key: Field::Absent,
            api_key_management_url: Field::Absent,
        })
    }

    #[test]
    fn save_builtin_overlay_strips_group() {
        let snapshot = snapshot(api_access());
        let current = layer(None);
        let mut config = api_config(Some("sk-1"));
        config.group = Field::Value(ProviderGroup::StandardPersonal);
        let update =
            save_personal_provider_overlay(&snapshot, &current, "builtin-1", &config, None, None)
                .expect("save");
        let rule = update.providers.get("builtin-1").expect("rule");
        assert!(rule.config.group.is_absent());
        // Built-in 身份名由 resolver 从 Built-in 配置合并；Personal Overlay 不复制。
        assert!(rule.provider_name.is_absent());
        let Field::Value(provider_config::ProviderAccess::ApiKey(access)) = &rule.config.access
        else {
            panic!("access")
        };
        assert_eq!(access.api_key, Field::Value("sk-1".to_string()));
    }

    #[test]
    fn save_guards_account_unknown_membership_and_labels() {
        let account = snapshot(provider_config::ProviderAccess::ZhipuAccount(
            provider_config::ZhipuAccountAccess::default(),
        ));
        let current = layer(None);
        let metadata = SaveOverlayMetadata {
            template_id: None,
            enabled: Some(false),
            provider_name: None,
        };
        assert!(save_personal_provider_overlay(
            &account,
            &current,
            "builtin-1",
            &ProviderConfig::default(),
            Some(&metadata),
            None
        )
        .is_err());
        assert!(save_personal_provider_overlay(
            &account,
            &current,
            "builtin-1",
            &api_config(Some("sk-1")),
            None,
            None
        )
        .is_err());
        assert!(save_personal_provider_overlay(
            &snapshot(api_access()),
            &current,
            "missing",
            &ProviderConfig::default(),
            None,
            None
        )
        .is_err());
        let membership = ProviderModelMembershipRef {
            provider_id: "builtin-1",
            inherited_model_ids: &[],
            personal_revision: "rev-2",
        };
        assert!(save_personal_provider_overlay(
            &snapshot(api_access()),
            &current,
            "builtin-1",
            &ProviderConfig::default(),
            None,
            Some(&membership)
        )
        .is_err());
        let p1 = provider_config::ProviderConfigRule {
            provider_id: "p1".into(),
            template_id: Field::Absent,
            provider_name: Field::Value("Taken".into()),
            enabled: Field::Absent,
            config: ProviderConfig {
                group: Field::Value(ProviderGroup::StandardPersonal),
                ..ProviderConfig::default()
            },
        };
        let taken = SaveOverlayMetadata {
            template_id: None,
            enabled: None,
            provider_name: Some("Taken".into()),
        };
        assert!(save_personal_provider_overlay(
            &snapshot(api_access()),
            &layer(Some(p1)),
            "builtin-1",
            &ProviderConfig::default(),
            Some(&taken),
            None
        )
        .is_err());
    }

    #[test]
    fn save_normalizes_membership_from_current_personal() {
        let mut templates = ProviderTemplateMap::empty();
        templates.values.insert(
            "tmpl".into(),
            ProviderTemplate {
                template_id: "tmpl".into(),
                template_name_map: ProviderTemplateNameMap {
                    zh_cn: None,
                    en_us: None,
                },
                config: ProviderConfig {
                    builtin_model_ids: Field::Value(vec!["b1".into()]),
                    ..ProviderConfig::default()
                },
            },
        );
        let mut snapshot = snapshot(api_access());
        snapshot.mode_builtin_provider_templates = templates;
        let rule = provider_config::ProviderConfigRule {
            provider_id: "p1".into(),
            template_id: Field::Value("tmpl".into()),
            provider_name: Field::Value("P1".into()),
            enabled: Field::Absent,
            config: ProviderConfig {
                group: Field::Value(ProviderGroup::StandardPersonal),
                personal_model_ids: Field::Value(vec!["b1".into(), "p-a".into(), "p-a".into()]),
                model_order: Field::Value(vec!["p-a".into(), "b1".into()]),
                ..ProviderConfig::default()
            },
        };
        let update = save_personal_provider_overlay(
            &snapshot,
            &layer(Some(rule)),
            "p1",
            &ProviderConfig::default(),
            None,
            None,
        )
        .expect("save");
        let saved = update.providers.get("p1").expect("rule");
        assert_eq!(
            saved.config.personal_model_ids,
            Field::Value(vec!["p-a".to_string()])
        );
        assert_eq!(
            saved.config.model_order,
            Field::Value(vec!["p-a".to_string(), "b1".to_string()])
        );
    }
}
