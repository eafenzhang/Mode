//! Subagent Markdown 的解析与序列化。
//!
//! 真源：`packages/services/src/subagents/subagentMarkdown.ts`。
//! YAML 采用受支持子集（嵌套映射/序列、标量、块标量、行内流式集合）；
//! 子集外输入按「严格解析失败」处理，与 TS 在 `yaml.parse` 抛错时回退宽松读取一致。

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::model::{format_markdown_model, parse_markdown_selection, Selection};
use plugin_discovery::js;

const VALID_COLORS: [&str; 8] = [
    "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan",
];
const VALID_PERMISSION_MODES: [&str; 2] = ["auto", "plan"];

#[derive(Debug, Clone, PartialEq)]
pub struct AgentDiagnostic {
    pub code: String,
    pub message: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub color: Option<String>,
    pub model_selection: Option<Selection>,
    pub default_model_selection: Option<Selection>,
    pub model_selection_override: Option<Selection>,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub inject_agents_md: Option<bool>,
    pub skills: Option<Vec<String>>,
    pub permission_mode: Option<String>,
    pub max_turns: Option<u64>,
    pub background: Option<bool>,
    pub mcp_servers: Option<Vec<Value>>,
    pub path: String,
    pub scope: String,
    pub source: String,
    pub enabled: bool,
    pub read_only: Option<bool>,
    pub project_path: Option<String>,
    pub plugin_id: Option<String>,
    pub plugin_name: Option<String>,
    pub diagnostics: Option<Vec<AgentDiagnostic>>,
}

impl AgentSummary {
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("id".into(), Value::String(self.id.clone()));
        object.insert("name".into(), Value::String(self.name.clone()));
        object.insert(
            "description".into(),
            Value::String(self.description.clone()),
        );
        object.insert(
            "systemPrompt".into(),
            Value::String(self.system_prompt.clone()),
        );
        if let Some(color) = &self.color {
            object.insert("color".into(), Value::String(color.clone()));
        }
        if let Some(selection) = &self.model_selection {
            object.insert(
                "modelSelection".into(),
                serde_json::to_value(selection).unwrap(),
            );
        }
        if let Some(selection) = &self.default_model_selection {
            object.insert(
                "defaultModelSelection".into(),
                serde_json::to_value(selection).unwrap(),
            );
        }
        if let Some(selection) = &self.model_selection_override {
            object.insert(
                "modelSelectionOverride".into(),
                serde_json::to_value(selection).unwrap(),
            );
        }
        if let Some(tools) = &self.tools {
            object.insert("tools".into(), strings(tools));
        }
        if let Some(tools) = &self.disallowed_tools {
            object.insert("disallowedTools".into(), strings(tools));
        }
        if let Some(value) = self.inject_agents_md {
            object.insert("injectAgentsMd".into(), Value::Bool(value));
        }
        if let Some(skills) = &self.skills {
            object.insert("skills".into(), strings(skills));
        }
        if let Some(mode) = &self.permission_mode {
            object.insert("permissionMode".into(), Value::String(mode.clone()));
        }
        if let Some(turns) = self.max_turns {
            object.insert("maxTurns".into(), json!(turns));
        }
        if let Some(value) = self.background {
            object.insert("background".into(), Value::Bool(value));
        }
        if let Some(servers) = &self.mcp_servers {
            object.insert("mcpServers".into(), Value::Array(servers.clone()));
        }
        object.insert("path".into(), Value::String(self.path.clone()));
        object.insert("scope".into(), Value::String(self.scope.clone()));
        object.insert("source".into(), Value::String(self.source.clone()));
        object.insert("enabled".into(), Value::Bool(self.enabled));
        if let Some(read_only) = self.read_only {
            object.insert("readOnly".into(), Value::Bool(read_only));
        }
        if let Some(project_path) = &self.project_path {
            object.insert("projectPath".into(), Value::String(project_path.clone()));
        }
        if let Some(plugin_id) = &self.plugin_id {
            object.insert("pluginId".into(), Value::String(plugin_id.clone()));
        }
        if let Some(plugin_name) = &self.plugin_name {
            object.insert("pluginName".into(), Value::String(plugin_name.clone()));
        }
        if let Some(diagnostics) = &self.diagnostics {
            object.insert(
                "diagnostics".into(),
                Value::Array(
                    diagnostics
                        .iter()
                        .map(|diagnostic| diagnostic.to_json())
                        .collect(),
                ),
            );
        }
        Value::Object(object)
    }
}

impl AgentDiagnostic {
    pub fn to_json(&self) -> Value {
        json!({ "code": self.code, "message": self.message, "path": self.path })
    }
}

fn strings(values: &[String]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|value| Value::String(value.clone()))
            .collect(),
    )
}

