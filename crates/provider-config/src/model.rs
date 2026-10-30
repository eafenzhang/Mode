//! Model 配置模型：解析、complete 校验输入与保序序列化。
//!
//! 真源：`packages/provider/src/config/model-config.ts`、
//! `packages/provider/src/config/manual-model-config.ts`、
//! `packages/shared/src/model-config.ts`。

use indexmap::IndexMap;

use crate::cel::validate_option_map;
use crate::json::{array, boolean, int, JsonValue};
use crate::value::{
    check_unknown_keys, expect_object, parse_bool, parse_optional_field, parse_positive_int,
    parse_required_field, parse_string, Field, Issue, ParseResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Sparse,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelInputFormatConfig {
    pub supports_text: Field<bool>,
    pub supports_image: Field<bool>,
    pub supports_video: Field<bool>,
    pub supports_audio: Field<bool>,
    pub supports_pdf: Field<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelOutputFormatConfig {
    pub supports_text: Field<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelPropertiesConfig {
    pub requires_mfjs_tool_schema: Field<bool>,
    pub context_window: Field<i64>,
    pub input_format: Field<ModelInputFormatConfig>,
    pub output_format: Field<ModelOutputFormatConfig>,
    pub supports_tool_call: Field<bool>,
    pub supports_json_schema_output: Field<bool>,
    pub supports_native_web_search: Field<bool>,
    pub supports_mid_conversation_system: Field<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EnumOptionSpecConfig {
    pub values: Field<Vec<String>>,
    pub map: Field<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LimitOptionSpecConfig {
    pub max: Field<i64>,
    pub map: Field<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelOptionSpecsConfig {
    pub reasoning_level: Field<EnumOptionSpecConfig>,
    pub max_output_tokens: Field<LimitOptionSpecConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelConfig {
    pub enabled: Field<bool>,
    pub properties: Field<ModelPropertiesConfig>,
    pub option_specs: Field<ModelOptionSpecsConfig>,
}

/// 手动配置：字段白名单与序列化顺序跟 `manualModelConfigSchema` 一致。
/// 注意它的 `properties` 顺序是 `contextWindow, supportsJsonSchemaOutput,
/// supportsNativeWebSearch, supportsMidConversationSystem, inputFormat`，
/// 与 `ModelConfig.toJSON()` 不同。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManualModelConfig {
    pub config: ModelConfig,
}

impl ManualModelConfig {
    pub fn into_model(self) -> ModelConfig {
        self.config
    }

    pub fn to_json(&self) -> JsonValue {
        manual_to_json(&self.config)
    }
}

impl ModelConfig {
    pub fn empty() -> Self {
        Self::default()
    }

    /// `ModelConfig.fromData`：从已校验的数据构造（解析路径使用）。
    pub fn from_data(data: ModelConfig) -> Self {
        data
    }

    /// `validateComplete` 的输入（`toJSON()` 结果）。
    pub fn to_json(&self) -> JsonValue {
        let mut entries = Vec::new();
        if let Some(enabled) = field_bool(&self.enabled) {
            entries.push(("enabled", enabled));
        }
        if let Some(properties) = field_json(&self.properties, ModelPropertiesConfig::to_json) {
            entries.push(("properties", properties));
        }
        if let Some(option_specs) = field_json(&self.option_specs, ModelOptionSpecsConfig::to_json)
        {
            entries.push(("optionSpecs", option_specs));
        }
        object_entries(entries)
    }

    /// 等价于 `validateComplete()`；返回已翻译的 ConfigValidationIssue 形状。
    pub fn validate_complete(&self) -> Vec<Issue> {
        let value = self.to_json();
        match parse_model_config_with(&value, &[], Mode::Complete) {
            Ok(_) => Vec::new(),
            Err(ParseErrorOrIssues::Issues(issues)) => {
                translate_issues(&value, issues, false)
            }
            Err(ParseErrorOrIssues::Invariant(message)) => {
                vec![Issue::new("invalid-config", Vec::new(), message)]
            }
        }
    }
}

impl ModelPropertiesConfig {
    pub fn to_json(&self) -> JsonValue {
        let mut entries = Vec::new();
        push_field_bool(&mut entries, "requiresMfjsToolSchema", &self.requires_mfjs_tool_schema);
        push_field_int(&mut entries, "contextWindow", &self.context_window);
        if let Some(input_format) = field_json(&self.input_format, ModelInputFormatConfig::to_json) {
            entries.push(("inputFormat", input_format));
        }
        if let Some(output_format) = field_json(&self.output_format, ModelOutputFormatConfig::to_json) {
            entries.push(("outputFormat", output_format));
        }
        push_field_bool(&mut entries, "supportsToolCall", &self.supports_tool_call);
        push_field_bool(
            &mut entries,
            "supportsJsonSchemaOutput",
            &self.supports_json_schema_output,
        );
        push_field_bool(
            &mut entries,
            "supportsNativeWebSearch",
            &self.supports_native_web_search,
        );
        push_field_bool(
            &mut entries,
            "supportsMidConversationSystem",
            &self.supports_mid_conversation_system,
        );
        object_entries(entries)
    }

    pub fn validate_complete(&self) -> Vec<Issue> {
        let value = self.to_json();
        let issues = raw_complete_issues(&value, |value, path| {
            parse_properties(value, path, Mode::Complete).map(|_| ())
        });
        translate_issues(&value, issues, false)
    }
}

impl ModelInputFormatConfig {
    pub fn to_json(&self) -> JsonValue {
        let mut entries = Vec::new();
        push_field_bool(&mut entries, "supportsText", &self.supports_text);
        push_field_bool(&mut entries, "supportsImage", &self.supports_image);
        push_field_bool(&mut entries, "supportsVideo", &self.supports_video);
        push_field_bool(&mut entries, "supportsAudio", &self.supports_audio);
        push_field_bool(&mut entries, "supportsPdf", &self.supports_pdf);
        object_entries(entries)
    }

    pub fn validate_complete(&self) -> Vec<Issue> {
        let value = self.to_json();
        let issues = raw_complete_issues(&value, |value, path| {
            parse_input_format(value, path, Mode::Complete).map(|_| ())
        });
        translate_issues(&value, issues, false)
    }
}

impl ModelOutputFormatConfig {
    pub fn to_json(&self) -> JsonValue {
        let mut entries = Vec::new();
        push_field_bool(&mut entries, "supportsText", &self.supports_text);
        object_entries(entries)
    }

    pub fn validate_complete(&self) -> Vec<Issue> {
        let value = self.to_json();
        let issues = raw_complete_issues(&value, |value, path| {
            parse_output_format(value, path, Mode::Complete).map(|_| ())
        });
        translate_issues(&value, issues, false)
    }
}

impl EnumOptionSpecConfig {
    pub fn to_json(&self) -> JsonValue {
        let mut entries = Vec::new();
        if let Some(values) = field_string_array(&self.values) {
            entries.push(("values", values));
        }
        if let Some(map) = field_string(&self.map) {
            entries.push(("map", map));
        }
        object_entries(entries)
    }

    pub fn validate_complete(&self) -> Vec<Issue> {
        let value = self.to_json();
        let issues = raw_complete_issues(&value, |value, path| {
            parse_enum_option_spec(value, path, Mode::Complete).map(|_| ())
        });
        translate_issues(&value, issues, true)
    }
}

impl LimitOptionSpecConfig {
    pub fn to_json(&self) -> JsonValue {
        let mut entries = Vec::new();
        push_field_int(&mut entries, "max", &self.max);
        if let Some(map) = field_string(&self.map) {
            entries.push(("map", map));
        }
        object_entries(entries)
    }

    pub fn validate_complete(&self) -> Vec<Issue> {
        let value = self.to_json();
        let issues = raw_complete_issues(&value, |value, path| {
            parse_limit_option_spec(value, path, Mode::Complete).map(|_| ())
        });
        translate_issues(&value, issues, true)
    }
}

impl ModelOptionSpecsConfig {
    pub fn to_json(&self) -> JsonValue {
        let mut entries = Vec::new();
        if let Some(reasoning) = field_json(&self.reasoning_level, EnumOptionSpecConfig::to_json) {
            entries.push(("reasoningLevel", reasoning));
        }
        if let Some(max_output_tokens) =
            field_json(&self.max_output_tokens, LimitOptionSpecConfig::to_json)
        {
            entries.push(("maxOutputTokens", max_output_tokens));
        }
        object_entries(entries)
    }

    pub fn validate_complete(&self) -> Vec<Issue> {
        let value = self.to_json();
        let issues = raw_complete_issues(&value, |value, path| {
            parse_option_specs(value, path, Mode::Complete).map(|_| ())
        });
        translate_issues(&value, issues, true)
    }
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

pub fn parse_model_config(value: &JsonValue, path: &[String]) -> ParseResult<ModelConfig> {
    parse_model_config_with(value, path, Mode::Sparse).map_err(issues_into_error)
}

pub fn parse_manual_model_config(
    value: &JsonValue,
    path: &[String],
) -> ParseResult<ManualModelConfig> {
    parse_manual_model_config_issues(value, path)
        .map(|config| ManualModelConfig { config })
        .map_err(issues_into_error)
}

fn parse_manual_model_config_issues(
    value: &JsonValue,
    path: &[String],
) -> Result<ModelConfig, ParseErrorOrIssues> {
    let map = expect_object(value, path).map_err(ParseErrorOrIssues::Issues)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["enabled", "properties", "optionSpecs"], path, &mut issues);

    let enabled = parse_optional_field(map, "enabled", path, parse_bool)
        .unwrap_or_else(|mut errors| {
            issues.append(&mut errors);
            Field::Absent
        });
    let properties = parse_required_field(map, "properties", path, "object", |value, field_path| {
        parse_manual_properties(value, field_path)
    })
    .map(Field::Value)
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        Field::Absent
    });
    let option_specs = parse_required_field(map, "optionSpecs", path, "object", |value, field_path| {
        parse_manual_option_specs(value, field_path)
    })
    .map(Field::Value)
    .unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        Field::Absent
    });

    if issues.is_empty() {
        Ok(ModelConfig {
            enabled,
            properties,
            option_specs,
        })
    } else {
        Err(ParseErrorOrIssues::Issues(issues))
    }
}

enum ParseErrorOrIssues {
    Issues(Vec<Issue>),
    #[allow(dead_code)]
    Invariant(String),
}

fn issues_into_error(error: ParseErrorOrIssues) -> crate::value::ParseError {
    match error {
        ParseErrorOrIssues::Issues(issues) => crate::value::ParseError::Issues(issues),
        ParseErrorOrIssues::Invariant(message) => crate::value::ParseError::Invariant(message),
    }
}

fn parse_model_config_with(
    value: &JsonValue,
    path: &[String],
    mode: Mode,
) -> Result<ModelConfig, ParseErrorOrIssues> {
    let map = expect_object(value, path).map_err(ParseErrorOrIssues::Issues)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["enabled", "properties", "optionSpecs"], path, &mut issues);

    let enabled = parse_mode_field(map, "enabled", path, mode, "boolean", parse_bool, &mut issues);
    let properties = parse_mode_field(
        map,
        "properties",
        path,
        mode,
        "object",
        |value, field_path| parse_properties(value, field_path, mode),
        &mut issues,
    );
    let option_specs = parse_mode_field(
        map,
        "optionSpecs",
        path,
        mode,
        "object",
        |value, field_path| parse_option_specs(value, field_path, mode),
        &mut issues,
    );

    if issues.is_empty() {
        Ok(ModelConfig {
            enabled,
            properties,
            option_specs,
        })
    } else {
        Err(ParseErrorOrIssues::Issues(issues))
    }
}

fn parse_properties(
    value: &JsonValue,
    path: &[String],
    mode: Mode,
) -> Result<ModelPropertiesConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(
        map,
        &[
            "requiresMfjsToolSchema",
            "contextWindow",
            "inputFormat",
            "outputFormat",
            "supportsToolCall",
            "supportsJsonSchemaOutput",
            "supportsNativeWebSearch",
            "supportsMidConversationSystem",
        ],
        path,
        &mut issues,
    );

    let requires_mfjs_tool_schema = parse_mode_field(
        map,
        "requiresMfjsToolSchema",
        path,
        mode,
        "boolean",
        parse_bool,
        &mut issues,
    );
    let context_window = parse_mode_field(
        map,
        "contextWindow",
        path,
        mode,
        "int",
        parse_positive_int,
        &mut issues,
    );
    let input_format = parse_mode_field(
        map,
        "inputFormat",
        path,
        mode,
        "object",
        |value, field_path| parse_input_format(value, field_path, mode),
        &mut issues,
    );
    let output_format = parse_mode_field(
        map,
        "outputFormat",
        path,
        mode,
        "object",
        |value, field_path| parse_output_format(value, field_path, mode),
        &mut issues,
    );
    let supports_tool_call = parse_mode_field(
        map,
        "supportsToolCall",
        path,
        mode,
        "boolean",
        parse_bool,
        &mut issues,
    );
    let supports_json_schema_output = parse_mode_field(
        map,
        "supportsJsonSchemaOutput",
        path,
        mode,
        "boolean",
        parse_bool,
        &mut issues,
    );
    let supports_native_web_search = parse_mode_field(
        map,
        "supportsNativeWebSearch",
        path,
        mode,
        "boolean",
        parse_bool,
        &mut issues,
    );
    let supports_mid_conversation_system = parse_mode_field(
        map,
        "supportsMidConversationSystem",
        path,
        mode,
        "boolean",
        parse_bool,
        &mut issues,
    );

    if issues.is_empty() {
        Ok(ModelPropertiesConfig {
            requires_mfjs_tool_schema,
            context_window,
            input_format,
            output_format,
            supports_tool_call,
            supports_json_schema_output,
            supports_native_web_search,
            supports_mid_conversation_system,
        })
    } else {
        Err(issues)
    }
}

