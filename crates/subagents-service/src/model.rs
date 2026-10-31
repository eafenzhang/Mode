//! Subagent 模型选择的解析、编码与旧 ID 迁移。
//!
//! 真源：`packages/shared/src/model-selection.ts`、`custom-model-value.ts`、
//! `legacy-model-provider-identity.ts`、`official-glm-model-id.ts`、
//! `subagent-markdown-selection.ts`。

use serde::{Deserialize, Serialize};
use serde_json::Value;

const CUSTOM_PREFIX: &str = "custom:";
const INHERIT_NAMES: [&str; 5] = ["inherit", "main", "sonnet", "opus", "haiku"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub provider_id: String,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<SelectionOptions>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionOptions {
    pub reasoning_level: String,
}

/// `modelSelectionSchema.parse`：`.strict()` + 每个字符串 `trim().min(1)`。
pub fn parse_selection_value(value: &Value) -> Option<Selection> {
    let object = value.as_object()?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "providerId" | "modelId" | "options"))
    {
        return None;
    }
    let provider_id = trimmed_non_empty(object.get("providerId"))?;
    let model_id = trimmed_non_empty(object.get("modelId"))?;
    let options = match object.get("options") {
        None => None,
        Some(Value::Object(options)) => {
            if options.keys().any(|key| key.as_str() != "reasoningLevel") {
                return None;
            }
            match options.get("reasoningLevel") {
                None => None,
                Some(value) => Some(SelectionOptions {
                    reasoning_level: trimmed_non_empty(Some(value))?,
                }),
            }
        }
        Some(_) => return None,
    };
    Some(Selection {
        provider_id,
        model_id,
        options,
    })
}

fn trimmed_non_empty(value: Option<&Value>) -> Option<String> {
    let trimmed = value?.as_str()?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// `normalizeSubagentModelSelection`：只保留 `reasoningLevel`；非法值按 zod 语义报错。
pub fn normalize_selection(selection: Option<Selection>) -> Result<Option<Selection>, String> {
    let Some(selection) = selection else {
        return Ok(None);
    };
    let provider_id = selection.provider_id.trim();
    let model_id = selection.model_id.trim();
    if provider_id.is_empty() || model_id.is_empty() {
        return Err("Invalid model selection".to_string());
    }
    let options = selection
        .options
        .as_ref()
        .map(|options| options.reasoning_level.trim())
        .filter(|level| !level.is_empty())
        .map(|level| SelectionOptions {
            reasoning_level: level.to_string(),
        });
    Ok(Some(Selection {
        provider_id: provider_id.to_string(),
        model_id: model_id.to_string(),
        options,
    }))
}

/// `parseSubagentMarkdownSelection`：`model` + `thoughtLevel` 两个 frontmatter 字段。
pub fn parse_markdown_selection(
    model: Option<&Value>,
    thought_level: Option<&Value>,
) -> Option<Selection> {
    let raw = model?.as_str()?;
    let value = raw.trim();
    if value.is_empty() || INHERIT_NAMES.contains(&value) {
        return None;
    }

    let base = match decode_custom_model_value(value) {
        Some(custom) => {
            let provider_id = custom.provider_id.trim();
            let model_name = custom.model_name.as_deref().map(str::trim).unwrap_or("");
            if provider_id.is_empty() || model_name.is_empty() {
                return None;
            }
            Selection {
                provider_id: provider_id.to_string(),
                model_id: model_name.to_string(),
                options: None,
            }
        }
        None => parse_model_picker_value(value)?,
    };

    let reasoning_level = thought_level
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|level| !level.is_empty());
    Some(match reasoning_level {
        Some(level) => Selection {
            options: Some(SelectionOptions {
                reasoning_level: level.to_string(),
            }),
            ..base
        },
        None => base,
    })
}

