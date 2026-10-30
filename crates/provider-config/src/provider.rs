//! Provider 配置模型：access / api / ProviderConfig / Template 与规则容器。
//!
//! 真源：`packages/provider/src/config/provider-config.ts`、
//! `packages/provider/src/config/provider-data-schema.ts`、
//! `packages/provider/src/config/rule-data-schema.ts`、
//! `packages/provider/src/config/schema.ts`。

use indexmap::IndexMap;

use crate::json::{array, boolean, object, string, JsonValue};
use crate::model::translate_issues;
use crate::url::is_valid_url;
use crate::value::{
    check_unknown_keys, expect_object, parse_bool, parse_non_blank_string, parse_non_empty_string,
    parse_optional_field, parse_optional_non_null_field, parse_required_field, parse_string,
    parse_string_array, parse_string_record, Field, Issue, ParseError, ParseResult,
};

// ---------------------------------------------------------------------------
// 枚举
// ---------------------------------------------------------------------------

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $value),+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($value => Some($name::$variant),)+
                    _ => None,
                }
            }

            #[allow(dead_code)]
            fn values() -> &'static [&'static str] {
                &[$($value),+]
            }
        }
    };
}

string_enum!(ProviderGroup {
    StandardPersonal => "standard-personal",
    ZaiFamily => "zai-family",
    BigmodelFamily => "bigmodel-family",
});
string_enum!(ProviderVisibility {
    Visible => "visible",
    Hidden => "hidden",
});
string_enum!(ProviderApiType {
    AnthropicMessages => "anthropic-messages",
    OpenAiChatCompletions => "openai-chat-completions",
    OpenAiResponses => "openai-responses",
});
string_enum!(ZhipuAccountMode {
    StartPlan => "start-plan",
    IndividualCodingPlan => "individual-coding-plan",
    TeamCodingPlan => "team-coding-plan",
    OffPeak => "off-peak",
});
string_enum!(ZhipuAccountType {
    Zai => "zai",
    Bigmodel => "bigmodel",
});
string_enum!(ApiKeyAccessType {
    ApiKey => "api-key",
    ZhipuCodingPlanApiKey => "zhipu-coding-plan-api-key",
});