fn parse_input_format(
    value: &JsonValue,
    path: &[String],
    mode: Mode,
) -> Result<ModelInputFormatConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    let keys = [
        "supportsText",
        "supportsImage",
        "supportsVideo",
        "supportsAudio",
        "supportsPdf",
    ];
    check_unknown_keys(map, &keys, path, &mut issues);

    let supports_text =
        parse_mode_field(map, "supportsText", path, mode, "boolean", parse_bool, &mut issues);
    let supports_image =
        parse_mode_field(map, "supportsImage", path, mode, "boolean", parse_bool, &mut issues);
    let supports_video =
        parse_mode_field(map, "supportsVideo", path, mode, "boolean", parse_bool, &mut issues);
    let supports_audio =
        parse_mode_field(map, "supportsAudio", path, mode, "boolean", parse_bool, &mut issues);
    let supports_pdf =
        parse_mode_field(map, "supportsPdf", path, mode, "boolean", parse_bool, &mut issues);

    if issues.is_empty() {
        Ok(ModelInputFormatConfig {
            supports_text,
            supports_image,
            supports_video,
            supports_audio,
            supports_pdf,
        })
    } else {
        Err(issues)
    }
}

fn parse_output_format(
    value: &JsonValue,
    path: &[String],
    mode: Mode,
) -> Result<ModelOutputFormatConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["supportsText"], path, &mut issues);
    let supports_text =
        parse_mode_field(map, "supportsText", path, mode, "boolean", parse_bool, &mut issues);
    if issues.is_empty() {
        Ok(ModelOutputFormatConfig { supports_text })
    } else {
        Err(issues)
    }
}

