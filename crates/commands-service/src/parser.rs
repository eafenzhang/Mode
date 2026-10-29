//! markdown 命令文件的手写 frontmatter 解析/生成（逐行对齐
//! `packages/services/src/commands/commandFileParser.ts`）。
//!
//! 这里刻意不用通用 YAML：TS 的实现是「找两行 `---` + 逐行认领键」的极简解析，
//! 换成真 YAML 会在缩进、引号、多文档等输入上产生不同结果。

use crate::types::CommandConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommandFile {
    pub name: String,
    pub prompt: String,
    pub content: String,
    pub file_path: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
}

/// 生成时会被替换（而不是保留）的 frontmatter 键。
pub const REPLACED_FRONTMATTER_KEYS: [&str; 2] = ["description", "argument-hint"];

fn get_cross_platform_basename(file_path: &str) -> &str {
    file_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(file_path)
}

/// 返回 (frontmatter 行, 正文行)；两者都借用原串，避免逐行分配。
fn split_markdown_command_content(content: &str) -> (Vec<&str>, Vec<&str>) {
    let lines: Vec<&str> = content.split('\n').collect();
    let Some(start) = lines.iter().position(|line| line.trim() == "---") else {
        return (Vec::new(), lines);
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(index, _)| index);
    let Some(end) = end else {
        return (Vec::new(), lines);
    };
    (lines[start + 1..end].to_vec(), lines[end + 1..].to_vec())
}

/// `readFrontmatterKey`：缩进行不是键；对 trim 后的行匹配 `^([A-Za-z0-9_-]+)\s*:`，小写化。
fn read_frontmatter_key(line: &str) -> Option<String> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let trimmed = line.trim();
    let bytes = trimmed.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' {
            index += 1;
        } else {
            break;
        }
    }
    if index == 0 {
        return None;
    }
    let key = &trimmed[..index];
    let rest = trimmed[index..].trim_start();
    if !rest.starts_with(':') {
        return None;
    }
    Some(key.to_ascii_lowercase())
}

fn read_frontmatter_multiline_value(lines: &[&str], target_key: &str) -> Option<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut collecting = false;

    for line in lines {
        if let Some(key) = read_frontmatter_key(line) {
            if collecting {
                break;
            }
            if key == target_key {
                collecting = true;
                // `line.split(":").slice(1).join(":")`：只按冒号切，保留值里的冒号。
                let value = line
                    .split(':')
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(":")
                    .trim()
                    .to_string();
                if !value.is_empty() {
                    collected.push(value);
                }
            }
            continue;
        }

        if collecting && (line.starts_with(' ') || line.starts_with('\t')) {
            collected.push(line.trim().to_string());
        }
    }

    let value = collected.join(" ").trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn preserve_frontmatter_lines(
    existing_content: Option<&str>,
    replaced_keys: &[&str],
) -> Vec<String> {
    let mut preserved: Vec<String> = Vec::new();
    let mut skip_until_next_key = false;

    for line in split_markdown_command_content(existing_content.unwrap_or("")).0 {
        if let Some(key) = read_frontmatter_key(line) {
            skip_until_next_key = replaced_keys.contains(&key.as_str());
        }
        if !skip_until_next_key {
            preserved.push(line.to_string());
        }
    }

    preserved
}

/// `parseCommandFile`：TS 的 `null` 分支只会在文件名为空时触发，而 `name` 至少是 `/`，
/// 因此这里直接返回解析结果。
pub fn parse_command_file(content: &str, file_path: &str) -> ParsedCommandFile {
    let (frontmatter_lines, content_lines) = split_markdown_command_content(content);
    let file_name = get_cross_platform_basename(file_path);
    let name = format!("/{}", strip_md_suffix(file_name));
    let content_text = content_lines.join("\n");
    let prompt = content_text.trim().to_string();

    ParsedCommandFile {
        name,
        prompt,
        content: content_text,
        file_path: file_path.to_string(),
        description: read_frontmatter_multiline_value(&frontmatter_lines, "description"),
        argument_hint: read_frontmatter_multiline_value(&frontmatter_lines, "argument-hint"),
    }
}

