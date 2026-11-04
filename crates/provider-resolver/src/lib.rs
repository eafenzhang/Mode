//! Provider 配置解析（Registry / Resolver 纯逻辑）。
//!
//! 真源：`packages/provider/src/resolver.ts` 与 `owned-order.ts`。只做纯计算：
//! 输入四层配置（Built-in / Personal / Account Overlay / 规则），输出：
//! - `effective_builtin_providers` / `effective_providers`（含模板与账号覆盖）；
//! - `resolved_providers`（逐 Provider 的模型候选与可执行/可选判定）；
//! - `registry_providers`（通过完整校验、可发布给运行时的子集）；
//! - 汇总 `issues`。
//!
//! 已知差异（spec `docs/specs/provider-resolver.md`）：
//! 1. TS 用 zod `.strict()` 校验并带 `.strict` 语义的路径前缀；Rust 用
//!    `validate_complete_at`/`validate_complete`，模型层 issue 路径由本 crate 手工加前缀；
//! 2. 规则正则只支持 Rust `regex` 文法，JS-only 语法（lookahead/backref）判不命中；
//! 3. 输出为结构化类型，序列化形状由调用方负责（TS 的 `serialize*` 助手未迁）。

use indexmap::IndexMap;
use provider_config::{
    AccountProviderStates, Field, Issue, ModelConfig, ModelConfigRuleResolutionInput,
    ModelConfigRules, ProviderConfig, ProviderConfigMap, ProviderConfigRule, ProviderTemplateMap,
    ProviderVisibility,
};