fn parse_enum_option_spec(
    value: &JsonValue,
    path: &[String],
    mode: Mode,
) -> Result<EnumOptionSpecConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["values", "map"], path, &mut issues);

    let values = parse_mode_field(
        map,
        "values",
        path,
        mode,
        "array",
        parse_reasoning_values,
        &mut issues,
    );
    let map_field = parse_mode_field(
        map,
        "map",
        path,
        mode,
        "string",
        |value, field_path| parse_option_map_string(value, field_path, "reasoningLevel"),
        &mut issues,
    );

    if issues.is_empty() {
        Ok(EnumOptionSpecConfig {
            values,
            map: map_field,
        })
    } else {
        Err(issues)
    }
}

fn parse_reasoning_values(value: &JsonValue, path: &[String]) -> Result<Vec<String>, Vec<Issue>> {
    let values = value
        .as_array()
        .ok_or_else(|| vec![Issue::invalid_type(path, "array", value.kind())])?;
    if values.is_empty() {
        return Err(vec![Issue::too_small(path, ">=1 item(s)")]);
    }
    let mut result = Vec::with_capacity(values.len());
    let mut issues = Vec::new();
    for (index, entry) in values.iter().enumerate() {
        let mut entry_path = path.to_vec();
        entry_path.push(index.to_string());
        match parse_string(entry, &entry_path) {
            Ok(text) if !text.trim().is_empty() => result.push(text),
            Ok(_) => issues.push(Issue::custom(
                &entry_path,
                "reasoningLevel.values 必须是非空字符串",
            )),
            Err(mut errors) => issues.append(&mut errors),
        }
    }
    let mut seen = std::collections::HashSet::new();
    for value in &result {
        if !seen.insert(value.clone()) {
            issues.push(Issue::custom(path, "reasoningLevel.values 不能重复"));
            break;
        }
    }
    if issues.is_empty() {
        Ok(result)
    } else {
        Err(issues)
    }
}