pub(crate) fn strip_md_suffix(file_name: &str) -> &str {
    let bytes = file_name.as_bytes();
    if bytes.len() >= 3 {
        let tail = &bytes[bytes.len() - 3..];
        if tail[0] == b'.' && tail[1].eq_ignore_ascii_case(&b'm') && tail[2].eq_ignore_ascii_case(&b'd')
        {
            return &file_name[..file_name.len() - 3];
        }
    }
    file_name
}

/// `generateCommandFileContent`：保留未知 frontmatter 键，覆写 description / argument-hint。
pub fn generate_command_file_content(
    config: &CommandConfig,
    existing_content: Option<&str>,
) -> String {
    let mut frontmatter_lines =
        preserve_frontmatter_lines(existing_content, &REPLACED_FRONTMATTER_KEYS);

    if let Some(description) = config.description.as_deref() {
        let trimmed = description.trim();
        if !trimmed.is_empty() {
            frontmatter_lines.push(format!("description: {trimmed}"));
        }
    }
    if let Some(argument_hint) = config.argument_hint.as_deref() {
        let trimmed = argument_hint.trim();
        if !trimmed.is_empty() {
            frontmatter_lines.push(format!("argument-hint: {trimmed}"));
        }
    }

    if frontmatter_lines.is_empty() {
        return config.prompt.clone();
    }

    format!(
        "---\n{}\n---\n\n{}",
        frontmatter_lines.join("\n"),
        config.prompt
    )
}

/// `getWritableCommandConfig`：描述符不支持 argument hint 时丢弃该字段。
pub fn writable_command_config(
    config: &CommandConfig,
    supports_argument_hint: bool,
) -> CommandConfig {
    if supports_argument_hint {
        config.clone()
    } else {
        CommandConfig {
            argument_hint: None,
            ..config.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_description_and_multiline_argument_hint() {
        let content = "---\ndescription: hello: world\nargument-hint: |\n  first\n  second\nother: x\n---\n\nbody\nline2\n";
        let parsed = parse_command_file(content, "C:\\ws\\.mode\\commands\\demo.md");
        assert_eq!(parsed.name, "/demo");
        assert_eq!(parsed.description.as_deref(), Some("hello: world"));
        assert_eq!(parsed.argument_hint.as_deref(), Some("| first second"));
        assert_eq!(parsed.prompt, "body\nline2");
        // 正文保留闭合 `---` 之后的空行（TS 的 `contentLines.join("\n")` 不 trim）。
        assert_eq!(parsed.content, "\nbody\nline2\n");
    }

    #[test]
    fn content_before_first_fence_is_dropped() {
        let content = "intro\n---\ndescription: d\n---\nbody";
        let parsed = parse_command_file(content, "/x/a.md");
        assert_eq!(parsed.description.as_deref(), Some("d"));
        assert_eq!(parsed.prompt, "body");
    }

    #[test]
    fn no_frontmatter_keeps_all_lines() {
        let parsed = parse_command_file("  keep me  \n", "/x/a.md");
        assert_eq!(parsed.content, "  keep me  \n");
        assert_eq!(parsed.prompt, "keep me");
        assert_eq!(parsed.description, None);
    }

    #[test]
    fn generate_without_frontmatter_returns_prompt_verbatim() {
        let config = CommandConfig {
            name: "/a".into(),
            prompt: "  raw prompt\n".into(),
            description: None,
            argument_hint: None,
            file_path: None,
        };
        assert_eq!(generate_command_file_content(&config, None), "  raw prompt\n");
    }

    #[test]
    fn generate_preserves_unknown_keys_and_drops_replaced_ones() {
        let existing = "---\nallowed-tools: Bash\n  continuation\ndescription: old\nargument-hint: old\n---\nold body";
        let config = CommandConfig {
            name: "/a".into(),
            prompt: "new body".into(),
            description: Some("  new  ".into()),
            argument_hint: Some(String::new()),
            file_path: None,
        };
        assert_eq!(
            generate_command_file_content(&config, Some(existing)),
            "---\nallowed-tools: Bash\n  continuation\ndescription: new\n---\n\nnew body"
        );
    }
}