/// `parseModelPickerValue`：`provider/model` 或 `provider/model$level`。
pub fn parse_model_picker_value(value: &str) -> Option<Selection> {
    let normalized = value.trim();
    let provider_separator = normalized.find('/')?;
    if provider_separator == 0 {
        return None;
    }
    let provider_id = normalized[..provider_separator].trim();
    if provider_id.is_empty() {
        return None;
    }
    let raw_model_id = &normalized[provider_separator + 1..];
    let reasoning_separator = raw_model_id.find('$');
    match reasoning_separator {
        Some(index) if index > 0 && index < raw_model_id.len() - 1 => {
            let model_id = raw_model_id[..index].trim();
            let reasoning_level = raw_model_id[index + 1..].trim();
            if model_id.is_empty() || reasoning_level.is_empty() {
                return None;
            }
            Some(Selection {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
                options: Some(SelectionOptions {
                    reasoning_level: reasoning_level.to_string(),
                }),
            })
        }
        _ => {
            let model_id = raw_model_id.trim();
            if model_id.is_empty() {
                return None;
            }
            Some(Selection {
                provider_id: provider_id.to_string(),
                model_id: model_id.to_string(),
                options: None,
            })
        }
    }
}

/// `formatSubagentMarkdownModel`。
pub fn format_markdown_model(selection: &Selection) -> String {
    if selection.provider_id.starts_with(CUSTOM_PREFIX)
        || selection.provider_id.contains('/')
        || selection.model_id.contains('$')
    {
        return encode_custom_model_value(&selection.provider_id, Some(&selection.model_id));
    }
    format!("{}/{}", selection.provider_id, selection.model_id)
}

pub struct DecodedCustomModelValue {
    pub provider_id: String,
    pub model_name: Option<String>,
}

pub fn decode_custom_model_value(value: &str) -> Option<DecodedCustomModelValue> {
    let body = value.strip_prefix(CUSTOM_PREFIX)?;
    let separator = body.find(':');
    let Some(separator) = separator else {
        return Some(DecodedCustomModelValue {
            provider_id: safe_decode_uri_component(body),
            model_name: None,
        });
    };

    let parts: Vec<&str> = body.split(':').collect();
    if parts.len() >= 3 && parts[0] == "builtin" {
        return Some(DecodedCustomModelValue {
            provider_id: format!("builtin:{}", parts[1]),
            model_name: Some(safe_decode_uri_component(&parts[2..].join(":"))),
        });
    }

    Some(DecodedCustomModelValue {
        provider_id: safe_decode_uri_component(&body[..separator]),
        model_name: Some(safe_decode_uri_component(&body[separator + 1..])),
    })
}

pub fn encode_custom_model_value(provider_id: &str, model_name: Option<&str>) -> String {
    let encoded_provider = encode_uri_component(provider_id);
    match model_name {
        None => format!("{CUSTOM_PREFIX}{encoded_provider}"),
        Some(name) => format!(
            "{CUSTOM_PREFIX}{encoded_provider}:{}",
            encode_uri_component(name)
        ),
    }
}

fn safe_decode_uri_component(value: &str) -> String {
    decode_uri_component(value).unwrap_or_else(|| value.to_string())
}