fn parse_limit_option_spec(
    value: &JsonValue,
    path: &[String],
    mode: Mode,
) -> Result<LimitOptionSpecConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["max", "map"], path, &mut issues);

    let max = parse_mode_field(
        map,
        "max",
        path,
        mode,
        "int",
        parse_positive_int,
        &mut issues,
    );
    let map_field = parse_mode_field(
        map,
        "map",
        path,
        mode,
        "string",
        |value, field_path| parse_option_map_string(value, field_path, "maxOutputTokens"),
        &mut issues,
    );

    if issues.is_empty() {
        Ok(LimitOptionSpecConfig {
            max,
            map: map_field,
        })
    } else {
        Err(issues)
    }
}

fn parse_option_specs(
    value: &JsonValue,
    path: &[String],
    mode: Mode,
) -> Result<ModelOptionSpecsConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["reasoningLevel", "maxOutputTokens"], path, &mut issues);

    let reasoning_level = parse_mode_field(
        map,
        "reasoningLevel",
        path,
        mode,
        "object",
        |value, field_path| parse_enum_option_spec(value, field_path, mode),
        &mut issues,
    );
    let max_output_tokens = parse_mode_field(
        map,
        "maxOutputTokens",
        path,
        mode,
        "object",
        |value, field_path| parse_limit_option_spec(value, field_path, mode),
        &mut issues,
    );

    if issues.is_empty() {
        Ok(ModelOptionSpecsConfig {
            reasoning_level,
            max_output_tokens,
        })
    } else {
        Err(issues)
    }
}