/// `parseSubagentMarkdown`：成功返回 agent，失败返回诊断。
pub struct ParsedAgent {
    pub agent: Option<AgentSummary>,
    pub diagnostic: Option<AgentDiagnostic>,
}

pub fn parse_agent_markdown(content: &str, path: &str, scope: &str) -> ParsedAgent {
    let normalized = content
        .strip_prefix('\u{feff}')
        .unwrap_or(content)
        .replace("\r\n", "\n");
    let split = split_markdown_frontmatter(&normalized);
    let Some(frontmatter_text) = split.frontmatter else {
        return ParsedAgent {
            agent: None,
            diagnostic: Some(AgentDiagnostic {
                code: "agent_missing_frontmatter".into(),
                message: format!("Agent Markdown must include frontmatter: {path}"),
                path: path.to_string(),
            }),
        };
    };
    let frontmatter = parse_runtime_compatible_frontmatter(&frontmatter_text);

    let name = scalar_string(frontmatter.get("name"));
    let description =
        scalar_string(frontmatter.get("description")).map(|value| value.replace("\\n", "\n"));
    let Some(name) = name else {
        return missing_required("name", path);
    };
    let Some(description) = description else {
        return missing_required("description", path);
    };

    let source = if scope == "built-in" {
        "built-in"
    } else {
        "user"
    };
    let model_selection =
        parse_markdown_selection(frontmatter.get("model"), frontmatter.get("thoughtLevel"));
    let color = normalize_enum(frontmatter.get("color"), &VALID_COLORS);
    let permission_mode =
        normalize_enum(frontmatter.get("permissionMode"), &VALID_PERMISSION_MODES);
    let max_turns = normalize_positive_integer(frontmatter.get("maxTurns"));
    let tools = parse_tool_spec_list(frontmatter.get("tools"));
    let disallowed_tools = parse_tool_spec_list(frontmatter.get("disallowedTools"));
    let skills = parse_string_list(frontmatter.get("skills"));
    let background = parse_optional_boolean(frontmatter.get("background"));
    let inject_agents_md = parse_optional_boolean(frontmatter.get("injectAgentsMd"));
    let mcp_servers = frontmatter
        .get("mcpServers")
        .and_then(Value::as_array)
        .cloned();

    ParsedAgent {
        agent: Some(AgentSummary {
            id: format!("{source}:{scope}:{}", name.trim().to_lowercase()),
            name,
            description,
            system_prompt: split.body.trim().to_string(),
            color,
            model_selection,
            default_model_selection: None,
            model_selection_override: None,
            tools,
            disallowed_tools,
            inject_agents_md,
            skills,
            permission_mode,
            max_turns,
            background,
            mcp_servers,
            path: path.to_string(),
            scope: scope.to_string(),
            source: source.to_string(),
            enabled: true,
            read_only: Some(scope == "built-in"),
            project_path: None,
            plugin_id: None,
            plugin_name: None,
            diagnostics: None,
        }),
        diagnostic: None,
    }
}

fn missing_required(field: &str, path: &str) -> ParsedAgent {
    ParsedAgent {
        agent: None,
        diagnostic: Some(AgentDiagnostic {
            code: "agent_missing_required_frontmatter".into(),
            message: format!("Agent frontmatter must include {field}: {path}"),
            path: path.to_string(),
        }),
    }
}

struct SplitMarkdown {
    frontmatter: Option<String>,
    body: String,
}

fn split_markdown_frontmatter(content: &str) -> SplitMarkdown {
    if !content.starts_with("---") {
        return SplitMarkdown {
            frontmatter: None,
            body: content.to_string(),
        };
    }
    let lines: Vec<&str> = content.split('\n').collect();
    if lines.first().map(|line| line.trim()) != Some("---") {
        return SplitMarkdown {
            frontmatter: None,
            body: content.to_string(),
        };
    }
    let end_index = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(index, _)| index);
    let Some(end_index) = end_index else {
        return SplitMarkdown {
            frontmatter: None,
            body: content.to_string(),
        };
    };
    SplitMarkdown {
        frontmatter: Some(lines[1..end_index].join("\n")),
        body: lines[end_index + 1..].join("\n"),
    }
}

/// `parseRuntimeCompatibleFrontmatter`：宽松值覆盖 YAML 值，`mcpServers` 仅取 YAML 数组。
fn parse_runtime_compatible_frontmatter(frontmatter: &str) -> Map<String, Value> {
    let loose = parse_loose_frontmatter(frontmatter);
    let yaml = parse_yaml_subset(frontmatter).unwrap_or_default();
    let yaml_mcp_servers = yaml.get("mcpServers").and_then(Value::as_array).cloned();
    let mut merged = yaml;
    for (key, value) in loose {
        merged.insert(key, value);
    }
    // YAML 的结构化 mcpServers 优先于宽松解析产出的字符串数组。
    if let Some(servers) = yaml_mcp_servers {
        merged.insert("mcpServers".into(), Value::Array(servers));
    }
    merged
}

