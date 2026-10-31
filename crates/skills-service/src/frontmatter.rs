//! `SKILL.md` frontmatter 解析。
//!
//! 真源：`skillsService.ts` 的 `readFrontmatter` / `splitFrontmatterAndLeakedBody` /
//! `readLooseFrontmatterFields` / `hasYamlUnsafeLooseInlineField` / `readFrontmatterString`。
//!
//! TS 用 npm `yaml` 包做严格解析；这里不引入 YAML 依赖，改为受支持子集，
//! 子集外一律按「严格解析失败」回退宽松字段（与 TS 在 YAML 报错时的行为一致）。
//! 边界见 `docs/specs/skills-service.md` §5.1。

use crate::fsx;

#[derive(Debug, Clone, PartialEq)]
pub struct Frontmatter {
    pub has_frontmatter: bool,
    pub name: String,
    pub description: String,
    pub body: String,
}

pub fn read_frontmatter(content: &str) -> Frontmatter {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let Some((frontmatter_text, after)) = split_frontmatter(&normalized) else {
        return Frontmatter {
            has_frontmatter: false,
            name: String::new(),
            description: String::new(),
            body: fsx::js_trim(&normalized).to_string(),
        };
    };

    let (metadata_text, leaked_body) = split_frontmatter_and_leaked_body(&frontmatter_text);
    let body_after = fsx::js_trim(after);
    let body = [leaked_body.as_str(), body_after]
        .iter()
        .filter(|part| !fsx::js_trim(part).is_empty())
        .copied()
        .collect::<Vec<&str>>()
        .join("\n\n");
    let body = fsx::js_trim(&body).to_string();

    let (loose_name, loose_description) = read_loose_frontmatter_fields(&metadata_text);
    let loose = || Frontmatter {
        has_frontmatter: true,
        name: loose_name.clone(),
        description: loose_description.clone(),
        body: body.clone(),
    };

    if has_yaml_unsafe_loose_inline_field(&metadata_text) {
        return loose();
    }
    let Some(entries) = parse_strict_mapping(&metadata_text) else {
        return loose();
    };

    let strict_name = entries
        .iter()
        .find(|(key, _)| key == "name")
        .map(|(_, value)| read_frontmatter_string(value))
        .unwrap_or_default();
    let strict_description = entries
        .iter()
        .find(|(key, _)| key == "description")
        .map(|(_, value)| read_frontmatter_string(value))
        .unwrap_or_default();

    Frontmatter {
        has_frontmatter: true,
        name: if strict_name.is_empty() {
            loose_name
        } else {
            strict_name
        },
        description: if strict_description.is_empty() {
            loose_description
        } else {
            strict_description
        },
        body,
    }
}

/// `^---\n([\s\S]*?)\n---(?:\n|$)`，返回 (frontmatter 文本, 其后的正文)。
fn split_frontmatter(normalized: &str) -> Option<(String, &str)> {
    let rest = normalized.strip_prefix("---\n")?;
    let mut search = 0;
    while let Some(position) = rest[search..].find("\n---") {
        let index = search + position;
        let after_dashes = index + 4;
        if after_dashes == rest.len() {
            return Some((rest[..index].to_string(), ""));
        }
        if rest[after_dashes..].starts_with('\n') {
            return Some((rest[..index].to_string(), &rest[after_dashes + 1..]));
        }
        search = index + 1;
    }
    None
}

fn split_frontmatter_and_leaked_body(frontmatter_text: &str) -> (String, String) {
    let lines: Vec<&str> = frontmatter_text.split('\n').collect();
    let leaked_index = lines
        .iter()
        .enumerate()
        .position(|(index, line)| index > 0 && is_markdown_heading(line));
    match leaked_index {
        None => (frontmatter_text.to_string(), String::new()),
        Some(index) => (
            fsx::js_trim_end(&lines[..index].join("\n")).to_string(),
            fsx::js_trim(&lines[index..].join("\n")).to_string(),
        ),
    }
}

/// `/^#{1,6}\s+\S/`。
fn is_markdown_heading(line: &str) -> bool {
    let hashes = line.chars().take_while(|value| *value == '#').count();
    if hashes == 0 || hashes > 6 {
        return false;
    }
    let rest = &line[hashes..];
    let mut chars = rest.chars();
    match chars.next() {
        Some(value) if fsx::is_js_space(value) => {}
        _ => return false,
    }
    chars.any(|value| !fsx::is_js_space(value))
}

