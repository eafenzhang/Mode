//! 模型规则容器（六类规则）与内置/个人规则列表。
//!
//! 真源：`packages/provider/src/config/model-config.ts` 的 `ModelConfigRules`、
//! `packages/provider/src/config/rule-data-schema.ts`、
//! `packages/provider/src/config/schema.ts`。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use indexmap::IndexMap;

use crate::json::{array, object, string, JsonValue};
use crate::model::{parse_manual_model_config, parse_model_config, ModelConfig};
use crate::regex::is_valid_js_regex_pattern;
use crate::value::{
    check_unknown_keys, expect_object, parse_non_empty_string, parse_optional_non_null_field,
    parse_required_field, parse_string, Field, Issue, ParseError, ParseResult,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRule {
    Model {
        model_match: String,
        config: ModelConfig,
    },
    ModelApi {
        model_match: String,
        config: ModelConfig,
        api_type_match: String,
    },
    ProviderSite {
        model_match: String,
        config: ModelConfig,
        base_url_match: String,
        api_type_match: Option<String>,
    },
    Template {
        template_id: String,
        model_id: String,
        config: ModelConfig,
    },
    Provider {
        provider_id: String,
        model_id: String,
        config: ModelConfig,
    },
    Manual {
        provider_id: String,
        model_id: String,
        config: ModelConfig,
    },
}

/// 精确模型规则类型（TS `ExactModelConfigRule.type`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactModelRuleKind {
    Provider,
    Manual,
}