// ---------------------------------------------------------------------------
// Access / API / Logo
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyAccess {
    pub access_type: ApiKeyAccessType,
    pub api_key: Field<String>,
    pub api_key_management_url: Field<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZhipuAccountAccess {
    pub account_type: Field<ZhipuAccountType>,
    pub mode: Field<ZhipuAccountMode>,
    pub entitled: Field<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderAccess {
    ApiKey(ApiKeyAccess),
    ZhipuAccount(ZhipuAccountAccess),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderApiConfig {
    pub api_type: Field<ProviderApiType>,
    pub base_url: Field<String>,
    pub headers: Field<IndexMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLogo {
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderConfig {
    pub group: Field<ProviderGroup>,
    pub logo: Field<ProviderLogo>,
    pub access: Field<ProviderAccess>,
    pub api: Field<ProviderApiConfig>,
    pub builtin_model_ids: Field<Vec<String>>,
    pub personal_model_ids: Field<Vec<String>>,
    pub model_order: Field<Vec<String>>,
    pub visibility: Field<ProviderVisibility>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderTemplateNameMap {
    pub zh_cn: Option<String>,
    pub en_us: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTemplate {
    pub template_id: String,
    pub template_name_map: ProviderTemplateNameMap,
    pub config: ProviderConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigRule {
    pub provider_id: String,
    pub template_id: Field<String>,
    pub provider_name: Field<String>,
    pub enabled: Field<bool>,
    pub config: ProviderConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderConfigMap {
    pub values: IndexMap<String, ProviderConfigRule>,
}

impl ProviderConfigMap {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn get(&self, provider_id: &str) -> Option<&ProviderConfigRule> {
        self.values.get(provider_id)
    }

    pub fn keys(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }

    pub fn entries(&self) -> impl Iterator<Item = (&String, &ProviderConfigRule)> {
        self.values.iter()
    }

    pub fn to_json(&self) -> JsonValue {
        array(self.values.values().map(rule_to_json).collect())
    }

    /// `validateComplete`：逐 Provider 在自身路径下验证。
    pub fn validate_complete(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        for (provider_id, rule) in &self.values {
            let base = vec![provider_id.clone()];
            issues.extend(rule.config.validate_complete_at(&base));
        }
        issues
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderTemplateMap {
    pub values: IndexMap<String, ProviderTemplate>,
}

impl ProviderTemplateMap {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn get(&self, template_id: &str) -> Option<&ProviderTemplate> {
        self.values.get(template_id)
    }

    pub fn keys(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }

    pub fn to_json(&self) -> JsonValue {
        array(self.values.values().map(template_to_json).collect())
    }

    /// `TemplateMap.validateComplete()` 恒为空。
    pub fn validate_complete(&self) -> Vec<Issue> {
        Vec::new()
    }
}

impl ProviderConfig {
    pub fn to_json(&self) -> JsonValue {
        let mut entries = Vec::new();
        push_field(&mut entries, "group", &self.group, |value| {
            string(value.as_str())
        });        push_field(&mut entries, "logo", &self.logo, |value| {
            object([("type", string("builtin")), ("key", string(&value.key))])
        });
        push_field(&mut entries, "access", &self.access, access_to_json);
        push_field(&mut entries, "api", &self.api, api_to_json);
        push_field(&mut entries, "builtinModelIds", &self.builtin_model_ids, |values| {
            string_array_json(values)
        });
        push_field(
            &mut entries,
            "personalModelIds",
            &self.personal_model_ids,
            |values| string_array_json(values),
        );
        push_field(&mut entries, "modelOrder", &self.model_order, |values| {
            string_array_json(values)
        });
        push_field(&mut entries, "visibility", &self.visibility, |value| {
            string(value.as_str())
        });
        object_entries(entries)
    }

    pub fn validate_complete(&self) -> Vec<Issue> {
        self.validate_complete_at(&[])
    }

    pub fn validate_complete_at(&self, base: &[String]) -> Vec<Issue> {
        let value = self.to_json();
        match parse_provider_config_value(&value, base, ProviderMode::Complete) {
            Ok(_) => Vec::new(),
            Err(ProviderParseError::Issues(issues)) => {
                crate::model::translate_issues_at(&value, base.len(), issues, false)
            }
            Err(ProviderParseError::Invariant(message)) => {
                vec![Issue::new("invalid-config", base.to_vec(), message)]
            }
        }
    }
}

impl ProviderApiConfig {
    pub fn to_json(&self) -> JsonValue {
        api_to_json(self)
    }

    pub fn validate_complete(&self) -> Vec<Issue> {
        let value = api_to_json(self);
        match parse_api(&value, &[], ApiMode::Complete) {
            Ok(_) => Vec::new(),
            Err(issues) => translate_issues(&value, issues, false),
        }
    }
}

impl ProviderTemplate {
    pub fn to_json(&self) -> JsonValue {
        template_to_json(self)
    }
}

impl ProviderConfigRule {
    pub fn to_json(&self) -> JsonValue {
        rule_to_json(self)
    }
}

// ---------------------------------------------------------------------------
// Provider 解析
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderMode {
    Sparse,
    Complete,
    Builtin,
    Personal,
    Template,
    Account,
}

enum ProviderParseError {
    Issues(Vec<Issue>),
    Invariant(String),
}

impl From<Vec<Issue>> for ProviderParseError {
    fn from(issues: Vec<Issue>) -> Self {
        ProviderParseError::Issues(issues)
    }
}

fn issues_error(issues: Vec<Issue>) -> ParseError {
    ParseError::Issues(issues)
}

impl From<ProviderParseError> for ParseError {
    fn from(error: ProviderParseError) -> Self {
        match error {
            ProviderParseError::Issues(issues) => ParseError::Issues(issues),
            ProviderParseError::Invariant(message) => ParseError::Invariant(message),
        }
    }
}

pub fn parse_provider_config(value: &JsonValue, path: &[String]) -> ParseResult<ProviderConfig> {
    parse_provider_config_value(value, path, ProviderMode::Sparse).map_err(ParseError::from)
}

pub fn parse_provider_config_map(value: &JsonValue, path: &[String]) -> ParseResult<ProviderConfigMap> {
    let entries = value
        .as_array()
        .ok_or_else(|| issues_error(vec![Issue::invalid_type(path, "array", value.kind())]))?;
    let mut rules = Vec::with_capacity(entries.len());
    let mut issues = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let mut entry_path = path.to_vec();
        entry_path.push(index.to_string());
        match parse_provider_rule(entry, &entry_path, ProviderMode::Sparse) {
            Ok(rule) => rules.push(rule),
            Err(ProviderParseError::Issues(mut errors)) => issues.append(&mut errors),
            Err(ProviderParseError::Invariant(message)) => return Err(ParseError::Invariant(message)),
        }
    }
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }
    build_provider_map(rules)
}

pub fn parse_mode_builtin_provider_config_rules(
    value: &JsonValue,
    path: &[String],
) -> ParseResult<(ProviderConfigMap, ProviderTemplateMap)> {
    let map = expect_object(value, path).map_err(issues_error)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["templateRules", "providerRules"], path, &mut issues);
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }

    let template_rules = parse_rule_array(map, "templateRules", path, |entry, entry_path| {
        parse_template_rule(entry, entry_path)
    })?;
    let provider_rules = parse_rule_array(map, "providerRules", path, |entry, entry_path| {
        parse_provider_rule(entry, entry_path, ProviderMode::Builtin)
    })?;

    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }
    check_unique_ids(
        template_rules.iter().map(|rule| rule.template_id.as_str()),
        "templateRules",
        "templateId",
        &mut issues,
    );
    check_unique_ids(
        provider_rules.iter().map(|rule| rule.provider_id.as_str()),
        "providerRules",
        "providerId",
        &mut issues,
    );
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }

    let providers = build_provider_map(provider_rules)?;
    let templates = build_template_map(template_rules)?;
    Ok((providers, templates))
}

pub fn parse_personal_provider_config_map(
    value: &JsonValue,
    path: &[String],
) -> ParseResult<ProviderConfigMap> {
    let map = expect_object(value, path).map_err(issues_error)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["providerRules"], path, &mut issues);
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }
    let mut rules = parse_rule_array(map, "providerRules", path, |entry, entry_path| {
        parse_personal_provider_rule(entry, entry_path)
    })?;
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }
    check_unique_ids(
        rules.iter().map(|rule| rule.provider_id.as_str()),
        "providerRules",
        "providerId",
        &mut issues,
    );
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }
    build_provider_map(std::mem::take(&mut rules))
}

pub fn parse_provider_template_map(
    value: &JsonValue,
    path: &[String],
) -> ParseResult<ProviderTemplateMap> {
    let entries = value
        .as_array()
        .ok_or_else(|| issues_error(vec![Issue::invalid_type(path, "array", value.kind())]))?;
    let mut rules = Vec::with_capacity(entries.len());
    let mut issues = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let mut entry_path = path.to_vec();
        entry_path.push(index.to_string());
        match parse_template_rule(entry, &entry_path) {
            Ok(rule) => rules.push(rule),
            Err(ProviderParseError::Issues(mut errors)) => issues.append(&mut errors),
            Err(ProviderParseError::Invariant(message)) => return Err(ParseError::Invariant(message)),
        }
    }
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }
    build_template_map(rules)
}