fn parse_loose_frontmatter(frontmatter: &str) -> Map<String, Value> {
    let mut values = Map::new();
    let mut pending_list_key: Option<String> = None;
    for raw_line in frontmatter.split('\n') {
        let line = js::js_trim_end(raw_line);
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(item) = list_item(line) {
            if let Some(key) = &pending_list_key {
                let mut existing = match values.get(key).and_then(Value::as_array) {
                    Some(items) => items.clone(),
                    None => Vec::new(),
                };
                existing.push(parse_scalar_value(item));
                values.insert(key.clone(), Value::Array(existing));
                continue;
            }
        }
        pending_list_key = None;
        let Some((key, raw_value)) = key_value(line) else {
            continue;
        };
        if raw_value.trim().is_empty() {
            values.insert(key.clone(), Value::Array(Vec::new()));
            pending_list_key = Some(key);
            continue;
        }
        values.insert(key, parse_scalar_value(&raw_value));
    }
    values
}

fn list_item(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('-')?;
    // TS 的 `^\s*-\s+(.*)$` 要求 `-` 后至少一个空白。
    if rest.is_empty() || !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some(rest.trim_start_matches([' ', '\t']))
}

fn key_value(line: &str) -> Option<(String, String)> {
    let bytes = line.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return None;
    }
    let mut index = 1;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' {
            index += 1;
            continue;
        }
        break;
    }
    let key = &line[..index];
    let rest = line[index..].trim_start_matches([' ', '\t']);
    let value = rest.strip_prefix(':')?;
    Some((
        key.to_string(),
        value.trim_start_matches([' ', '\t']).to_string(),
    ))
}

fn parse_scalar_value(raw_value: &str) -> Value {
    let value = strip_inline_comment(raw_value.trim());
    if value == "true" {
        return Value::Bool(true);
    }
    if value == "false" {
        return Value::Bool(false);
    }
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        if let Ok(number) = value.parse::<u64>() {
            return json!(number);
        }
    }
    if value.starts_with('[') && value.ends_with(']') && value.len() >= 2 {
        return Value::Array(
            split_top_level_list(&value[1..value.len() - 1])
                .into_iter()
                .map(|item| Value::String(unquote_scalar(&strip_inline_comment(item.trim()))))
                .collect(),
        );
    }
    if value.starts_with('{') && value.ends_with('}') {
        return serde_json::from_str(&value).unwrap_or_else(|_| Value::String(value.to_string()));
    }
    Value::String(unquote_scalar(&value))
}

fn strip_inline_comment(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut quote: Option<u8> = None;
    for index in 0..bytes.len() {
        let character = bytes[index];
        if (character == b'"' || character == b'\'') && (index == 0 || bytes[index - 1] != b'\\') {
            quote = if quote == Some(character) {
                None
            } else if quote.is_none() {
                Some(character)
            } else {
                quote
            };
        }
        if quote.is_none()
            && character == b'#'
            && index > 0
            && matches!(bytes[index - 1], b' ' | b'\t')
        {
            return value[..index].trim_end_matches([' ', '\t']).to_string();
        }
    }
    value.to_string()
}

fn split_top_level_list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut paren_depth = 0usize;
    for character in value.chars() {
        if (character == '"' || character == '\'') && quote.is_none() {
            quote = Some(character);
            current.push(character);
            continue;
        }
        if quote == Some(character) {
            quote = None;
            current.push(character);
            continue;
        }
        if quote.is_none() && character == '(' {
            paren_depth += 1;
        }
        if quote.is_none() && character == ')' {
            paren_depth = paren_depth.saturating_sub(1);
        }
        if quote.is_none() && paren_depth == 0 && character == ',' {
            if !current.trim().is_empty() {
                items.push(current.trim().to_string());
            }
            current.clear();
            continue;
        }
        current.push(character);
    }
    if !current.trim().is_empty() {
        items.push(current.trim().to_string());
    }
    items
}

fn unquote_scalar(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        return value[1..value.len() - 1].to_string();
    }
    value.to_string()
}

