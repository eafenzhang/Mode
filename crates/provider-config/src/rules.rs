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
    check_unknown_keys(map, &["providerModelRules", "manualProviderModelRules"], path, &mut issues);
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }

    let provider_rules = parse_rule_items(
        map,
        "providerModelRules",
        path,
        parse_provider_model_rule,
    )?;
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
                        vec![
                            "manualProviderModelRules".to_owned(),
                            index.to_string(),
                        ],
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
    check_unknown_keys(map, &["modelMatch", "config", "apiTypeMatch"], path, &mut issues);

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
    let api_type_match =
        parse_required_field(map, "apiTypeMatch", path, "string", parse_pattern).unwrap_or_else(
            |mut errors| {
                issues.append(&mut errors);
                String::new()
            },
        );

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
    let base_url_match =
        parse_required_field(map, "baseUrlMatch", path, "string", parse_pattern).unwrap_or_else(
            |mut errors| {
                issues.append(&mut errors);
                String::new()
            },
        );
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