fn exact_rule<'a>(
    rule: &'a ModelRule,
    provider_id: &str,
    model_id: &str,
) -> Option<(ExactModelRuleKind, &'a ModelConfig)> {
    match rule {
        ModelRule::Provider {
            provider_id: rule_provider,
            model_id: rule_model,
            config,
        } if rule_provider == provider_id && rule_model == model_id => {
            Some((ExactModelRuleKind::Provider, config))
        }
        ModelRule::Manual {
            provider_id: rule_provider,
            model_id: rule_model,
            config,
        } if rule_provider == provider_id && rule_model == model_id => {
            Some((ExactModelRuleKind::Manual, config))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelConfigRules {
    rules: Vec<ModelRule>,
}

impl ModelConfigRules {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(rules: Vec<ModelRule>) -> Self {
        Self { rules }
    }

    pub fn rules(&self) -> &[ModelRule] {
        &self.rules
    }

    /// TS `getExact`：按插入序叠加所有 `(providerId, modelId)` 精确规则
    /// （`provider-model` 与 `manual-provider-model` 都是精确规则）。
    pub fn get_exact(&self, provider_id: &str, model_id: &str) -> Option<ModelConfig> {
        let mut result: Option<ModelConfig> = None;
        for rule in &self.rules {
            let Some((_, config)) = exact_rule(rule, provider_id, model_id) else {
                continue;
            };
            result = Some(match result {
                Some(current) => current.overlay(config),
                None => config.clone(),
            });
        }
        result
    }

    /// TS `setExact`：替换/新增精确规则；保留第一条位置并丢弃后续重复。
    pub fn set_exact(
        &self,
        provider_id: &str,
        model_id: &str,
        config: ModelConfig,
        use_recommended_config: Option<bool>,
    ) -> ModelConfigRules {
        let manual = match use_recommended_config {
            Some(use_recommended) => !use_recommended,
            None => matches!(
                self.get_exact_rule(provider_id, model_id),
                Some((ExactModelRuleKind::Manual, _))
            ),
        };
        let replacement = if manual {
            ModelRule::Manual {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
                config,
            }
        } else {
            ModelRule::Provider {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
                config,
            }
        };
        let mut replaced = false;
        let mut rules = Vec::with_capacity(self.rules.len());
        for rule in &self.rules {
            if exact_rule(rule, provider_id, model_id).is_some() {
                if !replaced {
                    rules.push(replacement.clone());
                    replaced = true;
                }
                continue;
            }
            rules.push(rule.clone());
        }
        if !replaced {
            rules.push(replacement);
        }
        ModelConfigRules { rules }
    }

    /// TS `deleteExact`：删除指定 `(providerId, modelId)` 的全部精确规则。
    pub fn delete_exact(&self, provider_id: &str, model_id: &str) -> ModelConfigRules {
        ModelConfigRules {
            rules: self
                .rules
                .iter()
                .filter(|rule| exact_rule(rule, provider_id, model_id).is_none())
                .cloned()
                .collect(),
        }
    }

    /// TS `renameExactModel`：同 provider 下精确规则的 modelId 改名（保持位置）。
    pub fn rename_exact_model(
        &self,
        provider_id: &str,
        current_model_id: &str,
        next_model_id: &str,
    ) -> ModelConfigRules {
        if current_model_id == next_model_id {
            return self.clone();
        }
        ModelConfigRules {
            rules: self
                .rules
                .iter()
                .map(|rule| match rule {
                    ModelRule::Provider {
                        provider_id: rule_provider,
                        model_id: rule_model,
                        config,
                    } if rule_provider == provider_id && rule_model == current_model_id => {
                        ModelRule::Provider {
                            provider_id: rule_provider.clone(),
                            model_id: next_model_id.to_string(),
                            config: config.clone(),
                        }
                    }
                    ModelRule::Manual {
                        provider_id: rule_provider,
                        model_id: rule_model,
                        config,
                    } if rule_provider == provider_id && rule_model == current_model_id => {
                        ModelRule::Manual {
                            provider_id: rule_provider.clone(),
                            model_id: next_model_id.to_string(),
                            config: config.clone(),
                        }
                    }
                    other => other.clone(),
                })
                .collect(),
        }
    }

    /// TS `deleteExactForProvider`：删除该 provider 的所有精确规则（Provider/Manual），
    /// 保留模板/站点/模型名等非精确规则。
    pub fn delete_exact_for_provider(&self, provider_id: &str) -> ModelConfigRules {
        ModelConfigRules {
            rules: self
                .rules
                .iter()
                .filter(|rule| {
                    !matches!(
                        rule,
                        ModelRule::Provider { provider_id: rule_provider, .. }
                            | ModelRule::Manual { provider_id: rule_provider, .. }
                            if rule_provider == provider_id
                    )
                })
                .cloned()
                .collect(),
        }
    }

    /// TS `getExactRule`：返回最后一条精确规则及其类型（决定 useRecommendedConfig）。
    pub fn get_exact_rule<'a>(
        &'a self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<(ExactModelRuleKind, &'a ModelConfig)> {
        self.rules
            .iter()
            .rev()
            .find_map(|rule| exact_rule(rule, provider_id, model_id))
    }

    /// `toModeBuiltinJSON()`：五组按 schema 顺序输出。
    pub fn to_mode_builtin_json(&self) -> JsonValue {
        object([
            (
                "modelRules",
                array(
                    self.rules
                        .iter()
                        .filter_map(|rule| match rule {
                            ModelRule::Model {
                                model_match,
                                config,
                            } => Some(object([
                                ("modelMatch", string(model_match)),
                                ("config", config.to_json()),
                            ])),
                            _ => None,
                        })
                        .collect(),
                ),
            ),
            (
                "modelApiRules",
                array(
                    self.rules
                        .iter()
                        .filter_map(|rule| match rule {
                            ModelRule::ModelApi {
                                model_match,
                                config,
                                api_type_match,
                            } => Some(object([
                                ("modelMatch", string(model_match)),
                                ("config", config.to_json()),
                                ("apiTypeMatch", string(api_type_match)),
                            ])),
                            _ => None,
                        })
                        .collect(),
                ),
            ),
            (
                "providerSiteRules",
                array(
                    self.rules
                        .iter()
                        .filter_map(|rule| match rule {
                            ModelRule::ProviderSite {
                                model_match,
                                config,
                                base_url_match,
                                api_type_match,
                            } => {
                                let mut entries = vec![
                                    ("modelMatch", string(model_match)),
                                    ("config", config.to_json()),
                                    ("baseUrlMatch", string(base_url_match)),
                                ];
                                if let Some(value) = api_type_match {
                                    entries.push(("apiTypeMatch", string(value)));
                                }
                                Some(object_entries(entries))
                            }
                            _ => None,
                        })
                        .collect(),
                ),
            ),
            (
                "templateModelRules",
                array(
                    self.rules
                        .iter()
                        .filter_map(|rule| match rule {
                            ModelRule::Template {
                                template_id,
                                model_id,
                                config,
                            } => Some(object([
                                ("templateId", string(template_id)),
                                ("modelId", string(model_id)),
                                ("config", config.to_json()),
                            ])),
                            _ => None,
                        })
                        .collect(),
                ),
            ),
            (
                "builtinProviderModelRules",
                array(
                    self.rules
                        .iter()
                        .filter_map(|rule| match rule {
                            ModelRule::Provider {
                                provider_id,
                                model_id,
                                config,
                            } => Some(object([
                                ("modelId", string(model_id)),
                                ("config", config.to_json()),
                                ("providerId", string(provider_id)),
                            ])),
                            _ => None,
                        })
                        .collect(),
                ),
            ),
        ])
    }

    /// `toPersonalJSON()`：两组都按 `modelId,config,providerId`。
    pub fn to_personal_json(&self) -> JsonValue {
        object([
            (
                "providerModelRules",
                array(
                    self.rules
                        .iter()
                        .filter_map(|rule| match rule {
                            ModelRule::Provider {
                                provider_id,
                                model_id,
                                config,
                            } => Some(object([
                                ("modelId", string(model_id)),
                                ("config", config.to_json()),
                                ("providerId", string(provider_id)),
                            ])),
                            _ => None,
                        })
                        .collect(),
                ),
            ),
            (
                "manualProviderModelRules",
                array(
                    self.rules
                        .iter()
                        .filter_map(|rule| match rule {
                            ModelRule::Manual {
                                provider_id,
                                model_id,
                                config,
                            } => Some(object([
                                ("modelId", string(model_id)),
                                ("config", config.to_json()),
                                ("providerId", string(provider_id)),
                            ])),
                            _ => None,
                        })
                        .collect(),
                ),
            ),
        ])
    }

    /// `toJSON()`：内置五组 + `#collect("manual-provider-model")`
    /// （身份字段在前、`config` 在最后）。
    pub fn to_json(&self) -> JsonValue {
        let mut entries = Vec::new();
        if let JsonValue::Object(groups) = self.to_mode_builtin_json() {
            for (key, value) in groups {
                entries.push((key, value));
            }
        }
        entries.push((
            "manualProviderModelRules".to_owned(),
            array(
                self.rules
                    .iter()
                    .filter_map(|rule| match rule {
                        ModelRule::Manual {
                            provider_id,
                            model_id,
                            config,
                        } => Some(object([
                            ("modelId", string(model_id)),
                            ("providerId", string(provider_id)),
                            ("config", config.to_json()),
                        ])),
                        _ => None,
                    })
                    .collect(),
            ),
        ));
        let mut map = IndexMap::new();
        for (key, value) in entries {
            map.insert(key, value);
        }
        JsonValue::Object(map)
    }
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

pub fn parse_mode_builtin_model_config_rules(
    value: &JsonValue,
    path: &[String],
) -> ParseResult<ModelConfigRules> {
    let map = expect_object(value, path).map_err(ParseError::Issues)?;
    let mut issues = Vec::new();
    check_unknown_keys(
        map,
        &[
            "modelRules",
            "modelApiRules",
            "providerSiteRules",
            "templateModelRules",
            "builtinProviderModelRules",
        ],
        path,
        &mut issues,
    );
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }

    let mut rules = Vec::new();
    rules.extend(parse_rule_items(
        map,
        "modelRules",
        path,
        parse_model_match_rule,
    )?);
    rules.extend(parse_rule_items(
        map,
        "modelApiRules",
        path,
        parse_model_api_rule,
    )?);
    rules.extend(parse_rule_items(
        map,
        "providerSiteRules",
        path,
        parse_provider_site_rule,
    )?);
    rules.extend(parse_rule_items(
        map,
        "templateModelRules",
        path,
        parse_template_model_rule,
    )?);
    rules.extend(parse_rule_items(
        map,
        "builtinProviderModelRules",
        path,
        parse_provider_model_rule,
    )?);
    Ok(ModelConfigRules::new(rules))
}

pub fn parse_personal_model_config_rules(
    value: &JsonValue,
    path: &[String],
) -> ParseResult<ModelConfigRules> {
    let map = expect_object(value, path).map_err(ParseError::Issues)?;
    let mut issues = Vec::new();
    check_unknown_keys(
        map,
        &["providerModelRules", "manualProviderModelRules"],
        path,
        &mut issues,
    );
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }

    let provider_rules =
        parse_rule_items(map, "providerModelRules", path, parse_provider_model_rule)?;
    let manual_rules = parse_rule_items(
        map,
        "manualProviderModelRules",
        path,
        parse_manual_provider_model_rule,
    )?;

    let smart_ids = provider_rules
        .iter()
        .map(|rule| match rule {
            ModelRule::Provider {
                provider_id,
                model_id,
                ..
            } => format!("{provider_id}\u{1f}{model_id}"),
            _ => String::new(),
        })
        .collect::<std::collections::HashSet<_>>();
    for (index, rule) in manual_rules.iter().enumerate() {
        if let ModelRule::Manual {
            provider_id,
            model_id,
            ..
        } = rule
        {
            if smart_ids.contains(&format!("{provider_id}\u{1f}{model_id}")) {
                return Err(ParseError::Issues(vec![Issue::custom(
                    &[
                        path.to_vec(),
                        vec!["manualProviderModelRules".to_owned(), index.to_string()],
                    ]
                    .concat(),
                    "同一 Provider/Model 不能同时声明智能和手动配置",
                )]));
            }
        }
    }

    let mut rules = provider_rules;
    rules.extend(manual_rules);
    Ok(ModelConfigRules::new(rules))
}

fn parse_rule_items(
    map: &IndexMap<String, JsonValue>,
    key: &str,
    path: &[String],
    parser: impl Fn(&JsonValue, &[String]) -> Result<ModelRule, Vec<Issue>>,
) -> ParseResult<Vec<ModelRule>> {
    let mut field_path = path.to_vec();
    field_path.push(key.to_owned());
    let Some(value) = map.get(key) else {
        return Err(ParseError::Issues(vec![Issue::invalid_type(
            &field_path,
            "array",
            "undefined",
        )]));
    };
    let Some(entries) = value.as_array() else {
        return Err(ParseError::Issues(vec![Issue::invalid_type(
            &field_path,
            "array",
            value.kind(),
        )]));
    };
    let mut result = Vec::with_capacity(entries.len());
    let mut issues = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let mut entry_path = field_path.clone();
        entry_path.push(index.to_string());
        match parser(entry, &entry_path) {
            Ok(rule) => result.push(rule),
            Err(mut errors) => issues.append(&mut errors),
        }
    }
    if issues.is_empty() {
        Ok(result)
    } else {
        Err(ParseError::Issues(issues))
    }
}