fn parse_manual_properties(
    value: &JsonValue,
    path: &[String],
) -> Result<ModelPropertiesConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(
        map,
        &[
            "contextWindow",
            "supportsJsonSchemaOutput",
            "supportsNativeWebSearch",
            "supportsMidConversationSystem",
            "inputFormat",
        ],
        path,
        &mut issues,
    );

    let context_window = required_value(map, "contextWindow", path, "int", parse_positive_int, &mut issues);
    let supports_json_schema_output =
        required_value(map, "supportsJsonSchemaOutput", path, "boolean", parse_bool, &mut issues);
    let supports_native_web_search =
        required_value(map, "supportsNativeWebSearch", path, "boolean", parse_bool, &mut issues);
    let supports_mid_conversation_system = required_value(
        map,
        "supportsMidConversationSystem",
        path,
        "boolean",
        parse_bool,
        &mut issues,
    );
    let input_format = required_value(
        map,
        "inputFormat",
        path,
        "object",
        parse_manual_input_format,
        &mut issues,
    );

    if issues.is_empty() {
        Ok(ModelPropertiesConfig {
            requires_mfjs_tool_schema: Field::Absent,
            context_window: Field::Value(context_window),
            input_format: Field::Value(input_format),
            output_format: Field::Absent,
            supports_tool_call: Field::Absent,
            supports_json_schema_output: Field::Value(supports_json_schema_output),
            supports_native_web_search: Field::Value(supports_native_web_search),
            supports_mid_conversation_system: Field::Value(supports_mid_conversation_system),
        })
    } else {
        Err(issues)
    }
}