pub fn parse_account_provider_config_map(
    value: &JsonValue,
    path: &[String],
) -> ParseResult<ProviderConfigMap> {
    let map = expect_object(value, path).map_err(issues_error)?;
    let mut rules = Vec::with_capacity(map.len());
    let mut issues = Vec::new();
    for (provider_id, entry) in map {
        let mut entry_path = path.to_vec();
        entry_path.push(provider_id.clone());
        if provider_id.is_empty() {
            issues.push(Issue::too_small(
                std::slice::from_ref(provider_id),
                ">=1 character(s)",
            ));
            continue;
        }
        match parse_provider_config_value(entry, &entry_path, ProviderMode::Account) {
            Ok(config) => rules.push(ProviderConfigRule {
                provider_id: provider_id.clone(),
                template_id: Field::Absent,
                provider_name: Field::Absent,
                enabled: Field::Absent,
                config,
            }),
            Err(ProviderParseError::Issues(mut errors)) => issues.append(&mut errors),
            Err(ProviderParseError::Invariant(message)) => return Err(ParseError::Invariant(message)),
        }
    }
    if !issues.is_empty() {
        return Err(ParseError::Issues(issues));
    }
    build_provider_map(rules)
}

fn parse_rule_array<T>(
    map: &IndexMap<String, JsonValue>,
    key: &str,
    path: &[String],
    parser: impl Fn(&JsonValue, &[String]) -> Result<T, ProviderParseError>,
) -> Result<Vec<T>, ProviderParseError> {
    let mut field_path = path.to_vec();
    field_path.push(key.to_owned());
    let Some(value) = map.get(key) else {
        return Err(ProviderParseError::Issues(vec![Issue::invalid_type(
            &field_path,
            "array",
            "undefined",
        )]));
    };
    let Some(entries) = value.as_array() else {
        return Err(ProviderParseError::Issues(vec![Issue::invalid_type(
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
            Ok(item) => result.push(item),
            Err(ProviderParseError::Issues(mut errors)) => issues.append(&mut errors),
            Err(ProviderParseError::Invariant(message)) => {
                return Err(ProviderParseError::Invariant(message))
            }
        }
    }
    if issues.is_empty() {
        Ok(result)
    } else {
        Err(ProviderParseError::Issues(issues))
    }
}

fn parse_provider_rule(
    value: &JsonValue,
    path: &[String],
    mode: ProviderMode,
) -> Result<ProviderConfigRule, ProviderParseError> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(
        map,
        &["providerId", "templateId", "providerName", "enabled", "config"],
        path,
        &mut issues,
    );

    let provider_id = parse_required_field(map, "providerId", path, "string", parse_non_empty_string)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            String::new()
        });
    let template_id = parse_optional_field(map, "templateId", path, parse_non_empty_string)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        });
    let provider_name = parse_optional_field(map, "providerName", path, parse_non_empty_string)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        });
    let enabled =
        parse_optional_non_null_field(map, "enabled", path, "boolean", parse_bool).unwrap_or_else(
            |mut errors| {
                issues.append(&mut errors);
                Field::Absent
            },
        );
    let config = parse_required_field(map, "config", path, "object", |value, field_path| {
        parse_provider_config_value(value, field_path, mode).map_err(|error| match error {
            ProviderParseError::Issues(issues) => issues,
            ProviderParseError::Invariant(message) => {
                vec![Issue::new("invalid-config", field_path.to_vec(), message)]
            }
        })
    })
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        ProviderConfig::default()
    });

    if !issues.is_empty() {
        return Err(ProviderParseError::Issues(issues));
    }
    Ok(ProviderConfigRule {
        provider_id,
        template_id,
        provider_name,
        enabled,
        config,
    })
}