/// JS `encodeURIComponent`：保留 A-Z a-z 0-9 `- _ . ! ~ * ' ( )`，按 UTF-8 百分号编码。
pub fn encode_uri_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        let unreserved = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if unreserved {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// JS `decodeURIComponent`：非法转义返回 `None`（调用方回退原值）。
pub fn decode_uri_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = hex_value(bytes[index + 1])?;
            let low = hex_value(bytes[index + 2])?;
            out.push(high * 16 + low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `migrateLegacyModelProviderId`：只处理已发布旧 ID，普通未知 ID 原样保留。
pub fn migrate_legacy_model_provider_id(provider_id: &str) -> Option<String> {
    let mapped = match provider_id {
        "builtin:bigmodel" => Some("bigmodel-api"),
        "builtin:zai" => Some("zai-api"),
        "builtin:bigmodel-start-plan" => Some("account:bigmodel-start-plan"),
        "builtin:zai-start-plan" => Some("account:zai-start-plan"),
        "builtin:bigmodel-coding-plan" => Some("account:bigmodel-individual-coding-plan"),
        "builtin:zai-coding-plan" => Some("account:zai-individual-coding-plan"),
        _ => None,
    };
    match mapped {
        Some(value) => Some(value.to_string()),
        None => {
            if provider_id.starts_with("builtin:") {
                None
            } else {
                Some(provider_id.to_string())
            }
        }
    }
}

const OFFICIAL_GLM_MODEL_IDS: [&str; 22] = [
    "GLM-5.3",
    "GLM-5.3-Flash",
    "GLM-5V-Turbo",
    "GLM-5.2",
    "GLM-5.1",
    "GLM-5.1-Highspeed",
    "GLM-5",
    "GLM-5-Turbo",
    "GLM-4.7",
    "GLM-4.7-FlashX",
    "GLM-4.7-Flash",
    "GLM-4.6",
    "GLM-4.5-Air",
    "GLM-4.5",
    "GLM-4.6V",
    "GLM-4.6V-Flash",
    "GLM-4.6V-FlashX",
    "GLM-4.1V-Thinking-FlashX",
    "GLM-4.1V-Thinking-Flash",
    "GLM-4-FlashX-250414",
    "GLM-4-Flash-250414",
    "GLM-4V-Flash",
];

pub fn normalize_official_glm_model_id(model_id: &str) -> String {
    let lowered = model_id.to_lowercase();
    OFFICIAL_GLM_MODEL_IDS
        .iter()
        .find(|id| id.to_lowercase() == lowered)
        .map(|id| id.to_string())
        .unwrap_or_else(|| model_id.to_string())
}

pub fn migrate_official_glm_model_id(provider_id: &str, model_id: &str) -> String {
    if is_official_glm_provider(provider_id) {
        normalize_official_glm_model_id(model_id)
    } else {
        model_id.to_string()
    }
}

fn is_official_glm_provider(provider_id: &str) -> bool {
    matches!(
        provider_id,
        "builtin:zai"
            | "builtin:bigmodel"
            | "builtin:zai-start-plan"
            | "builtin:bigmodel-start-plan"
            | "builtin:zai-coding-plan"
            | "builtin:bigmodel-coding-plan"
            | "account:zai-start-plan"
            | "account:bigmodel-start-plan"
            | "account:zai-individual-coding-plan"
            | "account:bigmodel-individual-coding-plan"
            | "account:zai-team-coding-plan"
            | "account:bigmodel-team-coding-plan"
    )
}

/// `migrateModelValue`：只重写 frontmatter 里 `model` 单行的 Provider/型号。
pub fn migrate_model_value(value: &str) -> String {
    if let Some(decoded) = decode_custom_model_value(value) {
        let Some(model_name) = decoded.model_name.as_deref() else {
            return value.to_string();
        };
        if !decoded.provider_id.starts_with("builtin:") {
            return value.to_string();
        }
        let Some(provider_id) = migrate_legacy_model_provider_id(&decoded.provider_id) else {
            return value.to_string();
        };
        if provider_id == decoded.provider_id {
            return value.to_string();
        }
        let model_id = migrate_official_glm_model_id(&decoded.provider_id, model_name);
        let body = &value[CUSTOM_PREFIX.len()..];
        let separator = if let Some(rest) = body.strip_prefix("builtin:") {
            rest.find(':').map(|index| index + "builtin:".len())
        } else {
            body.find(':')
        };
        let Some(separator) = separator else {
            return value.to_string();
        };
        let encoded_model = if model_id == model_name {
            body[separator + 1..].to_string()
        } else {
            encode_uri_component(&model_id)
        };
        return format!(
            "{CUSTOM_PREFIX}{}:{}",
            encode_uri_component(&provider_id),
            encoded_model
        );
    }

    let Some(separator) = value.find('/') else {
        return value.to_string();
    };
    if separator < 1 {
        return value.to_string();
    }
    let old_provider = &value[..separator];
    if !old_provider.starts_with("builtin:") {
        return value.to_string();
    }
    let Some(provider_id) = migrate_legacy_model_provider_id(old_provider) else {
        return value.to_string();
    };
    let model_id = &value[separator + 1..];
    let (name, suffix) = match model_id.find('$') {
        Some(index) => (&model_id[..index], &model_id[index..]),
        None => (model_id, ""),
    };
    format!(
        "{provider_id}/{}{suffix}",
        migrate_official_glm_model_id(old_provider, name)
    )
}

/// `migrateSubagentMarkdownProvider`：只替换 frontmatter 中 `model:` 的取值。
pub fn migrate_subagent_markdown_provider(content: &str) -> String {
    let Some((frontmatter_start, body_start, body_end)) = frontmatter_bounds(content) else {
        return content.to_string();
    };
    let body = &content[body_start..body_end];
    let mut migrated = String::with_capacity(body.len());
    let mut changed = false;
    for line in body.split_inclusive('\n') {
        let (line_body, line_ending) = match line.strip_suffix("\r\n") {
            Some(body) => (body, "\r\n"),
            None => match line.strip_suffix('\n') {
                Some(body) => (body, "\n"),
                None => (line, ""),
            },
        };
        match rewrite_model_line(line_body) {
            Some(rewritten) => {
                changed = true;
                migrated.push_str(&rewritten);
                migrated.push_str(line_ending);
            }
            None => migrated.push_str(line),
        }
    }
    if !changed {
        return content.to_string();
    }
    let prefix = &content[..frontmatter_start];
    let suffix = &content[body_end..];
    format!("{prefix}{migrated}{suffix}")
}

/// 返回 `(替换起点, 内容起始, 内容结束)`；替换起点包含首行 `---\n`，
/// 内容不含闭合行前的换行。
fn frontmatter_bounds(content: &str) -> Option<(usize, usize, usize)> {
    let bom_len = if content.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let rest = &content[bom_len..];
    let first_line_end = rest.find('\n')?;
    let first_line = rest[..first_line_end].trim_end_matches('\r');
    if first_line.trim_end_matches([' ', '\t']) != "---" {
        return None;
    }
    let content_start = bom_len + first_line_end + 1;
    let mut cursor = content_start;
    while cursor <= content.len() {
        let line_end = content[cursor..]
            .find('\n')
            .map(|offset| cursor + offset)
            .unwrap_or(content.len());
        let line = content[cursor..line_end].trim_end_matches('\r');
        if line.trim_end_matches([' ', '\t']) == "---" {
            let mut body_end = if cursor > content_start && content.as_bytes()[cursor - 1] == b'\n'
            {
                cursor - 1
            } else {
                cursor
            };
            if body_end > content_start && content.as_bytes()[body_end - 1] == b'\r' {
                body_end -= 1;
            }
            return Some((content_start, content_start, body_end));
        }
        if line_end == content.len() {
            return None;
        }
        cursor = line_end + 1;
    }
    None
}

fn rewrite_model_line(line: &str) -> Option<String> {
    let (prefix, remainder) = split_model_prefix(line)?;
    let (token, trailing) = split_scalar_token(remainder)?;
    let value = decode_scalar_token(token)?;
    let next = migrate_model_value(&value);
    if next == value {
        return None;
    }
    let encoded = if token.starts_with('"') {
        serde_json::to_string(&next).ok()?
    } else if token.starts_with('\'') {
        format!("'{}'", next.replace('\'', "''"))
    } else {
        next
    };
    Some(format!("{prefix}{encoded}{trailing}"))
}

/// `model[ \t]*:[ \t]*` 前缀。
fn split_model_prefix(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("model")?;
    let after_spaces = rest.trim_start_matches([' ', '\t']);
    let value = after_spaces.strip_prefix(':')?;
    let after_colon = value.trim_start_matches([' ', '\t']);
    let prefix_len = line.len() - after_colon.len();
    Some((&line[..prefix_len], after_colon))
}

/// 按 TS 的标量正则拆出 `(token, trailing)`；`trailing` 含 token 之后的空白与注释。
fn split_scalar_token(remainder: &str) -> Option<(&str, &str)> {
    if remainder.is_empty() {
        return Some(("", ""));
    }
    let first = remainder.chars().next()?;
    if first == '"' {
        let mut escaped = false;
        for (index, character) in remainder.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if character == '\\' {
                escaped = true;
                continue;
            }
            if character == '"' {
                let end = index + character.len_utf8();
                return Some((&remainder[..end], &remainder[end..]));
            }
        }
        return None;
    }
    if first == '\'' {
        let bytes = remainder.as_bytes();
        let mut index = 1;
        while index < bytes.len() {
            if bytes[index] == b'\'' {
                if index + 1 < bytes.len() && bytes[index + 1] == b'\'' {
                    index += 2;
                    continue;
                }
                let end = index + 1;
                return Some((&remainder[..end], &remainder[end..]));
            }
            index += 1;
        }
        return None;
    }
    if let Some(comment) = find_inline_comment(remainder) {
        let token = remainder[..comment].trim_end_matches([' ', '\t']);
        return Some((token, &remainder[token.len()..]));
    }
    let token = remainder.trim_end_matches([' ', '\t']);
    Some((token, &remainder[token.len()..]))
}

fn find_inline_comment(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] == b'#' && (index == 0 || matches!(bytes[index - 1], b' ' | b'\t')) {
            return Some(index);
        }
    }
    None
}