fn parse_manual_input_format(
    value: &JsonValue,
    path: &[String],
) -> Result<ModelInputFormatConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(
        map,
        &["supportsImage", "supportsVideo", "supportsPdf"],
        path,
        &mut issues,
    );
    let supports_image = required_value(map, "supportsImage", path, "boolean", parse_bool, &mut issues);
    let supports_video = required_value(map, "supportsVideo", path, "boolean", parse_bool, &mut issues);
    let supports_pdf = required_value(map, "supportsPdf", path, "boolean", parse_bool, &mut issues);
    if issues.is_empty() {
        Ok(ModelInputFormatConfig {
            supports_text: Field::Absent,
            supports_image: Field::Value(supports_image),
            supports_video: Field::Value(supports_video),
            supports_audio: Field::Absent,
            supports_pdf: Field::Value(supports_pdf),
        })
    } else {
        Err(issues)
    }
}

fn parse_manual_option_specs(
    value: &JsonValue,
    path: &[String],
) -> Result<ModelOptionSpecsConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["reasoningLevel", "maxOutputTokens"], path, &mut issues);

    let reasoning_level = required_value(
        map,
        "reasoningLevel",
        path,
        "object",
        |value, field_path| parse_enum_option_spec(value, field_path, Mode::Complete),
        &mut issues,
    );
    let max_output_tokens = required_value(
        map,
        "maxOutputTokens",
        path,
        "object",
        parse_manual_limit_spec,
        &mut issues,
    );

    if issues.is_empty() {
        Ok(ModelOptionSpecsConfig {
            reasoning_level: Field::Value(reasoning_level),
            max_output_tokens: Field::Value(max_output_tokens),
        })
    } else {
        Err(issues)
    }
}

fn parse_manual_limit_spec(
    value: &JsonValue,
    path: &[String],
) -> Result<LimitOptionSpecConfig, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut issues = Vec::new();
    check_unknown_keys(map, &["max"], path, &mut issues);
    let max = required_value(map, "max", path, "int", parse_positive_int, &mut issues);
    if issues.is_empty() {
        Ok(LimitOptionSpecConfig {
            max: Field::Value(max),
            map: Field::Absent,
        })
    } else {
        Err(issues)
    }
}

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