fn scalar_string(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_enum(value: Option<&Value>, valid: &[&str]) -> Option<String> {
    let raw = scalar_string(value)?;
    valid.contains(&raw.as_str()).then_some(raw)
}

fn parse_optional_boolean(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(value) => Some(*value),
        Value::String(text) => {
            let lowered = text.to_lowercase();
            if lowered == "true" {
                Some(true)
            } else if lowered == "false" {
                Some(false)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn normalize_positive_integer(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => {
            let value = number.as_u64()?;
            (value > 0).then_some(value)
        }
        Value::String(text)
            if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            text.parse::<u64>().ok().filter(|value| *value > 0)
        }
        _ => None,
    }
}

fn parse_string_list(value: Option<&Value>) -> Option<Vec<String>> {
    match value? {
        Value::Array(items) => non_empty_list(
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .map(str::to_string)
                .collect(),
        ),
        Value::String(text) => non_empty_list(
            text.split(|character: char| character == ',' || js::is_js_space(character))
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        _ => None,
    }
}

fn parse_tool_spec_list(value: Option<&Value>) -> Option<Vec<String>> {
    match value? {
        Value::Array(items) => non_empty_list(
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .map(str::to_string)
                .collect(),
        ),
        Value::String(text) => {
            let mut result = Vec::new();
            let mut current = String::new();
            let mut paren_depth = 0usize;
            for character in text.chars() {
                if character == '(' {
                    paren_depth += 1;
                }
                if character == ')' {
                    paren_depth = paren_depth.saturating_sub(1);
                }
                if paren_depth == 0 && (character == ',' || js::is_js_space(character)) {
                    if !current.trim().is_empty() {
                        result.push(current.trim().to_string());
                    }
                    current.clear();
                    continue;
                }
                current.push(character);
            }
            if !current.trim().is_empty() {
                result.push(current.trim().to_string());
            }
            non_empty_list(result)
        }
        _ => None,
    }
}

fn non_empty_list(values: Vec<String>) -> Option<Vec<String>> {
    let filtered: Vec<String> = values
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect();
    (!filtered.is_empty()).then_some(filtered)
}

// ---------------------------------------------------------------------------
// 序列化
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentConfig {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub color: Option<String>,
    pub model_selection: Option<Selection>,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub inject_agents_md: Option<bool>,
    pub skills: Option<Vec<String>>,
    pub permission_mode: Option<String>,
    pub max_turns: Option<u64>,
    pub background: Option<bool>,
    pub mcp_servers: Option<Vec<Value>>,
}

pub fn serialize_agent_markdown(config: &AgentConfig) -> String {
    let mut lines = vec![
        format!("name: \"{}\"", escape_yaml_string(&config.name)),
        format!(
            "description: \"{}\"",
            escape_yaml_string(&config.description)
        ),
    ];
    append_scalar(&mut lines, "color", config.color.as_deref());
    if let Some(selection) = &config.model_selection {
        append_scalar(&mut lines, "model", Some(&format_markdown_model(selection)));
        append_scalar(
            &mut lines,
            "thoughtLevel",
            selection
                .options
                .as_ref()
                .map(|options| options.reasoning_level.as_str()),
        );
    }
    append_list(&mut lines, "tools", config.tools.as_deref());
    append_list(
        &mut lines,
        "disallowedTools",
        config.disallowed_tools.as_deref(),
    );
    append_list(&mut lines, "skills", config.skills.as_deref());
    append_scalar(
        &mut lines,
        "permissionMode",
        config.permission_mode.as_deref(),
    );
    if let Some(turns) = config.max_turns {
        lines.push(format!("maxTurns: {turns}"));
    }
    if let Some(value) = config.background {
        lines.push(format!(
            "background: {}",
            if value { "true" } else { "false" }
        ));
    }
    if let Some(value) = config.inject_agents_md {
        lines.push(format!(
            "injectAgentsMd: {}",
            if value { "true" } else { "false" }
        ));
    }
    append_unknown_list(&mut lines, "mcpServers", config.mcp_servers.as_deref());

    format!(
        "---\n{}\n---\n{}",
        lines.join("\n"),
        format_body(&config.system_prompt)
    )
}

fn format_body(system_prompt: &str) -> String {
    let body = system_prompt.trim();
    if body.is_empty() {
        "\n".to_string()
    } else {
        format!("\n{body}\n")
    }
}

fn append_scalar(lines: &mut Vec<String>, key: &str, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    if value.is_empty() {
        return;
    }
    lines.push(format!("{key}: {}", format_yaml_scalar(value)));
}

fn append_list(lines: &mut Vec<String>, key: &str, values: Option<&[String]>) {
    let Some(values) = values else {
        return;
    };
    if values.is_empty() {
        return;
    }
    lines.push(format!("{key}:"));
    for value in values {
        lines.push(format!("  - {}", format_yaml_scalar(value)));
    }
}

fn append_unknown_list(lines: &mut Vec<String>, key: &str, values: Option<&[Value]>) {
    let Some(values) = values else {
        return;
    };
    if values.is_empty() {
        return;
    }
    if values.iter().all(Value::is_string) {
        let strings: Vec<String> = values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        append_list(lines, key, Some(&strings));
        return;
    }
    let mut map = Map::new();
    map.insert(key.to_string(), Value::Array(values.to_vec()));
    let serialized = yaml_stringify_block(&Value::Object(map));
    lines.extend(serialized.lines().map(str::to_string));
}

fn format_yaml_scalar(value: &str) -> String {
    if is_safe_plain_yaml_scalar(value) {
        value.to_string()
    } else {
        format!("\"{}\"", escape_yaml_string(value))
    }
}

fn is_safe_plain_yaml_scalar(value: &str) -> bool {
    if value.is_empty() || value.trim() != value {
        return false;
    }
    let lowered = value.to_lowercase();
    if matches!(lowered.as_str(), "false" | "null" | "true" | "~") {
        return false;
    }
    if is_numeric_scalar(value) {
        return false;
    }
    let first = value.chars().next().unwrap_or(' ');
    if matches!(
        first,
        '*' | '?'
            | ':'
            | ','
            | '['
            | ']'
            | '{'
            | '}'
            | '&'
            | '!'
            | '|'
            | '>'
            | '\''
            | '"'
            | '%'
            | '@'
            | '`'
            | '-'
    ) {
        return false;
    }
    if value.contains('#') || value.contains(": ") {
        return false;
    }
    value.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || matches!(
                character,
                '_' | '.' | '/' | '@' | '*' | ' ' | '(' | ')' | '-'
            )
    })
}

fn is_numeric_scalar(value: &str) -> bool {
    let trimmed = value.strip_prefix(['+', '-']).unwrap_or(value);
    if trimmed.is_empty() {
        return false;
    }
    // /^(?:\d+|\d*\.\d+)(?:e[-+]?\d+)?$/i
    let bytes = trimmed.as_bytes();
    let mut index = 0;
    let mut integer_digits = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        integer_digits += 1;
        index += 1;
    }
    let mut fractional_digits = 0;
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            fractional_digits += 1;
            index += 1;
        }
        if fractional_digits == 0 {
            return false;
        }
    } else if integer_digits == 0 {
        return false;
    }
    if index < bytes.len() && (bytes[index] == b'e' || bytes[index] == b'E') {
        index += 1;
        if index < bytes.len() && matches!(bytes[index], b'+' | b'-') {
            index += 1;
        }
        let exponent_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }
    index == bytes.len()
}