fn parse_personal_provider_rule(
    value: &JsonValue,
    path: &[String],
) -> Result<ProviderConfigRule, ProviderParseError> {
    let provider_id = value
        .as_object()
        .and_then(|map| map.get("providerId"))
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .unwrap_or_default();
    let access_present = value
        .as_object()
        .and_then(|map| map.get("config"))
        .and_then(JsonValue::as_object)
        .map(|config| config.contains_key("access"))
        .unwrap_or(false);

    let rule = parse_provider_rule(value, path, ProviderMode::Personal)?;
    if provider_id.starts_with("account:") && access_present {
        return Err(ProviderParseError::Issues(vec![Issue::custom(
            &[path.to_vec(), vec!["config".to_owned(), "access".to_owned()]].concat(),
            "固定 Account Provider 的 Access 只能由 Mode Built-in Config 声明",
        )]));
    }
    Ok(rule)
}

fn parse_template_rule(
    value: &JsonValue,
    path: &[String],
) -> Result<ProviderTemplate, ProviderParseError> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["templateId", "templateNameMap", "config"], path, &mut issues);

    let template_id = parse_required_field(map, "templateId", path, "string", parse_non_empty_string)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            String::new()
        });
    let template_name_map = parse_required_field(
        map,
        "templateNameMap",
        path,
        "object",
        parse_template_name_map,
    )
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        ProviderTemplateNameMap::default()
    });
    let config = parse_required_field(map, "config", path, "object", |value, field_path| {
        parse_provider_config_value(value, field_path, ProviderMode::Template).map_err(|error| {
            match error {
                ProviderParseError::Issues(issues) => issues,
                ProviderParseError::Invariant(message) => {
                    vec![Issue::new("invalid-config", field_path.to_vec(), message)]
                }
            }
        })
    })
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        ProviderConfig::default()
    });

    if !issues.is_empty() {
        return Err(ProviderParseError::Issues(issues));
    }
    Ok(ProviderTemplate {
        template_id,
        template_name_map,
        config,
    })
}

fn parse_template_name_map(
    value: &JsonValue,
    path: &[String],
) -> Result<ProviderTemplateNameMap, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["zh-CN", "en-US"], path, &mut issues);
    let zh_cn =
        parse_optional_non_null_field(map, "zh-CN", path, "string", parse_non_empty_string)
            .unwrap_or_else(|mut errors| {
                issues.append(&mut errors);
                Field::Absent
            });
    let en_us =
        parse_optional_non_null_field(map, "en-US", path, "string", parse_non_empty_string)
            .unwrap_or_else(|mut errors| {
                issues.append(&mut errors);
                Field::Absent
            });
    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(ProviderTemplateNameMap {
        zh_cn: zh_cn.into_value(),
        en_us: en_us.into_value(),
    })
}