fn decode_scalar_token(token: &str) -> Option<String> {
    if token.starts_with('"') {
        return serde_json::from_str::<String>(token).ok();
    }
    if token.starts_with('\'') && token.ends_with('\'') && token.len() >= 2 {
        return Some(token[1..token.len() - 1].replace("''", "'"));
    }
    Some(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_parses_reasoning_suffix() {
        assert_eq!(
            parse_model_picker_value("glm/GLM-4.7$high"),
            Some(Selection {
                provider_id: "glm".into(),
                model_id: "GLM-4.7".into(),
                options: Some(SelectionOptions {
                    reasoning_level: "high".into()
                }),
            })
        );
        assert_eq!(
            parse_model_picker_value("glm/x$"),
            Some(Selection {
                provider_id: "glm".into(),
                model_id: "x$".into(),
                options: None
            })
        );
        assert!(parse_model_picker_value("no-separator").is_none());
        assert!(parse_model_picker_value("/x").is_none());
    }

    #[test]
    fn custom_values_round_trip() {
        let encoded = encode_custom_model_value("custom:a/b", Some("m$1"));
        assert_eq!(encoded, "custom:custom%3Aa%2Fb:m%241");
        let decoded = decode_custom_model_value(&encoded).unwrap();
        assert_eq!(decoded.provider_id, "custom:a/b");
        assert_eq!(decoded.model_name.as_deref(), Some("m$1"));
        assert_eq!(
            decode_custom_model_value("custom:builtin:zai:GLM-4.7")
                .unwrap()
                .provider_id,
            "builtin:zai"
        );
        assert!(decode_custom_model_value("plain").is_none());
    }

    #[test]
    fn markdown_selection_inherit_and_validation() {
        let model = Value::String("inherit".into());
        assert!(parse_markdown_selection(Some(&model), None).is_none());
        let model = Value::String("glm/GLM-4.7".into());
        let level = Value::String(" high ".into());
        let selection = parse_markdown_selection(Some(&model), Some(&level)).unwrap();
        assert_eq!(selection.model_id, "GLM-4.7");
        assert_eq!(selection.options.unwrap().reasoning_level, "high");
        assert!(parse_markdown_selection(None, None).is_none());
    }

    #[test]
    fn legacy_provider_migration_matches_mapping() {
        assert_eq!(
            migrate_legacy_model_provider_id("builtin:zai").as_deref(),
            Some("zai-api")
        );
        assert_eq!(
            migrate_legacy_model_provider_id("builtin:zai-coding-plan").as_deref(),
            Some("account:zai-individual-coding-plan")
        );
        assert!(migrate_legacy_model_provider_id("builtin:unknown").is_none());
        assert_eq!(
            migrate_legacy_model_provider_id("custom:abc").as_deref(),
            Some("custom:abc")
        );
    }

    #[test]
    fn legacy_markdown_provider_rewrites_only_builtin_lines() {
        let content = "---\nname: x\ndescription: y\nmodel: builtin:zai/glm-4.7$high\n---\nbody\n";
        assert_eq!(
            migrate_subagent_markdown_provider(content),
            "---\nname: x\ndescription: y\nmodel: zai-api/GLM-4.7$high\n---\nbody\n"
        );
        let custom = "---\nmodel: \"custom:builtin%3Azai:glm-4.7\"\n---\n";
        assert_eq!(
            migrate_subagent_markdown_provider(custom),
            "---\nmodel: \"custom:zai-api:GLM-4.7\"\n---\n"
        );
        let untouched = "---\nmodel: custom:abc/def\n---\n";
        assert_eq!(migrate_subagent_markdown_provider(untouched), untouched);
    }
}