fn escape_yaml_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

// ---------------------------------------------------------------------------
// YAML 子集
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct YamlLine {
    indent: usize,
    text: String,
}

pub fn parse_yaml_subset(yaml: &str) -> Result<Map<String, Value>, ()> {
    let lines = tokenize_yaml_lines(yaml)?;
    if lines.is_empty() {
        return Ok(Map::new());
    }
    let mut index = 0;
    let value = parse_yaml_block(&lines, &mut index, lines[0].indent)?;
    if index < lines.len() {
        return Err(());
    }
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(()),
    }
}

fn tokenize_yaml_lines(yaml: &str) -> Result<Vec<YamlLine>, ()> {
    let mut lines = Vec::new();
    for raw in yaml.split('\n') {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let trimmed_start = line.trim_start_matches(' ');
        let indent = line.len() - trimmed_start.len();
        if trimmed_start.starts_with('\t') {
            return Err(());
        }
        let text = strip_yaml_comment(trimmed_start);
        if text.trim().is_empty() {
            continue;
        }
        lines.push(YamlLine {
            indent,
            text: text.trim_end().to_string(),
        });
    }
    Ok(lines)
}

fn strip_yaml_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    let mut quote: Option<u8> = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if (byte == b'"' || byte == b'\'') && (index == 0 || bytes[index - 1] != b'\\') {
            if quote == Some(byte) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(byte);
            }
        }
        if quote.is_none()
            && byte == b'#'
            && (index == 0 || matches!(bytes[index - 1], b' ' | b'\t'))
        {
            return &value[..index];
        }
        index += 1;
    }
    value
}

fn parse_yaml_block(lines: &[YamlLine], index: &mut usize, indent: usize) -> Result<Value, ()> {
    if lines[*index].text.starts_with("- ") || lines[*index].text == "-" {
        parse_yaml_sequence(lines, index, indent)
    } else {
        parse_yaml_mapping(lines, index, indent)
    }
}