fn parse_mode_field<T>(
    map: &IndexMap<String, JsonValue>,
    key: &str,
    path: &[String],
    mode: Mode,
    expected: &str,
    parser: impl FnOnce(&JsonValue, &[String]) -> Result<T, Vec<Issue>>,
    issues: &mut Vec<Issue>,
) -> Field<T> {
    let result = match mode {
        Mode::Sparse => parse_optional_field(map, key, path, parser),
        Mode::Complete => {
            parse_required_field(map, key, path, expected, parser).map(Field::Value)
        }
    };
    result.unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        Field::Absent
    })
}

fn required_value<T>(
    map: &IndexMap<String, JsonValue>,
    key: &str,
    path: &[String],
    expected: &str,
    parser: impl FnOnce(&JsonValue, &[String]) -> Result<T, Vec<Issue>>,
    issues: &mut Vec<Issue>,
) -> T
where
    T: Default,
{
    parse_required_field(map, key, path, expected, parser).unwrap_or_else(|mut errors| {
        issues.append(&mut errors);
        T::default()
    })
}

fn parse_option_map_string(
    value: &JsonValue,
    path: &[String],
    variable_name: &str,
) -> Result<String, Vec<Issue>> {
    let source = parse_string(value, path)?;
    if source.is_empty() {
        return Err(vec![Issue::too_small(path, ">=1 character(s)")]);
    }
    validate_option_map(&source, variable_name)
        .map_err(|message| vec![Issue::custom(path, message)])?;
    Ok(source)
}

fn manual_to_json(config: &ModelConfig) -> JsonValue {
    let mut entries = Vec::new();
    push_field_bool(&mut entries, "enabled", &config.enabled);
    if let Some(properties) = field_json(&config.properties, manual_properties_to_json) {
        entries.push(("properties", properties));
    }
    if let Some(option_specs) = field_json(&config.option_specs, manual_option_specs_to_json) {
        entries.push(("optionSpecs", option_specs));
    }
    object_entries(entries)
}

fn manual_properties_to_json(properties: &ModelPropertiesConfig) -> JsonValue {
    let mut entries = Vec::new();
    push_field_int(&mut entries, "contextWindow", &properties.context_window);
    push_field_bool(
        &mut entries,
        "supportsJsonSchemaOutput",
        &properties.supports_json_schema_output,
    );
    push_field_bool(
        &mut entries,
        "supportsNativeWebSearch",
        &properties.supports_native_web_search,
    );
    push_field_bool(
        &mut entries,
        "supportsMidConversationSystem",
        &properties.supports_mid_conversation_system,
    );
    if let Some(input_format) = field_json(&properties.input_format, |input_format| {
        let mut entries = Vec::new();
        push_field_bool(&mut entries, "supportsImage", &input_format.supports_image);
        push_field_bool(&mut entries, "supportsVideo", &input_format.supports_video);
        push_field_bool(&mut entries, "supportsPdf", &input_format.supports_pdf);
        object_entries(entries)
    }) {
        entries.push(("inputFormat", input_format));
    }
    object_entries(entries)
}

fn manual_option_specs_to_json(option_specs: &ModelOptionSpecsConfig) -> JsonValue {
    let mut entries = Vec::new();
    if let Some(reasoning) =
        field_json(&option_specs.reasoning_level, EnumOptionSpecConfig::to_json)
    {
        entries.push(("reasoningLevel", reasoning));
    }
    if let Some(max_output_tokens) = field_json(&option_specs.max_output_tokens, |limit| {
        let mut entries = Vec::new();
        push_field_int(&mut entries, "max", &limit.max);
        object_entries(entries)
    }) {
        entries.push(("maxOutputTokens", max_output_tokens));
    }
    object_entries(entries)
}

fn object_entries(entries: Vec<(&str, JsonValue)>) -> JsonValue {    let mut map = IndexMap::new();
    for (key, value) in entries {
        map.insert(key.to_owned(), value);
    }
    JsonValue::Object(map)
}

