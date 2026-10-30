//! 稀疏配置字段、校验问题与对象解析辅助。

use indexmap::IndexMap;

use crate::json::JsonValue;

/// TS 的 `T | null | undefined`：`Absent` 对应 undefined（不输出），`Null` 输出 `null`。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Field<T> {
    #[default]
    Absent,
    Null,
    Value(T),
}

impl<T> Field<T> {
    pub fn is_absent(&self) -> bool {
        matches!(self, Field::Absent)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Field::Null)
    }

    pub fn value(&self) -> Option<&T> {
        match self {
            Field::Value(value) => Some(value),
            _ => None,
        }
    }

    pub fn into_value(self) -> Option<T> {
        match self {
            Field::Value(value) => Some(value),
            _ => None,
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Field<U> {
        match self {
            Field::Absent => Field::Absent,
            Field::Null => Field::Null,
            Field::Value(value) => Field::Value(f(value)),
        }
    }

    pub fn as_ref(&self) -> Field<&T> {
        match self {
            Field::Absent => Field::Absent,
            Field::Null => Field::Null,
            Field::Value(value) => Field::Value(value),
        }
    }
}

/// 与 zod issue 对齐的最小字段：接受/拒绝看 `code` 与 `path`，message 只用于诊断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub code: String,
    pub path: Vec<String>,
    pub message: String,
}

impl Issue {
    pub fn new(
        code: impl Into<String>,
        path: impl Into<Vec<String>>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn invalid_type(path: &[String], expected: &str, received: &str) -> Self {
        Self::new(
            "invalid_type",
            path.to_vec(),
            format!("Invalid input: expected {expected}, received {received}"),
        )
    }

    pub fn invalid_value(
        path: &[String],
        values: &[&str],
        received: impl std::fmt::Display,
    ) -> Self {
        Self::new(
            "invalid_value",
            path.to_vec(),
            format!(
                "Invalid option: expected one of {}; received {received}",
                values.join("|")
            ),
        )
    }

    pub fn unrecognized_keys(path: &[String], keys: &[&String]) -> Self {
        let keys = keys
            .iter()
            .map(|key| format!("{key:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        Self::new(
            "unrecognized_keys",
            path.to_vec(),
            format!("Unrecognized key(s): {keys}"),
        )
    }

    pub fn too_small(path: &[String], minimum: &str) -> Self {
        Self::new(
            "too_small",
            path.to_vec(),
            format!("Too small: expected number to be {minimum}"),
        )
    }

    pub fn custom(path: &[String], message: impl Into<String>) -> Self {
        Self::new("custom", path.to_vec(), message)
    }

    pub fn with_prefix(mut self, key: &str) -> Self {
        self.path.insert(0, key.to_owned());
        self
    }

    /// 稳定路径键，用于对拍时按集合比较。
    pub fn path_key(&self) -> String {
        self.path.join("\u{1f}")
    }
}

/// 解析失败：schema 问题，或 TS 构造函数抛出的非 schema 错误（重复 key、空模型 ID）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Issues(Vec<Issue>),
    Invariant(String),
}

impl ParseError {
    pub fn issues(&self) -> &[Issue] {
        match self {
            ParseError::Issues(issues) => issues,
            ParseError::Invariant(_) => &[],
        }
    }

    pub fn invariant(message: impl Into<String>) -> Self {
        ParseError::Invariant(message.into())
    }
}

impl From<Vec<Issue>> for ParseError {
    fn from(issues: Vec<Issue>) -> Self {
        ParseError::Issues(issues)
    }
}

pub type ParseResult<T> = Result<T, ParseError>;

pub fn expect_object<'a>(
    value: &'a JsonValue,
    path: &[String],
) -> Result<&'a IndexMap<String, JsonValue>, Vec<Issue>> {
    value
        .as_object()
        .ok_or_else(|| vec![Issue::invalid_type(path, "object", value.kind())])
}

pub fn check_unknown_keys(
    map: &IndexMap<String, JsonValue>,
    allowed: &[&str],
    path: &[String],
    issues: &mut Vec<Issue>,
) {
    let unknown = map
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        issues.push(Issue::unrecognized_keys(path, &unknown));
    }
}

pub fn parse_optional_field<T>(
    map: &IndexMap<String, JsonValue>,
    key: &str,
    path: &[String],
    parser: impl FnOnce(&JsonValue, &[String]) -> Result<T, Vec<Issue>>,
) -> Result<Field<T>, Vec<Issue>> {
    match map.get(key) {
        None => Ok(Field::Absent),
        Some(JsonValue::Null) => Ok(Field::Null),
        Some(value) => {
            let mut field_path = path.to_vec();
            field_path.push(key.to_owned());
            parser(value, &field_path).map(Field::Value)
        }
    }
}