fn parse_yaml_mapping(lines: &[YamlLine], index: &mut usize, indent: usize) -> Result<Value, ()> {
    let mut map = Map::new();
    while *index < lines.len() {
        let line = &lines[*index];
        if line.indent < indent {
            break;
        }
        if line.indent > indent {
            return Err(());
        }
        if line.text.starts_with("- ") || line.text == "-" {
            break;
        }
        let (key, rest) = split_yaml_key(&line.text)?;
        *index += 1;
        let value = if rest.is_empty() {
            if *index < lines.len() && lines[*index].indent > indent {
                let child_indent = lines[*index].indent;
                parse_yaml_block(lines, index, child_indent)?
            } else {
                Value::Null
            }
        } else if let Some(indicator) = rest.chars().next().filter(|c| *c == '|' || *c == '>') {
            parse_block_scalar(lines, index, indent, indicator)?
        } else {
            parse_yaml_scalar(rest)?
        };
        map.insert(key, value);
    }
    Ok(Value::Object(map))
}

fn parse_yaml_sequence(lines: &[YamlLine], index: &mut usize, indent: usize) -> Result<Value, ()> {
    let mut items = Vec::new();
    while *index < lines.len() {
        let line = &lines[*index];
        if line.indent != indent {
            break;
        }
        let Some(rest) = line.text.strip_prefix('-') else {
            break;
        };
        if !rest.is_empty() && !rest.starts_with(' ') {
            break;
        }
        let item_text = rest.trim_start();
        *index += 1;
        if item_text.is_empty() {
            if *index < lines.len() && lines[*index].indent > indent {
                let child_indent = lines[*index].indent;
                items.push(parse_yaml_block(lines, index, child_indent)?);
            } else {
                items.push(Value::Null);
            }
            continue;
        }
        // `- key: value`：把同一行剩余部分当作映射的首项，后续更深缩进行继续补全。
        if let Ok((key, value_text)) = split_yaml_key(item_text) {
            let mut map = Map::new();
            let value = if value_text.is_empty() {
                if *index < lines.len() && lines[*index].indent > indent {
                    let child_indent = lines[*index].indent;
                    parse_yaml_block(lines, index, child_indent)?
                } else {
                    Value::Null
                }
            } else if let Some(indicator) =
                value_text.chars().next().filter(|c| *c == '|' || *c == '>')
            {
                parse_block_scalar(lines, index, indent, indicator)?
            } else {
                parse_yaml_scalar(value_text)?
            };
            map.insert(key, value);
            let continuation_indent = if *index < lines.len() && lines[*index].indent > indent {
                lines[*index].indent
            } else {
                usize::MAX
            };
            if continuation_indent != usize::MAX {
                match parse_yaml_mapping(lines, index, continuation_indent)? {
                    Value::Object(rest_map) => {
                        for (key, value) in rest_map {
                            map.insert(key, value);
                        }
                    }
                    _ => return Err(()),
                }
            }
            items.push(Value::Object(map));
            continue;
        }
        items.push(parse_yaml_scalar(item_text)?);
    }
    Ok(Value::Array(items))
}

fn parse_block_scalar(
    lines: &[YamlLine],
    index: &mut usize,
    parent_indent: usize,
    indicator: char,
) -> Result<Value, ()> {
    let mut parts = Vec::new();
    let mut content_indent: Option<usize> = None;
    while *index < lines.len() && lines[*index].indent > parent_indent {
        let line = &lines[*index];
        let indent = *content_indent.get_or_insert(line.indent);
        if line.indent < indent {
            break;
        }
        let padding = line.indent - indent;
        parts.push(format!("{}{}", " ".repeat(padding), line.text));
        *index += 1;
    }
    let value = if indicator == '>' {
        parts.join(" ")
    } else {
        parts.join("\n")
    };
    Ok(Value::String(format!("{value}\n")))
}

fn split_yaml_key(text: &str) -> Result<(String, &str), ()> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Err(());
    }
    if bytes[0] == b'"' || bytes[0] == b'\'' {
        let quote = bytes[0];
        let mut index = 1;
        while index < bytes.len() {
            if bytes[index] == quote {
                if quote == b'\'' && index + 1 < bytes.len() && bytes[index + 1] == b'\'' {
                    index += 2;
                    continue;
                }
                let raw_key = &text[1..index];
                let rest = text[index + 1..].trim_start_matches([' ', '\t']);
                let rest = rest.strip_prefix(':').ok_or(())?;
                return Ok((unquote_key(raw_key), rest.trim_start_matches([' ', '\t'])));
            }
            index += 1;
        }
        return Err(());
    }
    let mut index = 1;
    while index < bytes.len() {
        if bytes[index] == b':' {
            let rest = &text[index + 1..];
            if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') {
                let key = js::js_trim(&text[..index]).to_string();
                if key.is_empty() {
                    return Err(());
                }
                return Ok((key, rest.trim_start_matches([' ', '\t'])));
            }
        }
        index += 1;
    }
    Err(())
}