fn parse_provider_config_value(
    value: &JsonValue,
    path: &[String],
    mode: ProviderMode,
) -> Result<ProviderConfig, ProviderParseError> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    let keys: &[&str] = match mode {
        ProviderMode::Sparse | ProviderMode::Complete => &[
            "group",
            "logo",
            "access",
            "api",
            "builtinModelIds",
            "personalModelIds",
            "modelOrder",
            "visibility",
        ],
        ProviderMode::Builtin => &[
            "group",
            "logo",
            "access",
            "api",
            "builtinModelIds",
            "visibility",
        ],
        ProviderMode::Personal => &[
            "group",
            "logo",
            "access",
            "api",
            "personalModelIds",
            "modelOrder",
            "visibility",
        ],
        ProviderMode::Template => &["logo", "access", "api", "builtinModelIds"],
        ProviderMode::Account => &["builtinModelIds", "access"],
    };
    check_unknown_keys(map, keys, path, &mut issues);

    let group = if matches!(mode, ProviderMode::Template | ProviderMode::Account) {
        Field::Absent
    } else {
        let required = matches!(mode, ProviderMode::Complete | ProviderMode::Builtin);
        let constraint = match mode {
            ProviderMode::Builtin => GroupConstraint::Builtin,
            ProviderMode::Personal => GroupConstraint::Personal,
            _ => GroupConstraint::Any,
        };
        let parser = |value: &JsonValue, field_path: &[String]| {
            parse_group(value, field_path, constraint)
        };
        if required {
            parse_required_field(map, "group", path, "string", parser)
                .map(Field::Value)
                .unwrap_or_else(|mut errors| {
                    issues.append(&mut errors);
                    Field::Absent
                })
        } else {
            parse_optional_field(map, "group", path, parser).unwrap_or_else(|mut errors| {
                issues.append(&mut errors);
                Field::Absent
            })
        }
    };

    let logo = if matches!(mode, ProviderMode::Account) {
        Field::Absent
    } else {
        parse_optional_field(map, "logo", path, parse_logo).unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        })
    };

    let access_required = mode == ProviderMode::Complete;
    let access_mode = match mode {
        ProviderMode::Complete => AccessMode::Complete,
        ProviderMode::Template => AccessMode::Template,
        ProviderMode::Account => AccessMode::Account,
        _ => AccessMode::Sparse,
    };
    let access = if access_required {
        parse_required_field(map, "access", path, "object", |value, field_path| {
            parse_access(value, field_path, access_mode)
        })
        .map(Field::Value)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        })
    } else {
        parse_optional_field(map, "access", path, |value, field_path| {
            parse_access(value, field_path, access_mode)
        })
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        })
    };

    let api_mode = match mode {
        ProviderMode::Complete => ApiMode::Complete,
        ProviderMode::Personal => ApiMode::Personal,
        _ => ApiMode::Sparse,
    };
    let api = if mode == ProviderMode::Account {
        Field::Absent
    } else if mode == ProviderMode::Complete {
        parse_required_field(map, "api", path, "object", |value, field_path| {
            parse_api(value, field_path, api_mode)
        })
        .map(Field::Value)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        })
    } else {
        parse_optional_field(map, "api", path, |value, field_path| {
            parse_api(value, field_path, api_mode)
        })
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        })
    };

    let builtin_model_ids = if matches!(
        mode,
        ProviderMode::Sparse
            | ProviderMode::Complete
            | ProviderMode::Builtin
            | ProviderMode::Template
            | ProviderMode::Account
    ) {
        parse_optional_field(map, "builtinModelIds", path, parse_string_array)
            .unwrap_or_else(|mut errors| {
                issues.append(&mut errors);
                Field::Absent
            })
    } else {
        Field::Absent
    };
    let personal_model_ids = if matches!(mode, ProviderMode::Sparse | ProviderMode::Complete | ProviderMode::Personal)
    {
        parse_optional_field(map, "personalModelIds", path, parse_string_array).unwrap_or_else(
            |mut errors| {
                issues.append(&mut errors);
                Field::Absent
            },
        )
    } else {
        Field::Absent
    };
    let model_order = if matches!(mode, ProviderMode::Sparse | ProviderMode::Complete | ProviderMode::Personal) {
        parse_optional_field(map, "modelOrder", path, parse_string_array).unwrap_or_else(
            |mut errors| {
                issues.append(&mut errors);
                Field::Absent
            },
        )
    } else {
        Field::Absent
    };
    let visibility = if matches!(
        mode,
        ProviderMode::Sparse
            | ProviderMode::Complete
            | ProviderMode::Builtin
            | ProviderMode::Personal
    ) {
        parse_optional_field(map, "visibility", path, |value, field_path| {
            parse_enum::<ProviderVisibility>(value, field_path, ProviderVisibility::values())
        })
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        })
    } else {
        Field::Absent
    };

    if !issues.is_empty() {
        return Err(ProviderParseError::Issues(issues));
    }

    let mut config = ProviderConfig {
        group,
        logo,
        access,
        api,
        builtin_model_ids,
        personal_model_ids,
        model_order,
        visibility,
    };
    normalize_model_ids(&mut config)?;
    Ok(config)
}