fn read_loose_frontmatter_fields(frontmatter_text: &str) -> (String, String) {
    let mut name = String::new();
    let mut description = String::new();
    let lines: Vec<&str> = frontmatter_text.split('\n').collect();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if let Some(colon) = line.find(':') {
            let key = fsx::js_trim(&line[..colon]);
            let raw_value = fsx::js_trim(&line[colon + 1..]);
            if key == "name" {
                name = loose_inline_string(raw_value);
            } else if key == "description" {
                let style = raw_value.chars().next();
                if style == Some('|') || style == Some('>') {
                    let mut block: Vec<String> = Vec::new();
                    let mut next = index + 1;
                    while next < lines.len() {
                        let candidate = lines[next];
                        if !(candidate.starts_with(' ') || candidate.starts_with('\t')) {
                            break;
                        }
                        block.push(strip_one_or_two_indent(candidate));
                        index = next;
                        next += 1;
                    }
                    description = if style == Some('>') {
                        block.join(" ")
                    } else {
                        block.join("\n")
                    };
                } else {
                    description = loose_inline_string(raw_value);
                }
            }
        }
        index += 1;
    }
    (name, description)
}

/// `/^\s{1,2}/` 的替换：最多去掉两个前导 JS 空白。
fn strip_one_or_two_indent(line: &str) -> String {
    let mut rest = line;
    for _ in 0..2 {
        match rest.chars().next() {
            Some(value) if fsx::is_js_space(value) => rest = &rest[value.len_utf8()..],
            _ => break,
        }
    }
    rest.to_string()
}

/// `value.trim().replace(/^["']|["']$/g, "")`。
fn loose_inline_string(value: &str) -> String {
    let trimmed = fsx::js_trim(value);
    let mut start = 0;
    let mut end = trimmed.len();
    let bytes = trimmed.as_bytes();
    if start < end && (bytes[start] == b'"' || bytes[start] == b'\'') {
        start += 1;
    }
    if start < end && (bytes[end - 1] == b'"' || bytes[end - 1] == b'\'') {
        end -= 1;
    }
    trimmed[start..end].to_string()
}

fn has_yaml_unsafe_loose_inline_field(frontmatter_text: &str) -> bool {
    for line in frontmatter_text.split('\n') {
        let Some(colon) = line.find(':') else {
            continue;
        };
        let key = fsx::js_trim(&line[..colon]);
        if key != "name" && key != "description" {
            continue;
        }
        let raw_value = fsx::js_trim(&line[colon + 1..]);
        if raw_value.is_empty()
            || raw_value.starts_with('"')
            || raw_value.starts_with('\'')
            || raw_value.starts_with('|')
            || raw_value.starts_with('>')
        {
            continue;
        }
        if raw_value.contains(": ") {
            return true;
        }
    }
    false
}

/// JS `String(value)` 语义（`readFrontmatterString` 的非空分支）。
fn read_frontmatter_string(value: &YamlValue) -> String {
    value.to_js_string()
}

#[derive(Debug, Clone, PartialEq)]
enum YamlValue {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Object,
    Array(Vec<YamlValue>),
}

impl YamlValue {
    fn to_js_string(&self) -> String {
        match self {
            YamlValue::Null => String::new(),
            YamlValue::Bool(value) => value.to_string(),
            YamlValue::Number(value) => js_number_to_string(*value),
            YamlValue::Str(value) => value.clone(),
            YamlValue::Object => "[object Object]".to_string(),
            YamlValue::Array(items) => items
                .iter()
                .map(YamlValue::to_js_string)
                .collect::<Vec<String>>()
                .join(","),
        }
    }
}

fn js_number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if value == 0.0 {
        return "0".to_string();
    }
    if value.fract() == 0.0 && value.abs() < 1e21 {
        return format!("{}", value as i128);
    }
    format!("{value}")
}

/// 严格解析：只支持 §5.1 的子集；任何不确定的语法都返回 `None`（回退宽松）。
fn parse_strict_mapping(text: &str) -> Option<Vec<(String, YamlValue)>> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut entries: Vec<(String, YamlValue)> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if line.trim_matches([' ', '\t']).is_empty() {
            index += 1;
            continue;
        }
        if line.starts_with('\t') || line.starts_with(' ') {
            // 顶层缩进行必须已被上一个键作为值消费掉，否则是非法结构。
            return None;
        }
        let trimmed = line.trim_end_matches([' ', '\t']);
        if trimmed.starts_with('#') {
            index += 1;
            continue;
        }
        if trimmed == "---" || trimmed == "..." {
            return None;
        }

        let (key, raw_value) = split_key_value(trimmed)?;
        if entries.iter().any(|(existing, _)| existing == &key) {
            return None;
        }
        let (value, last_line) = parse_value(&lines, index, raw_value)?;
        entries.push((key, value));
        index = last_line + 1;
    }
    Some(entries)
}