/// `optional` 但 **非 nullable** 的字段：缺省合法，`null` 非法。
pub fn parse_optional_non_null_field<T>(
    map: &IndexMap<String, JsonValue>,
    key: &str,
    path: &[String],
    expected: &str,
    parser: impl FnOnce(&JsonValue, &[String]) -> Result<T, Vec<Issue>>,
) -> Result<Field<T>, Vec<Issue>> {
    let mut field_path = path.to_vec();
    field_path.push(key.to_owned());
    match map.get(key) {
        None => Ok(Field::Absent),
        Some(JsonValue::Null) => Err(vec![Issue::invalid_type(&field_path, expected, "null")]),
        Some(value) => parser(value, &field_path).map(Field::Value),
    }
}

pub fn parse_required_field<T>(
    map: &IndexMap<String, JsonValue>,
    key: &str,
    path: &[String],
    expected: &str,
    parser: impl FnOnce(&JsonValue, &[String]) -> Result<T, Vec<Issue>>,
) -> Result<T, Vec<Issue>> {
    let mut field_path = path.to_vec();
    field_path.push(key.to_owned());
    match map.get(key) {
        None => Err(vec![Issue::invalid_type(&field_path, expected, "undefined")]),
        Some(JsonValue::Null) => Err(vec![Issue::invalid_type(&field_path, expected, "null")]),
        Some(value) => parser(value, &field_path),
    }
}

pub fn parse_bool(value: &JsonValue, path: &[String]) -> Result<bool, Vec<Issue>> {
    value
        .as_bool()
        .ok_or_else(|| vec![Issue::invalid_type(path, "boolean", value.kind())])
}

pub fn parse_string(value: &JsonValue, path: &[String]) -> Result<String, Vec<Issue>> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| vec![Issue::invalid_type(path, "string", value.kind())])
}

pub fn parse_non_empty_string(value: &JsonValue, path: &[String]) -> Result<String, Vec<Issue>> {
    let text = parse_string(value, path)?;
    if text.is_empty() {
        return Err(vec![Issue::too_small(path, ">=1 character(s)")]);
    }
    Ok(text)
}

pub fn parse_non_blank_string(value: &JsonValue, path: &[String]) -> Result<String, Vec<Issue>> {
    let text = parse_string(value, path)?;
    if text.trim().is_empty() {
        return Err(vec![Issue::custom(path, "必填配置不能为空")]);
    }
    Ok(text)
}

/// `z.number().int().positive()`：安全整数且 > 0。
pub fn parse_positive_int(value: &JsonValue, path: &[String]) -> Result<i64, Vec<Issue>> {
    let number = value
        .as_number()
        .ok_or_else(|| vec![Issue::invalid_type(path, "int", value.kind())])?;
    let integer = crate::json::safe_integer(number).ok_or_else(|| {
        vec![Issue::invalid_type(path, "int", value.kind())]
    })?;
    if integer <= 0 {
        return Err(vec![Issue::too_small(path, ">0")]);
    }
    Ok(integer)
}

pub fn parse_string_array(value: &JsonValue, path: &[String]) -> Result<Vec<String>, Vec<Issue>> {
    let values = value
        .as_array()
        .ok_or_else(|| vec![Issue::invalid_type(path, "array", value.kind())])?;
    let mut result = Vec::with_capacity(values.len());
    let mut issues = Vec::new();
    for (index, entry) in values.iter().enumerate() {
        let mut entry_path = path.to_vec();
        entry_path.push(index.to_string());
        match parse_non_empty_string(entry, &entry_path) {
            Ok(text) => result.push(text),
            Err(mut entry_issues) => issues.append(&mut entry_issues),
        }
    }
    if issues.is_empty() {
        Ok(result)
    } else {
        Err(issues)
    }
}

pub fn parse_string_record(
    value: &JsonValue,
    path: &[String],
) -> Result<IndexMap<String, String>, Vec<Issue>> {
    let map = expect_object(value, path)?;
    let mut result = IndexMap::new();
    let mut issues = Vec::new();
    for (key, entry) in map {
        let mut entry_path = path.to_vec();
        entry_path.push(key.clone());
        match parse_string(entry, &entry_path) {
            Ok(text) => {
                result.insert(key.clone(), text);
            }
            Err(mut entry_issues) => issues.append(&mut entry_issues),
        }
    }
    if issues.is_empty() {
        Ok(result)
    } else {
        Err(issues)
    }
}