fn normalize_model_ids(config: &mut ProviderConfig) -> Result<(), ProviderParseError> {
    for field in [
        &mut config.builtin_model_ids,
        &mut config.personal_model_ids,
        &mut config.model_order,
    ] {
        if let Field::Value(ids) = field {
            for id in ids.iter_mut() {
                *id = id.trim().to_owned();
                if id.is_empty() {
                    return Err(ProviderParseError::Invariant(
                        "Model ID 不能为空".to_owned(),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum GroupConstraint {
    Any,
    Builtin,
    Personal,
}

fn parse_group(
    value: &JsonValue,
    path: &[String],
    constraint: GroupConstraint,
) -> Result<ProviderGroup, Vec<Issue>> {
    let text = parse_string(value, path)?;
    let group = ProviderGroup::parse(&text);
    let allowed = match (group, constraint) {
        (Some(group), GroupConstraint::Any) => Some(group),
        (Some(group @ (ProviderGroup::ZaiFamily | ProviderGroup::BigmodelFamily)), GroupConstraint::Builtin) => {
            Some(group)
        }
        (Some(ProviderGroup::StandardPersonal), GroupConstraint::Personal) => {
            Some(ProviderGroup::StandardPersonal)
        }
        _ => None,
    };
    match allowed {
        Some(group) => Ok(group),
        None => {
            let values: &[&str] = match constraint {
                GroupConstraint::Any => ProviderGroup::values(),
                GroupConstraint::Builtin => &["zai-family", "bigmodel-family"],
                GroupConstraint::Personal => &["standard-personal"],
            };
            Err(vec![Issue::invalid_value(path, values, format!("{text:?}"))])
        }
    }
}

fn parse_logo(value: &JsonValue, path: &[String]) -> Result<ProviderLogo, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["type", "key"], path, &mut issues);
    let type_value = parse_required_field(map, "type", path, "string", parse_string).unwrap_or_else(
        |mut errors| {
            issues.append(&mut errors);
            String::new()
        },
    );
    if type_value != "builtin" {
        let mut type_path = path.to_vec();
        type_path.push("type".to_owned());
        issues.push(Issue::invalid_value(
            &type_path,
            &["builtin"],
            format!("{type_value:?}"),
        ));
    }
    let key = parse_required_field(map, "key", path, "string", parse_non_empty_string).unwrap_or_else(
        |mut errors| {
            issues.append(&mut errors);
            String::new()
        },
    );
    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(ProviderLogo { key })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AccessMode {
    Sparse,
    Complete,
    Template,
    Account,
}

fn parse_access(
    value: &JsonValue,
    path: &[String],
    mode: AccessMode,
) -> Result<ProviderAccess, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let tag = map.get("type").and_then(JsonValue::as_str);
    let mut type_path = path.to_vec();
    type_path.push("type".to_owned());

    if mode == AccessMode::Account {
        if tag != Some("zhipu-account") {
            return Err(vec![Issue::invalid_value(
                &type_path,
                &["zhipu-account"],
                format!("{tag:?}"),
            )]);
        }
        let mut issues = Vec::new();
        check_unknown_keys(map, &["type", "entitled"], path, &mut issues);
        let entitled = parse_optional_field(map, "entitled", path, parse_bool).unwrap_or_else(
            |mut errors| {
                issues.append(&mut errors);
                Field::Absent
            },
        );
        if !issues.is_empty() {
            return Err(issues);
        }
        return Ok(ProviderAccess::ZhipuAccount(ZhipuAccountAccess {
            account_type: Field::Absent,
            mode: Field::Absent,
            entitled,
        }));
    }

    match tag {
        Some("api-key") | Some("zhipu-coding-plan-api-key") => {
            let access_type = ApiKeyAccessType::parse(tag.expect("已匹配")).expect("枚举");
            let template = mode == AccessMode::Template;
            let complete = mode == AccessMode::Complete;
            let mut issues = Vec::new();
            let allowed: &[&str] = if template {
                &["type", "apiKeyManagementUrl"]
            } else {
                &["type", "apiKey", "apiKeyManagementUrl"]
            };
            check_unknown_keys(map, allowed, path, &mut issues);

            let api_key = if complete {
                parse_required_field(map, "apiKey", path, "string", parse_non_blank_string)
                    .map(Field::Value)
                    .unwrap_or_else(|mut errors| {
                        issues.append(&mut errors);
                        Field::Absent
                    })
            } else if template {
                Field::Absent
            } else {
                parse_optional_field(map, "apiKey", path, parse_string).unwrap_or_else(
                    |mut errors| {
                        issues.append(&mut errors);
                        Field::Absent
                    },
                )
            };
            let api_key_management_url =
                parse_optional_field(map, "apiKeyManagementUrl", path, parse_url_string)
                    .unwrap_or_else(|mut errors| {
                        issues.append(&mut errors);
                        Field::Absent
                    });
            if !issues.is_empty() {
                return Err(issues);
            }
            Ok(ProviderAccess::ApiKey(ApiKeyAccess {
                access_type,
                api_key,
                api_key_management_url,
            }))
        }
        Some("zhipu-account") => {
            if mode == AccessMode::Template {
                return Err(vec![Issue::invalid_value(
                    &type_path,
                    &["api-key", "zhipu-coding-plan-api-key"],
                    "\"zhipu-account\"",
                )]);
            }
            let complete = mode == AccessMode::Complete;
            let mut issues = Vec::new();
            check_unknown_keys(
                map,
                &["type", "accountType", "mode", "entitled"],
                path,
                &mut issues,
            );
            let account_type = if complete {
                parse_required_field(map, "accountType", path, "string", |value, field_path| {
                    parse_enum::<ZhipuAccountType>(value, field_path, ZhipuAccountType::values())
                })
                .map(Field::Value)
                .unwrap_or_else(|mut errors| {
                    issues.append(&mut errors);
                    Field::Absent
                })
            } else {
                parse_optional_field(map, "accountType", path, |value, field_path| {
                    parse_enum::<ZhipuAccountType>(value, field_path, ZhipuAccountType::values())
                })
                .unwrap_or_else(|mut errors| {
                    issues.append(&mut errors);
                    Field::Absent
                })
            };
            let mode_field = if complete {
                parse_required_field(map, "mode", path, "string", |value, field_path| {
                    parse_enum::<ZhipuAccountMode>(value, field_path, ZhipuAccountMode::values())
                })
                .map(Field::Value)
                .unwrap_or_else(|mut errors| {
                    issues.append(&mut errors);
                    Field::Absent
                })
            } else {
                parse_optional_field(map, "mode", path, |value, field_path| {
                    parse_enum::<ZhipuAccountMode>(value, field_path, ZhipuAccountMode::values())
                })
                .unwrap_or_else(|mut errors| {
                    issues.append(&mut errors);
                    Field::Absent
                })
            };
            let entitled = if complete {
                parse_required_field(map, "entitled", path, "boolean", parse_bool)
                    .map(Field::Value)
                    .unwrap_or_else(|mut errors| {
                        issues.append(&mut errors);
                        Field::Absent
                    })
            } else {
                parse_optional_field(map, "entitled", path, parse_bool).unwrap_or_else(
                    |mut errors| {
                        issues.append(&mut errors);
                        Field::Absent
                    },
                )
            };
            if !issues.is_empty() {
                return Err(issues);
            }
            Ok(ProviderAccess::ZhipuAccount(ZhipuAccountAccess {
                account_type,
                mode: mode_field,
                entitled,
            }))
        }
        _ => Err(vec![Issue::invalid_value(
            &type_path,
            &["api-key", "zhipu-coding-plan-api-key", "zhipu-account"],
            format!("{tag:?}"),
        )]),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ApiMode {
    Sparse,
    Complete,
    Personal,
}

fn parse_api(value: &JsonValue, path: &[String], mode: ApiMode) -> Result<ProviderApiConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["type", "baseUrl", "headers"], path, &mut issues);

    let api_type = if mode == ApiMode::Complete {
        parse_required_field(map, "type", path, "string", |value, field_path| {
            parse_enum::<ProviderApiType>(value, field_path, ProviderApiType::values())
        })
        .map(Field::Value)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        })
    } else {
        parse_optional_field(map, "type", path, |value, field_path| {
            parse_enum::<ProviderApiType>(value, field_path, ProviderApiType::values())
        })
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        })
    };

    let base_url = match mode {
        ApiMode::Complete => {
            parse_required_field(map, "baseUrl", path, "string", parse_non_blank_url).map(Field::Value)
        }
        ApiMode::Personal => parse_optional_field(map, "baseUrl", path, parse_string),
        ApiMode::Sparse => parse_optional_field(map, "baseUrl", path, parse_url_string),
    }
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        Field::Absent
    });

    let headers = parse_optional_field(map, "headers", path, parse_string_record).unwrap_or_else(
        |mut errors| {
            issues.append(&mut errors);
            Field::Absent
        },
    );

    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(ProviderApiConfig {
        api_type,
        base_url,
        headers,
    })
}

