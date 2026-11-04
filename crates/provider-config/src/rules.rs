//! 模型规则容器（六类规则）与内置/个人规则列表。
//!
//! 真源：`packages/provider/src/config/model-config.ts` 的 `ModelConfigRules`、
//! `packages/provider/src/config/rule-data-schema.ts`、
//! `packages/provider/src/config/schema.ts`。

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
fn matches_rule(pattern: &str, value: &str, ignore_case: bool) -> bool {
    match regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
        .case_insensitive(ignore_case)
        .build()
    {
        Ok(regex) => regex.is_match(value),
        Err(_) => false,
    }
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