fn parse_model_match_rule(value: &JsonValue, path: &[String]) -> Result<ModelRule, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["modelMatch", "config"], path, &mut issues);

    let model_match = parse_required_field(map, "modelMatch", path, "string", parse_pattern)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            String::new()
        });
    let config = parse_required_field(map, "config", path, "object", |value, field_path| {
        parse_model_config(value, field_path).map_err(parse_error_to_issues)
    })
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        ModelConfig::empty()
    });

    if issues.is_empty() {
        Ok(ModelRule::Model {
            model_match,
            config,
        })
    } else {
        Err(issues)
    }
}

fn parse_model_api_rule(value: &JsonValue, path: &[String]) -> Result<ModelRule, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(
        map,
        &["modelMatch", "config", "apiTypeMatch"],
        path,
        &mut issues,
    );

    let model_match = parse_required_field(map, "modelMatch", path, "string", parse_pattern)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            String::new()
        });
    let config = parse_required_field(map, "config", path, "object", |value, field_path| {
        parse_model_config(value, field_path).map_err(parse_error_to_issues)
    })
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        ModelConfig::empty()
    });
    let api_type_match = parse_required_field(map, "apiTypeMatch", path, "string", parse_pattern)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            String::new()
        });

    if issues.is_empty() {
        Ok(ModelRule::ModelApi {
            model_match,
            config,
            api_type_match,
        })
    } else {
        Err(issues)
    }
}