fn unquote_key(raw: &str) -> String {
    raw.replace("''", "'").replace("\\\"", "\"")
}

fn parse_yaml_scalar(text: &str) -> Result<Value, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Value::Null);
    }
    let first = text.chars().next().unwrap_or(' ');
    if first == '"' {
        return serde_json::from_str::<String>(text)
            .map(Value::String)
            .map_err(|_| ());
    }
    if first == '\'' {
        if !text.ends_with('\'') || text.len() < 2 {
            return Err(());
        }
        return Ok(Value::String(text[1..text.len() - 1].replace("''", "'")));
    }
    if text.starts_with('[') || text.starts_with('{') {
        return serde_json::from_str::<Value>(text).map_err(|_| ());
    }
    match text {
        "true" | "True" | "TRUE" => return Ok(Value::Bool(true)),
        "false" | "False" | "FALSE" => return Ok(Value::Bool(false)),
        "null" | "Null" | "NULL" | "~" => return Ok(Value::Null),
        _ => {}
    }
    if is_yaml_integer(text) {
        if let Ok(value) = text.parse::<i64>() {
            return Ok(json!(value));
        }
    }
    if let Ok(value) = text.parse::<f64>() {
        if text.contains('.') || text.contains('e') || text.contains('E') {
            if let Some(number) = serde_json::Number::from_f64(value) {
                return Ok(Value::Number(number));
            }
        }
    }
    Ok(Value::String(text.to_string()))
}

fn is_yaml_integer(text: &str) -> bool {
    let trimmed = text.strip_prefix(['+', '-']).unwrap_or(text);
    !trimmed.is_empty() && trimmed.bytes().all(|byte| byte.is_ascii_digit())
}

/// `yaml.stringify({ key: value }, { lineWidth: 0 })` 的块风格近似。
pub fn yaml_stringify_block(value: &Value) -> String {
    let mut out = String::new();
    emit_yaml(&mut out, value, 0, None);
    out.trim_end().to_string()
}

fn emit_yaml(out: &mut String, value: &Value, indent: usize, prefix: Option<&str>) {
    match value {
        Value::Object(map) => {
            if let Some(prefix) = prefix {
                out.push_str(&" ".repeat(indent));
                out.push_str(prefix);
                out.push_str(":\n");
            }
            for (key, child) in map {
                emit_yaml_entry(
                    out,
                    key,
                    child,
                    indent + if prefix.is_some() { 2 } else { 0 },
                );
            }
        }
        Value::Array(items) => {
            if let Some(prefix) = prefix {
                out.push_str(&" ".repeat(indent));
                out.push_str(prefix);
                out.push_str(":\n");
            }
            for item in items {
                emit_yaml_sequence_item(out, item, indent + if prefix.is_some() { 2 } else { 0 });
            }
        }
        other => {
            if let Some(prefix) = prefix {
                out.push_str(&" ".repeat(indent));
                out.push_str(prefix);
                out.push_str(": ");
                out.push_str(&yaml_scalar(other));
                out.push('\n');
            }
        }
    }
}

fn emit_yaml_entry(out: &mut String, key: &str, value: &Value, indent: usize) {
    match value {
        Value::Object(map) if !map.is_empty() => emit_yaml(out, value, indent, Some(key)),
        Value::Array(items) if !items.is_empty() => emit_yaml(out, value, indent, Some(key)),
        _ => {
            out.push_str(&" ".repeat(indent));
            out.push_str(key);
            out.push_str(": ");
            out.push_str(&yaml_scalar(value));
            out.push('\n');
        }
    }
}

fn emit_yaml_sequence_item(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            let mut first = true;
            for (key, child) in map {
                let entry_indent = if first { indent } else { indent + 2 };
                if first {
                    out.push_str(&" ".repeat(indent));
                    out.push_str("- ");
                    first = false;
                } else {
                    out.push_str(&" ".repeat(entry_indent));
                }
                match child {
                    Value::Object(nested) if !nested.is_empty() => {
                        out.push_str(key);
                        out.push_str(":\n");
                        for (nested_key, nested_value) in nested {
                            emit_yaml_entry(out, nested_key, nested_value, entry_indent + 2);
                        }
                    }
                    Value::Array(items) if !items.is_empty() => {
                        out.push_str(key);
                        out.push_str(":\n");
                        for item in items {
                            emit_yaml_sequence_item(out, item, entry_indent + 2);
                        }
                    }
                    _ => {
                        out.push_str(key);
                        out.push_str(": ");
                        out.push_str(&yaml_scalar(child));
                        out.push('\n');
                    }
                }
            }
        }
        Value::Array(items) if !items.is_empty() => {
            out.push_str(&" ".repeat(indent));
            out.push_str("-\n");
            for item in items {
                emit_yaml_sequence_item(out, item, indent + 2);
            }
        }
        _ => {
            out.push_str(&" ".repeat(indent));
            out.push_str("- ");
            out.push_str(&yaml_scalar(value));
            out.push('\n');
        }
    }
}