fn split_key_value(line: &str) -> Option<(String, &str)> {
    let first = line.chars().next()?;
    if first == '"' || first == '\'' {
        let rest = &line[1..];
        let mut closing = None;
        let mut chars = rest.char_indices();
        while let Some((position, value)) = chars.next() {
            if value == first {
                if first == '\'' && rest[position + 1..].starts_with('\'') {
                    chars.next();
                    continue;
                }
                closing = Some(position);
                break;
            }
            if first == '"' && value == '\\' {
                chars.next();
            }
        }
        let closing = closing?;
        let key = unescape_quoted(&rest[..closing], first)?;
        let after = rest[closing + 1..].trim_start_matches([' ', '\t']);
        let value = after.strip_prefix(':')?;
        return Some((key, value));
    }

    let mut colon = None;
    for (position, value) in line.char_indices() {
        if value != ':' {
            continue;
        }
        let following = line[position + 1..].chars().next();
        if following.map(fsx::is_js_space).unwrap_or(true) {
            colon = Some(position);
            break;
        }
    }
    let colon = colon?;
    let key = fsx::js_trim(&line[..colon]).to_string();
    if key.is_empty() {
        return None;
    }
    Some((key, &line[colon + 1..]))
}

/// 解析一个键的值，返回 (值, 该值消费到的最后一行下标)。
fn parse_value(lines: &[&str], index: usize, raw_value: &str) -> Option<(YamlValue, usize)> {
    let raw = fsx::js_trim(raw_value);

    if raw.is_empty() {
        // 空值：后接缩进行 = 嵌套结构；否则 Null。
        let mut next = index + 1;
        let mut last_indented = None;
        while next < lines.len() {
            let candidate = lines[next];
            if candidate.trim_matches([' ', '\t']).is_empty() {
                next += 1;
                continue;
            }
            if candidate.starts_with(' ') || candidate.starts_with('\t') {
                last_indented = Some(next);
                next += 1;
                continue;
            }
            break;
        }
        return match last_indented {
            Some(last) => Some((YamlValue::Object, last)),
            None => Some((YamlValue::Null, index)),
        };
    }

    if raw.starts_with('#') {
        return Some((YamlValue::Null, index));
    }

    let first = raw.chars().next()?;
    if first == '|' || first == '>' {
        return parse_block_scalar(lines, index, raw, first);
    }
    if first == '"' || first == '\'' {
        return parse_quoted_scalar(raw, first).map(|value| (value, index));
    }
    if first == '[' || first == '{' {
        let value = parse_flow_collection(raw)?;
        return Some((value, index));
    }
    if matches!(first, '&' | '*' | '!' | '%' | '@' | '`') {
        return None;
    }
    if first == '-' && raw.chars().nth(1).map(fsx::is_js_space).unwrap_or(true) {
        return None;
    }

    // 多行 plain 标量：后续缩进行折叠为空格。
    let mut text = strip_plain_comment(raw)
        .trim_end_matches([' ', '\t'])
        .to_string();
    let mut last = index;
    let mut next = index + 1;
    while next < lines.len() {
        let candidate = lines[next];
        if candidate.trim_matches([' ', '\t']).is_empty() {
            next += 1;
            continue;
        }
        if !(candidate.starts_with(' ') || candidate.starts_with('\t')) {
            break;
        }
        let piece = candidate.trim_matches([' ', '\t']);
        if piece.starts_with('#') {
            break;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(piece);
        last = next;
        next += 1;
    }
    Some((classify_plain_scalar(&text), last))
}

fn strip_plain_comment(raw: &str) -> &str {
    for (position, value) in raw.char_indices() {
        if value != '#' || position == 0 {
            continue;
        }
        let previous = raw[..position].chars().next_back();
        if matches!(previous, Some(' ') | Some('\t')) {
            return &raw[..position];
        }
    }
    raw
}

fn parse_block_scalar(
    lines: &[&str],
    index: usize,
    raw: &str,
    style: char,
) -> Option<(YamlValue, usize)> {
    let mut chomp = ' ';
    let mut explicit_indent = None;
    for value in raw[1..].chars() {
        match value {
            '+' | '-' if chomp == ' ' => chomp = value,
            '1'..='9' if explicit_indent.is_none() => {
                explicit_indent = value.to_digit(10).map(|v| v as usize)
            }
            ' ' | '\t' => break,
            '#' => break,
            _ => return None,
        }
    }

    let mut body: Vec<String> = Vec::new();
    let mut last = index;
    let mut content_indent = explicit_indent;
    let mut next = index + 1;
    while next < lines.len() {
        let candidate = lines[next];
        if candidate.trim_matches([' ', '\t']).is_empty() {
            body.push(String::new());
            last = next;
            next += 1;
            continue;
        }
        let indent = candidate.chars().take_while(|value| *value == ' ').count();
        if candidate.starts_with('\t') {
            return None;
        }
        let required = content_indent.unwrap_or(0);
        if indent < required || (content_indent.is_none() && indent == 0) {
            break;
        }
        if content_indent.is_none() {
            content_indent = Some(indent);
        }
        let strip = content_indent.unwrap_or(0).min(indent);
        body.push(candidate[strip..].to_string());
        last = next;
        next += 1;
    }
    // 末尾的空行属于下一个键之前的分隔，不计入内容。
    while body.last().map(String::is_empty).unwrap_or(false) {
        body.pop();
        last = last.saturating_sub(1);
    }

    let mut text = if style == '>' {
        let mut folded = String::new();
        for line in &body {
            if line.is_empty() {
                folded.push('\n');
            } else {
                if !folded.is_empty() && !folded.ends_with('\n') {
                    folded.push(' ');
                }
                folded.push_str(line);
            }
        }
        folded
    } else {
        body.join("\n")
    };

    match chomp {
        '-' => {
            while text.ends_with('\n') {
                text.pop();
            }
        }
        '+' => {}
        _ => {
            while text.ends_with('\n') {
                text.pop();
            }
            if !text.is_empty() {
                text.push('\n');
            }
        }
    }
    Some((YamlValue::Str(text), last))
}

fn parse_quoted_scalar(raw: &str, quote: char) -> Option<YamlValue> {
    let rest = &raw[1..];
    let mut closing = None;
    let mut chars = rest.char_indices();
    while let Some((position, value)) = chars.next() {
        if value == quote {
            if quote == '\'' && rest[position + 1..].starts_with('\'') {
                chars.next();
                continue;
            }
            closing = Some(position);
            break;
        }
        if quote == '"' && value == '\\' {
            chars.next();
        }
    }
    let closing = closing?;
    let trailing = rest[closing + 1..].trim_start_matches([' ', '\t']);
    if !trailing.is_empty() && !trailing.starts_with('#') {
        return None;
    }
    unescape_quoted(&rest[..closing], quote).map(YamlValue::Str)
}

fn unescape_quoted(text: &str, quote: char) -> Option<String> {
    if quote == '\'' {
        return Some(text.replace("''", "'"));
    }
    let mut result = String::new();
    let mut chars = text.chars();
    while let Some(value) = chars.next() {
        if value != '\\' {
            result.push(value);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            'n' => result.push('\n'),
            't' => result.push('\t'),
            'r' => result.push('\r'),
            '0' => result.push('\0'),
            '\\' => result.push('\\'),
            '"' => result.push('"'),
            '/' => result.push('/'),
            'u' => {
                let mut code = String::new();
                for _ in 0..4 {
                    code.push(chars.next()?);
                }
                let value = u32::from_str_radix(&code, 16).ok()?;
                result.push(char::from_u32(value)?);
            }
            _ => return None,
        }
    }
    Some(result)
}

fn parse_flow_collection(raw: &str) -> Option<YamlValue> {
    let open = raw.chars().next()?;
    let close = if open == '[' { ']' } else { '}' };
    if !raw.ends_with(close) {
        return None;
    }
    let inner = &raw[1..raw.len() - 1];
    if open == '{' {
        if inner.trim().is_empty() {
            return Some(YamlValue::Object);
        }
        return Some(YamlValue::Object);
    }
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return Some(YamlValue::Array(Vec::new()));
    }
    let mut items = Vec::new();
    for part in split_flow_items(trimmed)? {
        let part = fsx::js_trim(&part);
        if part.is_empty() {
            return None;
        }
        items.push(classify_plain_scalar(part));
    }
    Some(YamlValue::Array(items))
}