fn parse_provider_site_rule(value: &JsonValue, path: &[String]) -> Result<ModelRule, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(
        map,
        &["modelMatch", "config", "baseUrlMatch", "apiTypeMatch"],
        path,
        &mut issues,
    );

    let model_match = parse_required_field(map, "modelMatch", path, "string", parse_pattern)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            String::new()
        });
    let config = parse_required_field(map, "config", path, "object", |value, field_path| {
        parse_model_config(value, field_path).map_err(parse_error_to_issues)
    })
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        ModelConfig::empty()
    });
    let base_url_match = parse_required_field(map, "baseUrlMatch", path, "string", parse_pattern)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            String::new()
        });
    let api_type_match =
        parse_optional_non_null_field(map, "apiTypeMatch", path, "string", parse_pattern)
            .unwrap_or_else(|mut errors| {
                issues.append(&mut errors);
                Field::Absent
            })
            .into_value();

    if issues.is_empty() {
        Ok(ModelRule::ProviderSite {
            model_match,
            config,
            base_url_match,
            api_type_match,
        })
    } else {
        Err(issues)
    }
}

fn parse_template_model_rule(value: &JsonValue, path: &[String]) -> Result<ModelRule, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["templateId", "modelId", "config"], path, &mut issues);

    let template_id =
        parse_required_field(map, "templateId", path, "string", parse_non_empty_string)
            .unwrap_or_else(|mut errors| {
                issues.append(&mut errors);
                String::new()
            });
    let model_id = parse_required_field(map, "modelId", path, "string", parse_non_empty_string)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            String::new()
        });
    let config = parse_required_field(map, "config", path, "object", |value, field_path| {
        parse_model_config(value, field_path).map_err(parse_error_to_issues)
    })
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        ModelConfig::empty()
    });

    if issues.is_empty() {
        Ok(ModelRule::Template {
            template_id,
            model_id,
            config,
        })
    } else {
        Err(issues)
    }
}