/// `ModelConfigRules.resolve` 的输入。
pub struct ProviderConfigResolverInput<'a> {
    pub mode_builtin_providers: &'a ProviderConfigMap,
    pub mode_builtin_provider_templates: Option<&'a ProviderTemplateMap>,
    pub personal_providers: &'a ProviderConfigMap,
    pub mode_builtin_model_rules: &'a ModelConfigRules,
    pub personal_models: &'a ModelConfigRules,
    pub account_providers: &'a ProviderConfigMap,
    pub account_states: Option<&'a AccountProviderStates>,
    pub personal_provider_order: Option<&'a [String]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderModelSource {
    Builtin,
    Personal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProviderModel {
    pub model_id: String,
    pub source: ProviderModelSource,
    pub config: ModelConfig,
    pub effective_builtin_config: ModelConfig,
    pub issues: Vec<Issue>,
    pub enabled: bool,
    pub executable: bool,
    pub selectable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub provider_id: String,
    pub enabled: bool,
    pub provider_name: Field<String>,
    pub template_id: Field<String>,
    pub config: ProviderConfig,
    pub template_config: Option<ProviderConfig>,
    pub effective_builtin_config: Option<ProviderConfig>,
    pub provider_issues: Vec<Issue>,
    pub models: Vec<ResolvedProviderModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryModel {
    pub model_id: String,
    pub config: ModelConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryProvider {
    pub provider_id: String,
    pub provider_name: Field<String>,
    pub template_id: Field<String>,
    pub config: ProviderConfig,
    pub models: Vec<RegistryModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigResolution {
    pub effective_builtin_providers: ProviderConfigMap,
    pub effective_providers: ProviderConfigMap,
    pub resolved_providers: Vec<ResolvedProvider>,
    pub registry_providers: Vec<RegistryProvider>,
    pub issues: Vec<Issue>,
}

pub struct ProviderConfigResolver;

impl ProviderConfigResolver {
    pub fn resolve(&self, input: &ProviderConfigResolverInput<'_>) -> ProviderConfigResolution {
        // 1. Account 覆盖只作用于 Built-in 已存在的 Provider，并去掉分组归属。
        let account_providers = ProviderConfigMap {
            values: input
                .account_providers
                .values
                .iter()
                .filter(|(provider_id, _)| input.mode_builtin_providers.get(provider_id).is_some())
                .map(|(provider_id, rule)| {
                    (
                        provider_id.clone(),
                        ProviderConfigRule {
                            config: rule.config.without_group(),
                            ..rule.clone()
                        },
                    )
                })
                .collect::<IndexMap<_, _>>(),
        };
        let concrete_builtin = input.mode_builtin_providers.overlay(&account_providers);
        let templates = input.mode_builtin_provider_templates;
        // 2. Built-in 模板叠加。
        let effective_builtin = concrete_builtin.map_configs(|concrete, _provider_id, rule| {
            if let Field::Value(template_id) = &rule.template_id {
                if let Some(template) = templates.and_then(|map| map.get(template_id)) {
                    return template.config.overlay(concrete);
                }
            }
            concrete.clone()
        });
        // 3. Personal：Built-in 已存在的同 id 去分组；非 Built-in 走模板叠加。
        let personal = input
            .personal_providers
            .map_configs(|config, provider_id, _rule| {
                if effective_builtin.get(provider_id).is_some() {
                    config.without_group()
                } else {
                    config.clone()
                }
            });
        let templated_personal = personal.map_configs(|config, provider_id, rule| {
            if effective_builtin.get(provider_id).is_some() {
                return config.clone();
            }
            if let Field::Value(template_id) = &rule.template_id {
                if let Some(template) = templates.and_then(|map| map.get(template_id)) {
                    return template.config.overlay(config);
                }
            }
            config.clone()
        });
        let effective_providers = effective_builtin.overlay(&templated_personal);
        let effective_model_rules = ModelConfigRules::compose_effective(
            input.mode_builtin_model_rules,
            input.personal_models,
        );

        let mut issues: Vec<Issue> = Vec::new();
        let mut resolved_providers: Vec<ResolvedProvider> = Vec::new();
        let mut registry_providers: Vec<RegistryProvider> = Vec::new();

        for provider_id in resolve_provider_order(input, &effective_providers) {
            let Some(rule) = effective_providers.get(&provider_id) else {
                continue;
            };
            let config = &rule.config;
            // 账号不再支持总禁用；旧覆盖值不能让无开关的账号永久失效。
            let enabled =
                is_zhipu_account(&config.access) || !matches!(rule.enabled, Field::Value(false));
            let provider_path = vec!["providers".to_string(), provider_id.clone()];
            let mut provider_issues = config.validate_complete_at(&provider_path);
            let template_id = rule.template_id.value().cloned();
            let template_config = template_id
                .as_deref()
                .and_then(|id| templates.and_then(|map| map.get(id)))
                .map(|template| template.config.clone());
            if let Some(template_id) = &template_id {
                if template_config.is_none() {
                    let mut path = provider_path.clone();
                    path.push("templateId".to_string());
                    provider_issues.push(Issue::new(
                        "missing-template",
                        path,
                        format!("Provider Template 不存在: {template_id}"),
                    ));
                }
            }
            issues.extend(provider_issues.iter().cloned());

            let builtin_ids_in_order = unique_in_order(
                &config
                    .builtin_model_ids
                    .value()
                    .cloned()
                    .unwrap_or_default(),
            );
            let builtin_set: std::collections::HashSet<String> =
                builtin_ids_in_order.iter().cloned().collect();
            let personal_ids_in_order: Vec<String> = unique_in_order(
                &config
                    .personal_model_ids
                    .value()
                    .cloned()
                    .unwrap_or_default(),
            )
            .into_iter()
            .filter(|model_id| !builtin_set.contains(model_id))
            .collect();
            let ordered_model_ids = resolve_owned_order(
                &builtin_ids_in_order,
                &personal_ids_in_order,
                &config.model_order.value().cloned().unwrap_or_default(),
            );

            let access_entitled = !is_zhipu_account(&config.access)
                || matches!(zhipu_entitled(&config.access), Field::Value(true));
            let account_current = input
                .account_states
                .and_then(|states| states.get(&provider_id))
                .and_then(|state| state.current)
                .unwrap_or(true);
            let provider_executable =
                enabled && access_entitled && account_current && provider_issues.is_empty();

            let mut models: Vec<ResolvedProviderModel> = Vec::new();
            let mut model_issues_all: Vec<Issue> = Vec::new();
            for model_id in ordered_model_ids {
                let resolve_input = ModelConfigRuleResolutionInput {
                    provider_id: &provider_id,
                    template_id: template_id.as_deref(),
                    model_id: &model_id,
                    api_type: provider_api_type(config),
                    base_url: provider_base_url(config),
                };
                let model_config = effective_model_rules.resolve(&resolve_input);
                let effective_builtin_config =
                    input.mode_builtin_model_rules.resolve(&resolve_input);
                let mut model_issues = model_config.validate_complete();
                prefix_paths(&mut model_issues, &provider_path, &model_id);
                model_issues_all.extend(model_issues.iter().cloned());
                let model_enabled = matches!(model_config.enabled, Field::Value(true));
                let executable = provider_executable && model_enabled && model_issues.is_empty();
                let selectable = executable
                    && !matches!(config.visibility, Field::Value(ProviderVisibility::Hidden));
                let source = if builtin_set.contains(&model_id) {
                    ProviderModelSource::Builtin
                } else {
                    ProviderModelSource::Personal
                };
                models.push(ResolvedProviderModel {
                    model_id,
                    source,
                    config: model_config,
                    effective_builtin_config,
                    issues: model_issues,
                    enabled: model_enabled,
                    executable,
                    selectable,
                });
            }
            issues.extend(model_issues_all);
            resolved_providers.push(ResolvedProvider {
                provider_id: provider_id.clone(),
                enabled,
                provider_name: rule.provider_name.clone(),
                template_id: field_from_option(template_id.clone()),
                config: config.clone(),
                template_config,
                effective_builtin_config: effective_builtin
                    .get(&provider_id)
                    .map(|rule| rule.config.clone()),
                provider_issues,
                models: models.clone(),
            });

            if resolved_providers
                .last()
                .map(|resolved| resolved.provider_issues.is_empty())
                .unwrap_or(false)
            {
                let valid_models: Vec<RegistryModel> = models
                    .iter()
                    .filter(|model| model.executable)
                    .map(|model| RegistryModel {
                        model_id: model.model_id.clone(),
                        config: model.config.clone(),
                    })
                    .collect();
                if !valid_models.is_empty() {
                    registry_providers.push(RegistryProvider {
                        provider_id: provider_id.clone(),
                        provider_name: rule.provider_name.clone(),
                        template_id: field_from_option(template_id),
                        config: config.clone(),
                        models: valid_models,
                    });
                }
            }
        }

        ProviderConfigResolution {
            effective_builtin_providers: effective_builtin,
            effective_providers,
            resolved_providers,
            registry_providers,
            issues,
        }
    }
}

fn is_zhipu_account(access: &Field<provider_config::ProviderAccess>) -> bool {
    matches!(
        access,
        Field::Value(provider_config::ProviderAccess::ZhipuAccount(_))
    )
}

fn zhipu_entitled(access: &Field<provider_config::ProviderAccess>) -> Field<bool> {
    match access {
        Field::Value(provider_config::ProviderAccess::ZhipuAccount(account)) => {
            account.entitled.clone()
        }
        _ => Field::Absent,
    }
}

fn provider_api_type(config: &ProviderConfig) -> Option<&str> {
    match &config.api {
        Field::Value(api) => match &api.api_type {
            Field::Value(api_type) => Some(api_type.as_str()),
            _ => None,
        },
        _ => None,
    }
}

fn provider_base_url(config: &ProviderConfig) -> Option<&str> {
    match &config.api {
        Field::Value(api) => match &api.base_url {
            Field::Value(base_url) => Some(base_url.as_str()),
            _ => None,
        },
        _ => None,
    }
}

fn prefix_paths(issues: &mut [Issue], provider_path: &[String], model_id: &str) {
    for issue in issues {
        let mut path = provider_path.to_vec();
        path.push("models".to_string());
        path.push(model_id.to_string());
        path.extend(issue.path.clone());
        issue.path = path;
    }
}

fn resolve_provider_order(
    input: &ProviderConfigResolverInput<'_>,
    effective_providers: &ProviderConfigMap,
) -> Vec<String> {
    let source_ids = effective_providers.keys();
    let family_ids: Vec<String> = source_ids
        .iter()
        .filter(|provider_id| {
            matches!(
                effective_providers.get(provider_id).and_then(|rule| rule.config.group.value()),
                Some(group)
                    if matches!(
                        group,
                        provider_config::ProviderGroup::ZaiFamily
                            | provider_config::ProviderGroup::BigmodelFamily
                    )
            )
        })
        .cloned()
        .collect();
    let family_set: std::collections::HashSet<String> = family_ids.iter().cloned().collect();
    let builtin_ids: Vec<String> = input
        .mode_builtin_providers
        .keys()
        .into_iter()
        .filter(|provider_id| !family_set.contains(provider_id))
        .collect();
    let builtin_set: std::collections::HashSet<String> = builtin_ids.iter().cloned().collect();
    let personal_ids: Vec<String> = input
        .personal_providers
        .keys()
        .into_iter()
        .filter(|provider_id| {
            !builtin_set.contains(provider_id) && !family_set.contains(provider_id)
        })
        .collect();
    let mut result = family_ids;
    result.extend(resolve_owned_order(
        &builtin_ids,
        &personal_ids,
        input.personal_provider_order.unwrap_or(&[]),
    ));
    result
}

fn resolve_owned_order(
    builtin_ids: &[String],
    personal_ids: &[String],
    requested_order: &[String],
) -> Vec<String> {
    let builtin = unique_in_order(builtin_ids);
    let builtin_set: std::collections::HashSet<String> = builtin.iter().cloned().collect();
    let personal: Vec<String> = unique_in_order(personal_ids)
        .into_iter()
        .filter(|id| !builtin_set.contains(id))
        .collect();
    let members: std::collections::HashSet<String> =
        builtin.iter().chain(personal.iter()).cloned().collect();
    let ordered: Vec<String> = unique_in_order(requested_order)
        .into_iter()
        .filter(|id| members.contains(id))
        .collect();
    let ordered_set: std::collections::HashSet<String> = ordered.iter().cloned().collect();
    let mut result: Vec<String> = builtin
        .iter()
        .filter(|id| !ordered_set.contains(*id))
        .cloned()
        .collect();
    result.extend(ordered);
    result.extend(
        personal
            .iter()
            .filter(|id| !ordered_set.contains(*id))
            .cloned(),
    );
    result
}

fn unique_in_order(values: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            result.push(value.clone());
        }
    }
    result
}

fn field_from_option(value: Option<String>) -> Field<String> {
    value.map(Field::Value).unwrap_or(Field::Absent)
}