fn field_json<T>(field: &Field<T>, to_json: impl Fn(&T) -> JsonValue) -> Option<JsonValue> {
    match field {
        Field::Absent => None,
        Field::Null => Some(JsonValue::Null),
        Field::Value(value) => Some(to_json(value)),
    }
}

fn field_bool(field: &Field<bool>) -> Option<JsonValue> {
    field_json(field, |value| boolean(*value))
}

fn field_int(field: &Field<i64>) -> Option<JsonValue> {
    field_json(field, |value| int(*value))
}

fn field_string(field: &Field<String>) -> Option<JsonValue> {
    field_json(field, |value| crate::json::string(value.clone()))
}

fn field_string_array(field: &Field<Vec<String>>) -> Option<JsonValue> {
    field_json(field, |values| {
        array(values.iter().cloned().map(crate::json::string).collect())
    })
}

fn push_field_bool(entries: &mut Vec<(&'static str, JsonValue)>, key: &'static str, field: &Field<bool>) {
    if let Some(value) = field_bool(field) {
        entries.push((key, value));
    }
}

fn push_field_int(entries: &mut Vec<(&'static str, JsonValue)>, key: &'static str, field: &Field<i64>) {
    if let Some(value) = field_int(field) {
        entries.push((key, value));
    }
}

/// 运行一次 complete 解析，返回原始 zod 风格问题。
fn raw_complete_issues(
    value: &JsonValue,
    parser: impl FnOnce(&JsonValue, &[String]) -> Result<(), Vec<Issue>>,
) -> Vec<Issue> {
    match parser(value, &[]) {
        Ok(()) => Vec::new(),
        Err(issues) => issues,
    }
}

/// 复刻 `validateConfigSchema` 的问题分类与路径。
pub fn translate_issues(value: &JsonValue, issues: Vec<Issue>, option_spec: bool) -> Vec<Issue> {
    translate_issues_at(value, 0, issues, option_spec)
}

/// 同 `translate_issues`，但问题路径带有 `base_len` 段外部前缀
/// （ProviderConfigMap 按 providerId 路径验证）。
pub fn translate_issues_at(
    value: &JsonValue,
    base_len: usize,
    issues: Vec<Issue>,
    option_spec: bool,
) -> Vec<Issue> {
    issues
        .into_iter()
        .map(|issue| translate_issue_at(value, base_len, issue, option_spec))
        .collect()
}

fn translate_issue_at(
    value: &JsonValue,
    base_len: usize,
    issue: Issue,
    option_spec: bool,
) -> Issue {
    let relative = if base_len <= issue.path.len() {
        issue.path[base_len..].to_vec()
    } else {
        issue.path.clone()
    };
    let field_value = value_at_path(value, &relative);
    let missing = ((issue.code == "invalid_type" || issue.code == "invalid_value")
        && field_value.is_none_or(JsonValue::is_null))
        || (issue.code == "custom" && issue.message == "必填配置不能为空");
    let invalid_url = issue.code == "invalid_format";
    let code = if missing {
        "required-field-missing"
    } else if invalid_url {
        "invalid-url"
    } else if option_spec || issue.path.iter().any(|segment| segment == "optionSpecs") {
        "invalid-option-spec"
    } else {
        "invalid-config"
    };
    let joined = issue.path.join(".");
    let message = if missing {
        format!("缺少必填配置 {joined}")
    } else if invalid_url {
        format!("配置 {joined} 必须是有效 URL")
    } else {
        issue.message
    };
    Issue::new(code, issue.path, message)
}

fn value_at_path<'a>(value: &'a JsonValue, path: &[String]) -> Option<&'a JsonValue> {
    let mut current = value;
    for segment in path {
        match current {
            JsonValue::Object(map) => current = map.get(segment)?,
            JsonValue::Array(values) => current = values.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        }
    }
    Some(current)
}