fn split_flow_items(text: &str) -> Option<Vec<String>> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for value in text.chars() {
        match quote {
            Some(active) => {
                current.push(value);
                if value == active {
                    quote = None;
                }
            }
            None => match value {
                '"' | '\'' => {
                    quote = Some(value);
                    current.push(value);
                }
                '[' | ']' | '{' | '}' => return None,
                ',' => {
                    items.push(current.clone());
                    current.clear();
                }
                _ => current.push(value),
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    items.push(current);
    Some(items)
}

/// YAML 1.2 core 的标量分类（只需覆盖 JS `String()` 的可观测结果）。
fn classify_plain_scalar(text: &str) -> YamlValue {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return YamlValue::Null,
        "true" | "True" | "TRUE" => return YamlValue::Bool(true),
        "false" | "False" | "FALSE" => return YamlValue::Bool(false),
        ".inf" | ".Inf" | ".INF" => return YamlValue::Number(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => return YamlValue::Number(f64::NEG_INFINITY),
        ".nan" | ".NaN" | ".NAN" => return YamlValue::Number(f64::NAN),
        _ => {}
    }
    if let Some(number) = parse_yaml_number(text) {
        return YamlValue::Number(number);
    }
    YamlValue::Str(text.to_string())
}

fn parse_yaml_number(text: &str) -> Option<f64> {
    if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return i128::from_str_radix(rest, 16)
            .ok()
            .map(|value| value as f64);
    }
    if let Some(rest) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
        return i128::from_str_radix(rest, 8).ok().map(|value| value as f64);
    }
    let candidate = text.strip_prefix(['-', '+']).unwrap_or(text);
    if candidate.is_empty() {
        return None;
    }
    let is_integer = candidate.chars().all(|value| value.is_ascii_digit());
    let is_float = {
        let mut seen_dot = false;
        let mut seen_exp = false;
        let mut valid = !candidate.is_empty();
        for (index, value) in candidate.char_indices() {
            match value {
                '0'..='9' => {}
                '.' if !seen_dot && !seen_exp => seen_dot = true,
                'e' | 'E' if !seen_exp && index > 0 => seen_exp = true,
                '+' | '-' if seen_exp && index > 0 => {}
                _ => {
                    valid = false;
                    break;
                }
            }
        }
        valid && (seen_dot || seen_exp)
    };
    if !is_integer && !is_float {
        return None;
    }
    text.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> Frontmatter {
        read_frontmatter(content)
    }

    #[test]
    fn missing_frontmatter_uses_trimmed_body() {
        let parsed = parse("\n  hello  \n");
        assert!(!parsed.has_frontmatter);
        assert_eq!(parsed.name, "");
        assert_eq!(parsed.body, "hello");
    }

    #[test]
    fn strict_scalars_and_block_scalars() {
        let parsed = parse("---\nname: alpha\ndescription: |\n  line one\n  line two\n---\nbody\n");
        assert!(parsed.has_frontmatter);
        assert_eq!(parsed.name, "alpha");
        assert_eq!(parsed.description, "line one\nline two\n");
        assert_eq!(parsed.body, "body");
    }

    #[test]
    fn quoted_values_are_unquoted() {
        let parsed = parse("---\nname: \"alpha\"\ndescription: 'a b'\n---\n");
        assert_eq!(parsed.name, "alpha");
        assert_eq!(parsed.description, "a b");
        assert_eq!(parsed.body, "");
    }

    #[test]
    fn unsafe_inline_field_falls_back_to_loose() {
        let parsed = parse("---\nname: alpha\ndescription: 触发场景: 中文\n---\n");
        assert_eq!(parsed.description, "触发场景: 中文");
    }

    #[test]
    fn crlf_is_normalized() {
        let parsed = parse("---\r\nname: alpha\r\n---\r\n\r\nbody\r\n");
        assert_eq!(parsed.name, "alpha");
        assert_eq!(parsed.body, "body");
    }

    #[test]
    fn leaked_heading_moves_into_body() {
        let parsed = parse("---\nname: alpha\n# Usage\n\nuse it\n---\n");
        assert_eq!(parsed.name, "alpha");
        assert_eq!(parsed.body, "# Usage\n\nuse it");
    }

    #[test]
    fn nested_mapping_is_not_a_string_field() {
        let parsed = parse("---\nname: alpha\nmetadata:\n  owner: x\n---\n");
        assert_eq!(parsed.name, "alpha");
        assert_eq!(parsed.description, "");
    }

    #[test]
    fn numbers_and_booleans_follow_js_string() {
        assert_eq!(classify_plain_scalar("123").to_js_string(), "123");
        assert_eq!(classify_plain_scalar("1.50").to_js_string(), "1.5");
        assert_eq!(classify_plain_scalar("true").to_js_string(), "true");
        assert_eq!(classify_plain_scalar("~").to_js_string(), "");
    }

    #[test]
    fn unterminated_quotes_fall_back() {
        // 严格解析失败 → 宽松读取（引号被剥掉）。
        let parsed = parse("---\nname: \"alpha\n---\n");
        assert_eq!(parsed.name, "alpha");
    }
}