fn yaml_scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => {
            if is_safe_plain_yaml_scalar(text) {
                text.clone()
            } else {
                format!("\"{}\"", escape_yaml_string(text))
            }
        }
        Value::Array(items) if items.is_empty() => "[]".to_string(),
        Value::Object(map) if map.is_empty() => "{}".to_string(),
        other => format!("\"{}\"", escape_yaml_string(&other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_loose_fields() {
        let content = "---\nname: \"alpha\"\ndescription: 'desc'\ntools: Read, Grep(path a)\nmaxTurns: 3\nbackground: true\n---\nbody\n";
        let parsed = parse_agent_markdown(content, "/ws/a.md", "user");
        let agent = parsed.agent.unwrap();
        assert_eq!(agent.name, "alpha");
        assert_eq!(agent.description, "desc");
        assert_eq!(agent.id, "user:user:alpha");
        assert_eq!(agent.system_prompt, "body");
        assert_eq!(
            agent.tools,
            Some(vec!["Read".to_string(), "Grep(path a)".to_string()])
        );
        assert_eq!(agent.max_turns, Some(3));
        assert_eq!(agent.background, Some(true));
        assert_eq!(agent.read_only, Some(false));
    }

    #[test]
    fn missing_required_fields_produce_diagnostics() {
        let parsed = parse_agent_markdown("---\nname: x\n---\n", "/ws/a.md", "user");
        let diagnostic = parsed.diagnostic.unwrap();
        assert_eq!(diagnostic.code, "agent_missing_required_frontmatter");
        assert!(diagnostic.message.ends_with("/ws/a.md"));

        let parsed = parse_agent_markdown("no frontmatter", "/ws/a.md", "user");
        assert_eq!(parsed.diagnostic.unwrap().code, "agent_missing_frontmatter");
    }

    #[test]
    fn yaml_subset_parses_nested_mcp_servers() {
        let yaml = "name: x\nmcpServers:\n  - name: fs\n    command: npx\n    args:\n      - -y\n      - pkg\n";
        let parsed = parse_yaml_subset(yaml).unwrap();
        let servers = parsed.get("mcpServers").unwrap().as_array().unwrap();
        assert_eq!(servers[0]["name"], "fs");
        assert_eq!(servers[0]["command"], "npx");
        assert_eq!(servers[0]["args"], json!(["-y", "pkg"]));
    }

    #[test]
    fn serializes_agent_markdown_with_quoted_name_and_body() {
        let config = AgentConfig {
            name: "demo".into(),
            description: "a: b".into(),
            system_prompt: "do it".into(),
            tools: Some(vec!["Read".into()]),
            model_selection: Some(Selection {
                provider_id: "glm".into(),
                model_id: "GLM-4.7".into(),
                options: None,
            }),
            ..AgentConfig::default()
        };
        assert_eq!(
            serialize_agent_markdown(&config),
            "---\nname: \"demo\"\ndescription: \"a: b\"\nmodel: glm/GLM-4.7\ntools:\n  - Read\n---\n\ndo it\n"
        );
    }

    #[test]
    fn serializes_object_mcp_servers_as_block_yaml() {
        let config = AgentConfig {
            name: "demo".into(),
            description: "d".into(),
            system_prompt: "s".into(),
            mcp_servers: Some(vec![json!({ "name": "fs", "command": "npx" })]),
            ..AgentConfig::default()
        };
        let text = serialize_agent_markdown(&config);
        // 对象键按 serde_json 的字母序写出（TS 保留插入序），见 spec §已知差异。
        assert!(text.contains("mcpServers:\n  - command: npx\n    name: fs\n"));
        // 写出的 Markdown 必须能被自己的解析器读回。
        let parsed = parse_agent_markdown(&text, "/ws/a.md", "user");
        assert_eq!(
            parsed.agent.unwrap().mcp_servers,
            Some(vec![json!({ "name": "fs", "command": "npx" })])
        );
    }

    #[test]
    fn round_trip_preserves_real_newlines_in_description() {
        let config = AgentConfig {
            name: "demo".into(),
            description: "line1\nline2".into(),
            system_prompt: "body".into(),
            ..AgentConfig::default()
        };
        let text = serialize_agent_markdown(&config);
        let parsed = parse_agent_markdown(&text, "/ws/a.md", "user")
            .agent
            .unwrap();
        assert_eq!(parsed.description, "line1\nline2");
    }
}