fn parse_provider_model_rule(value: &JsonValue, path: &[String]) -> Result<ModelRule, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["providerId", "modelId", "config"], path, &mut issues);

    let provider_id =
        parse_required_field(map, "providerId", path, "string", parse_non_empty_string)
            .unwrap_or_else(|mut errors| {
                issues.append(&mut errors);
                String::new()
            });
    let model_id = parse_required_field(map, "modelId", path, "string", parse_non_empty_string)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            String::new()
        });
    let config = parse_required_field(map, "config", path, "object", |value, field_path| {
        parse_model_config(value, field_path).map_err(parse_error_to_issues)
    })
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        ModelConfig::empty()
    });

    if issues.is_empty() {
        Ok(ModelRule::Provider {
            provider_id,
            model_id,
            config,
        })
    } else {
        Err(issues)
    }
}

fn parse_manual_provider_model_rule(
    value: &JsonValue,
    path: &[String],
) -> Result<ModelRule, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["providerId", "modelId", "config"], path, &mut issues);

    let provider_id =
        parse_required_field(map, "providerId", path, "string", parse_non_empty_string)
            .unwrap_or_else(|mut errors| {
                issues.append(&mut errors);
                String::new()
            });
    let model_id = parse_required_field(map, "modelId", path, "string", parse_non_empty_string)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            String::new()
        });
    let config = parse_required_field(map, "config", path, "object", |value, field_path| {
        parse_manual_model_config(value, field_path)
            .map(|manual| manual.into_model())
            .map_err(parse_error_to_issues)
    })
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        ModelConfig::empty()
    });

    if issues.is_empty() {
        Ok(ModelRule::Manual {
            provider_id,
            model_id,
            config,
        })
    } else {
        Err(issues)
    }
}

fn parse_pattern(value: &JsonValue, path: &[String]) -> Result<String, Vec<Issue>> {
    let pattern = parse_string(value, path)?;
    if pattern.is_empty() {
        return Err(vec![Issue::too_small(path, ">=1 character(s)")]);
    }
    if !is_valid_js_regex_pattern(&pattern) {
        return Err(vec![Issue::custom(path, "无效匹配正则")]);
    }
    Ok(pattern)
}

fn parse_error_to_issues(error: ParseError) -> Vec<Issue> {
    match error {
        ParseError::Issues(issues) => issues,
        ParseError::Invariant(message) => {
            vec![Issue::new("invalid-config", Vec::new(), message)]
        }
    }
}

fn object_entries(entries: Vec<(&'static str, JsonValue)>) -> JsonValue {
    let mut map = IndexMap::new();
    for (key, value) in entries {
        map.insert(key.to_owned(), value);
    }
    JsonValue::Object(map)
}

/// `ModelConfigRules.resolve` 的输入（与 TS `ModelConfigRuleResolutionInput` 同构）。
pub struct ModelConfigRuleResolutionInput<'a> {
    pub provider_id: &'a str,
    pub template_id: Option<&'a str>,
    pub model_id: &'a str,
    pub api_type: Option<&'a str>,
    pub base_url: Option<&'a str>,
}

impl ModelConfigRules {
    /// Built-in 保留原层次和组内顺序；个人只包含普通/手动精确规则。
    pub fn compose_effective(
        builtin: &ModelConfigRules,
        personal: &ModelConfigRules,
    ) -> ModelConfigRules {
        let mut rules = builtin.rules.clone();
        rules.extend(
            personal
                .rules
                .iter()
                .filter(|rule| is_exact_model_rule(rule))
                .cloned(),
        );
        ModelConfigRules { rules }
    }