fn parse_enum<T>(
    value: &JsonValue,
    path: &[String],
    values: &[&str],
) -> Result<T, Vec<Issue>>
where
    T: EnumParser,
{
    let text = parse_string(value, path)?;
    T::parse(&text).ok_or_else(|| vec![Issue::invalid_value(path, values, format!("{text:?}"))])
}

trait EnumParser: Sized {
    fn parse(value: &str) -> Option<Self>;
}

impl EnumParser for ProviderGroup {
    fn parse(value: &str) -> Option<Self> {
        ProviderGroup::parse(value)
    }
}

impl EnumParser for ProviderVisibility {
    fn parse(value: &str) -> Option<Self> {
        ProviderVisibility::parse(value)
    }
}

impl EnumParser for ProviderApiType {
    fn parse(value: &str) -> Option<Self> {
        ProviderApiType::parse(value)
    }
}

impl EnumParser for ZhipuAccountMode {
    fn parse(value: &str) -> Option<Self> {
        ZhipuAccountMode::parse(value)
    }
}

impl EnumParser for ZhipuAccountType {
    fn parse(value: &str) -> Option<Self> {
        ZhipuAccountType::parse(value)
    }
}

fn parse_url_string(value: &JsonValue, path: &[String]) -> Result<String, Vec<Issue>> {
    let text = parse_string(value, path)?;
    if is_valid_url(&text) {
        Ok(text)
    } else {
        Err(vec![Issue::new("invalid_format", path.to_vec(), "Invalid URL")])
    }
}

fn parse_non_blank_url(value: &JsonValue, path: &[String]) -> Result<String, Vec<Issue>> {
    let text = parse_non_blank_string(value, path)?;
    if is_valid_url(&text) {
        Ok(text)
    } else {
        Err(vec![Issue::new("invalid_format", path.to_vec(), "Invalid URL")])
    }
}

fn check_unique_ids<'a>(
    ids: impl Iterator<Item = &'a str>,
    group: &str,
    key: &str,
    issues: &mut Vec<Issue>,
) {
    let mut seen = std::collections::HashSet::new();
    for (index, id) in ids.enumerate() {
        if !seen.insert(id.to_owned()) {
            issues.push(Issue::new(
                "custom",
                vec![
                    group.to_owned(),
                    index.to_string(),
                    key.to_owned(),
                ],
                format!("重复 {key}: {id}"),
            ));
        }
    }
}

fn build_provider_map(rules: Vec<ProviderConfigRule>) -> ParseResult<ProviderConfigMap> {
    let mut values = IndexMap::new();
    for rule in rules {
        if values.contains_key(&rule.provider_id) {
            return Err(ParseError::Invariant(format!(
                "重复 Provider key: {}",
                rule.provider_id
            )));
        }
        values.insert(rule.provider_id.clone(), rule);
    }
    Ok(ProviderConfigMap { values })
}

fn build_template_map(rules: Vec<ProviderTemplate>) -> ParseResult<ProviderTemplateMap> {
    let mut values = IndexMap::new();
    for rule in rules {
        if values.contains_key(&rule.template_id) {
            return Err(ParseError::Invariant(format!(
                "重复 Provider Template key: {}",
                rule.template_id
            )));
        }
        values.insert(rule.template_id.clone(), rule);
    }
    Ok(ProviderTemplateMap { values })
}

// ---------------------------------------------------------------------------
// 序列化
// ---------------------------------------------------------------------------