    /// TS `resolve`：按规则声明顺序逐条 overlay；手动精确规则先清掉 properties/optionSpecs。
    pub fn resolve(&self, input: &ModelConfigRuleResolutionInput<'_>) -> ModelConfig {
        let mut result = ModelConfig::default();
        let base_url = input.base_url.and_then(normalize_base_url_for_rule_match);
        for rule in &self.rules {
            match rule {
                ModelRule::Provider {
                    provider_id,
                    model_id,
                    config,
                } => {
                    if provider_id != input.provider_id || model_id != input.model_id {
                        continue;
                    }
                    result = result.overlay(config);
                }
                ModelRule::Manual {
                    provider_id,
                    model_id,
                    config,
                } => {
                    if provider_id != input.provider_id || model_id != input.model_id {
                        continue;
                    }
                    result = result.clear_manual().overlay(config);
                }
                ModelRule::Template {
                    template_id,
                    model_id,
                    config,
                } => {
                    if input.template_id != Some(template_id.as_str()) || model_id != input.model_id
                    {
                        continue;
                    }
                    result = result.overlay(config);
                }
                ModelRule::Model {
                    model_match,
                    config,
                } => {
                    // 只放宽推荐规则匹配，不改真实请求里的模型 ID。
                    if !matches_rule(model_match, input.model_id, true) {
                        continue;
                    }
                    result = result.overlay(config);
                }
                ModelRule::ModelApi {
                    model_match,
                    config,
                    api_type_match,
                } => {
                    if !matches_rule(model_match, input.model_id, true) {
                        continue;
                    }
                    let Some(api_type) = input.api_type else {
                        continue;
                    };
                    if !matches_rule(api_type_match, api_type, false) {
                        continue;
                    }
                    result = result.overlay(config);
                }
                ModelRule::ProviderSite {
                    model_match,
                    config,
                    base_url_match,
                    api_type_match,
                } => {
                    if !matches_rule(model_match, input.model_id, true) {
                        continue;
                    }
                    if let Some(api_type_match) = api_type_match {
                        let Some(api_type) = input.api_type else {
                            continue;
                        };
                        if !matches_rule(api_type_match, api_type, false) {
                            continue;
                        }
                    }
                    let Some(base_url) = base_url.as_deref() else {
                        continue;
                    };
                    if !matches_rule(base_url_match, base_url, false) {
                        continue;
                    }
                    result = result.overlay(config);
                }
            }
        }
        result
    }
}

fn is_exact_model_rule(rule: &ModelRule) -> bool {
    matches!(rule, ModelRule::Provider { .. } | ModelRule::Manual { .. })
}

/// TS `matchesRule`：`new RegExp("^(?:" + pattern + ")$")` 全匹配。
/// JS-only 语法（lookahead/backref）在 Rust regex 不合法 → 判不命中（spec 已知差异）。
/// 规则正则缓存：pattern+大小写 → 编译结果（非法 pattern 缓存 None）。
type RuleRegexCache = Mutex<HashMap<(String, bool), Option<regex::Regex>>>;

fn matches_rule(pattern: &str, value: &str, ignore_case: bool) -> bool {
    // TS 对每条规则 `new RegExp(...)`；Rust 若照抄会为每个 provider×model×rule
    // 重新编译（实测 14s/次 resolve）。规则集合是只读的，按 pattern+大小写缓存编译结果。
    static CACHE: OnceLock<RuleRegexCache> = OnceLock::new();
    let anchored = format!("^(?:{pattern})$");
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let compiled = guard
        .entry((anchored.clone(), ignore_case))
        .or_insert_with(|| {
            regex::RegexBuilder::new(&anchored)
                .case_insensitive(ignore_case)
                .build()
                .ok()
        });
    compiled.as_ref().is_some_and(|regex| regex.is_match(value))
}