fn access_to_json(access: &ProviderAccess) -> JsonValue {
    match access {
        ProviderAccess::ApiKey(access) => {
            let mut entries = vec![("type", string(access.access_type.as_str()))];
            if let Some(api_key) = field_string(&access.api_key) {
                entries.push(("apiKey", api_key));
            }
            if let Some(url) = field_string(&access.api_key_management_url) {
                entries.push(("apiKeyManagementUrl", url));
            }
            object_entries(entries)
        }
        ProviderAccess::ZhipuAccount(access) => {
            let mut entries = vec![("type", string("zhipu-account"))];
            if let Some(account_type) = field_enum(&access.account_type) {
                entries.push(("accountType", account_type));
            }
            if let Some(mode) = field_enum(&access.mode) {
                entries.push(("mode", mode));
            }
            if let Some(entitled) = field_bool(&access.entitled) {
                entries.push(("entitled", entitled));
            }
            object_entries(entries)
        }
    }
}

fn api_to_json(api: &ProviderApiConfig) -> JsonValue {
    let mut entries = Vec::new();
    if let Some(api_type) = field_enum(&api.api_type) {
        entries.push(("type", api_type));
    }
    if let Some(base_url) = field_string(&api.base_url) {
        entries.push(("baseUrl", base_url));
    }
    if let Some(headers) = field_string_record(&api.headers) {
        entries.push(("headers", headers));
    }
    object_entries(entries)
}

fn template_to_json(template: &ProviderTemplate) -> JsonValue {
    let mut names = IndexMap::new();
    if let Some(value) = &template.template_name_map.zh_cn {
        names.insert("zh-CN".to_owned(), string(value));
    }
    if let Some(value) = &template.template_name_map.en_us {
        names.insert("en-US".to_owned(), string(value));
    }
    object([
        ("templateId", string(&template.template_id)),
        ("templateNameMap", JsonValue::Object(names)),
        ("config", template.config.to_json()),
    ])
}

fn rule_to_json(rule: &ProviderConfigRule) -> JsonValue {
    let mut entries = vec![("providerId", string(&rule.provider_id))];
    if let Some(template_id) = field_string(&rule.template_id) {
        entries.push(("templateId", template_id));
    }
    if let Some(provider_name) = field_string(&rule.provider_name) {
        entries.push(("providerName", provider_name));
    }
    if let Some(enabled) = field_bool(&rule.enabled) {
        entries.push(("enabled", enabled));
    }
    entries.push(("config", rule.config.to_json()));
    object_entries(entries)
}

fn object_entries(entries: Vec<(&'static str, JsonValue)>) -> JsonValue {
    let mut map = IndexMap::new();
    for (key, value) in entries {
        map.insert(key.to_owned(), value);
    }
    JsonValue::Object(map)
}

fn field_bool(field: &Field<bool>) -> Option<JsonValue> {
    match field {
        Field::Absent => None,
        Field::Null => Some(JsonValue::Null),
        Field::Value(value) => Some(boolean(*value)),
    }
}

fn field_string(field: &Field<String>) -> Option<JsonValue> {
    match field {
        Field::Absent => None,
        Field::Null => Some(JsonValue::Null),
        Field::Value(value) => Some(string(value)),
    }
}

fn field_enum<T: Copy + AsStr>(field: &Field<T>) -> Option<JsonValue> {
    match field {
        Field::Absent => None,
        Field::Null => Some(JsonValue::Null),
        Field::Value(value) => Some(string(value.as_str())),
    }
}

trait AsStr {
    fn as_str(&self) -> &'static str;
}

impl AsStr for ProviderGroup {
    fn as_str(&self) -> &'static str {
        ProviderGroup::as_str(*self)
    }
}

impl AsStr for ProviderVisibility {
    fn as_str(&self) -> &'static str {
        ProviderVisibility::as_str(*self)
    }
}

impl AsStr for ProviderApiType {
    fn as_str(&self) -> &'static str {
        ProviderApiType::as_str(*self)
    }
}

impl AsStr for ZhipuAccountMode {
    fn as_str(&self) -> &'static str {
        ZhipuAccountMode::as_str(*self)
    }
}

impl AsStr for ZhipuAccountType {
    fn as_str(&self) -> &'static str {
        ZhipuAccountType::as_str(*self)
    }
}

fn string_array_json(values: &[String]) -> JsonValue {
    array(values.iter().map(string).collect())
}

fn field_string_record(field: &Field<IndexMap<String, String>>) -> Option<JsonValue> {
    match field {
        Field::Absent => None,
        Field::Null => Some(JsonValue::Null),
        Field::Value(values) => Some(JsonValue::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), string(value)))
                .collect(),
        )),
    }
}

fn push_field<T>(
    entries: &mut Vec<(&'static str, JsonValue)>,
    key: &'static str,
    field: &Field<T>,
    to_json: impl FnOnce(&T) -> JsonValue,
) {
    match field {
        Field::Absent => {}
        Field::Null => entries.push((key, JsonValue::Null)),
        Field::Value(value) => entries.push((key, to_json(value))),
    }
}