/// TS `normalizeBaseURLForRuleMatch`：host 大小写/默认端口/尾斜杠不影响命中，
/// 保留 path/query/fragment 原样。
fn normalize_base_url_for_rule_match(value: &str) -> Option<String> {
    let parsed = url::Url::parse(value).ok()?;
    let mut suffix = String::new();
    if let Some(query) = parsed.query() {
        suffix.push('?');
        suffix.push_str(query);
    }
    if let Some(fragment) = parsed.fragment() {
        suffix.push('#');
        suffix.push_str(fragment);
    }
    let serialized = parsed.to_string();
    let endpoint = if suffix.is_empty() {
        serialized
    } else {
        serialized[..serialized.len() - suffix.len()].to_string()
    };
    Some(format!("{}{}", endpoint.trim_end_matches('/'), suffix))
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    use crate::value::Field;

    fn config_with_context_window(context_window: i64) -> ModelConfig {
        ModelConfig {
            properties: Field::Value(crate::model::ModelPropertiesConfig {
                context_window: Field::Value(context_window),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn input<'a>(provider_id: &'a str, model_id: &'a str) -> ModelConfigRuleResolutionInput<'a> {
        ModelConfigRuleResolutionInput {
            provider_id,
            template_id: None,
            model_id,
            api_type: None,
            base_url: None,
        }
    }

    #[test]
    fn compose_effective_keeps_builtin_and_personal_exact_only() {
        let builtin = ModelConfigRules::new(vec![ModelRule::Model {
            model_match: ".*".to_string(),
            config: config_with_context_window(1),
        }]);
        let personal = ModelConfigRules::new(vec![
            ModelRule::Provider {
                provider_id: "p".to_string(),
                model_id: "m".to_string(),
                config: config_with_context_window(2),
            },
            ModelRule::Model {
                model_match: ".*".to_string(),
                config: config_with_context_window(3),
            },
        ]);
        let composed = ModelConfigRules::compose_effective(&builtin, &personal);
        assert_eq!(composed.rules().len(), 2);
        assert!(matches!(composed.rules()[1], ModelRule::Provider { .. }));
    }

    #[test]
    fn resolve_applies_rules_in_declaration_order() {
        let rules = ModelConfigRules::new(vec![
            ModelRule::Model {
                model_match: "GPT-.*".to_string(),
                config: config_with_context_window(1),
            },
            ModelRule::Provider {
                provider_id: "p".to_string(),
                model_id: "m".to_string(),
                config: config_with_context_window(2),
            },
            ModelRule::ModelApi {
                model_match: "m".to_string(),
                api_type_match: "openai.*".to_string(),
                config: config_with_context_window(3),
            },
        ]);
        // model match 忽略大小写；provider 精确规则覆盖推荐规则；ModelApi 需 apiType。
        let resolved = rules.resolve(&input("p", "m"));
        assert_eq!(
            resolved
                .properties
                .value()
                .and_then(|p| p.context_window.value().copied()),
            Some(2)
        );
        let with_api = ModelConfigRuleResolutionInput {
            api_type: Some("openai-chat-completions"),
            ..input("p", "m")
        };
        // 顺序：model(1) → provider(2) → model-api(3)。
        assert_eq!(
            rules
                .resolve(&with_api)
                .properties
                .value()
                .and_then(|p| p.context_window.value().copied()),
            Some(3)
        );
    }

    #[test]
    fn resolve_provider_site_normalizes_base_url() {
        let rules = ModelConfigRules::new(vec![ModelRule::ProviderSite {
            model_match: "m".to_string(),
            base_url_match: r"https://api\.example\.com/v1".to_string(),
            api_type_match: None,
            config: config_with_context_window(9),
        }]);
        let hit = ModelConfigRuleResolutionInput {
            base_url: Some("https://API.example.com:443/v1/"),
            ..input("p", "m")
        };
        assert_eq!(
            rules
                .resolve(&hit)
                .properties
                .value()
                .and_then(|p| p.context_window.value().copied()),
            Some(9)
        );
        let miss = ModelConfigRuleResolutionInput {
            base_url: Some("https://other.example.com/v1"),
            ..input("p", "m")
        };
        assert_eq!(rules.resolve(&miss).properties, Field::Absent);
    }

    #[test]
    fn manual_rule_clears_inherited_model_fields_first() {
        let rules = ModelConfigRules::new(vec![
            ModelRule::Model {
                model_match: ".*".to_string(),
                config: config_with_context_window(1),
            },
            ModelRule::Manual {
                provider_id: "p".to_string(),
                model_id: "m".to_string(),
                config: ModelConfig {
                    enabled: Field::Value(true),
                    ..Default::default()
                },
            },
        ]);
        let resolved = rules.resolve(&input("p", "m"));
        // 手动规则自带完整字段：先清掉继承的 properties，再 overlay。
        assert_eq!(resolved.properties, Field::Absent);
        assert_eq!(resolved.enabled, Field::Value(true));
    }

    #[test]
    fn normalize_base_url_matches_js_serialization() {
        assert_eq!(
            normalize_base_url_for_rule_match("https://API.example.com:443/v1/").as_deref(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(
            normalize_base_url_for_rule_match("https://x.example.com/v1?b=2").as_deref(),
            Some("https://x.example.com/v1?b=2")
        );
        assert_eq!(normalize_base_url_for_rule_match("not a url"), None);
    }

    #[test]
    fn js_only_regex_syntax_does_not_match() {
        // lookahead 在 Rust regex 不合法：判不命中而不是 panic。
        assert!(!matches_rule("(?=m)m", "m", true));
        assert!(matches_rule("m.*", "M1", true));
    }
}

#[cfg(test)]
mod exact_rule_tests {
    use super::*;
    use crate::model::{LimitOptionSpecConfig, ModelOptionSpecsConfig};

    fn config_with_max(max: i64) -> ModelConfig {
        ModelConfig {
            option_specs: Field::Value(ModelOptionSpecsConfig {
                reasoning_level: Field::Absent,
                max_output_tokens: Field::Value(LimitOptionSpecConfig {
                    max: Field::Value(max),
                    map: Field::Absent,
                }),
            }),
            ..ModelConfig::default()
        }
    }

    #[test]
    fn exact_rule_lookup_matches_last_rule_and_overlays() {
        let rules = ModelConfigRules::new(vec![
            ModelRule::Provider {
                provider_id: "p1".to_string(),
                model_id: "m1".to_string(),
                config: config_with_max(100),
            },
            ModelRule::Provider {
                provider_id: "p1".to_string(),
                model_id: "other".to_string(),
                config: config_with_max(999),
            },
            ModelRule::Manual {
                provider_id: "p1".to_string(),
                model_id: "m1".to_string(),
                config: ModelConfig {
                    enabled: Field::Value(false),
                    ..ModelConfig::default()
                },
            },
        ]);

        // getExact：按序叠加（后者覆盖前者字段）。
        let merged = rules.get_exact("p1", "m1").expect("exact");
        assert_eq!(merged.enabled, Field::Value(false));
        let Field::Value(specs) = &merged.option_specs else {
            panic!("option specs");
        };
        let Field::Value(tokens) = &specs.max_output_tokens else {
            panic!("tokens");
        };
        assert_eq!(tokens.max, Field::Value(100));

        // getExactRule：最后一条精确规则的类型。
        let (kind, config) = rules.get_exact_rule("p1", "m1").expect("rule");
        assert_eq!(kind, ExactModelRuleKind::Manual);
        assert_eq!(config.enabled, Field::Value(false));

        assert!(rules.get_exact("p1", "missing").is_none());
        assert!(rules.get_exact_rule("p1", "missing").is_none());
    }
}

#[cfg(test)]
mod delete_exact_tests {
    use super::*;

    #[test]
    fn delete_exact_for_provider_keeps_non_exact_rules() {
        let rules = ModelConfigRules::new(vec![
            ModelRule::Provider {
                provider_id: "p1".to_string(),
                model_id: "m1".to_string(),
                config: ModelConfig::default(),
            },
            ModelRule::Manual {
                provider_id: "p1".to_string(),
                model_id: "m2".to_string(),
                config: ModelConfig::default(),
            },
            ModelRule::Provider {
                provider_id: "p2".to_string(),
                model_id: "m1".to_string(),
                config: ModelConfig::default(),
            },
            ModelRule::Template {
                template_id: "t1".to_string(),
                model_id: "m1".to_string(),
                config: ModelConfig::default(),
            },
            ModelRule::Model {
                model_match: "m.*".to_string(),
                config: ModelConfig::default(),
            },
        ]);
        let trimmed = rules.delete_exact_for_provider("p1");
        assert!(trimmed.get_exact_rule("p1", "m1").is_none());
        assert!(trimmed.get_exact_rule("p1", "m2").is_none());
        assert!(trimmed.get_exact_rule("p2", "m1").is_some());
        // 非精确规则保留：Template/Model 共 2 条 + p2 Provider 1 条。
        assert_eq!(trimmed.rules().len(), 3);
    }
}
#[cfg(test)]
mod exact_rule_mutation_tests {
    use super::*;

    fn exact(provider_id: &str, model_id: &str) -> ModelRule {
        ModelRule::Provider {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            config: ModelConfig::default(),
        }
    }

    #[test]
    fn set_exact_replaces_first_and_can_switch_to_manual() {
        let rules = ModelConfigRules::new(vec![
            exact("p1", "m1"),
            exact("p1", "m1"),
            exact("p2", "m1"),
        ]);
        let config = ModelConfig {
            enabled: Field::Value(false),
            ..ModelConfig::default()
        };
        let updated = rules.set_exact("p1", "m1", config, Some(false));
        assert_eq!(updated.rules().len(), 2, "重复精确规则只留一条");
        let (kind, saved) = updated.get_exact_rule("p1", "m1").expect("rule");
        assert_eq!(kind, ExactModelRuleKind::Manual);
        assert_eq!(saved.enabled, Field::Value(false));
        assert!(updated.get_exact_rule("p2", "m1").is_some());

        // 新 pair 追加，默认沿用 previous（无 → Provider）。
        let appended = updated.set_exact("p1", "m2", ModelConfig::default(), None);
        assert_eq!(
            appended.get_exact_rule("p1", "m2").expect("rule").0,
            ExactModelRuleKind::Provider
        );
    }

    #[test]
    fn delete_and_rename_exact_rules() {
        let rules = ModelConfigRules::new(vec![exact("p1", "m1"), exact("p1", "m2")]);
        let renamed = rules.rename_exact_model("p1", "m1", "m9");
        assert!(renamed.get_exact_rule("p1", "m1").is_none());
        assert!(renamed.get_exact_rule("p1", "m9").is_some());
        let deleted = renamed.delete_exact("p1", "m9");
        assert!(deleted.get_exact_rule("p1", "m9").is_none());
        assert!(deleted.get_exact_rule("p1", "m2").is_some());
        // 同名 rename 幂等。
        assert_eq!(
            deleted.rename_exact_model("p1", "m2", "m2").rules().len(),
            1
        );
    }
}
