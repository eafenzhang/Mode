//! 命令来源描述符与命名规则（对齐 `commandsService.ts` 顶部常量与 `getCommandName` /
//! `getCommandFileName`）。

use std::path::Path;

use crate::types::{CommandAgentSource, CommandConfig, SettingsDirectorySource};

pub const MODE_COMMAND_AGENT_SOURCE: CommandAgentSource = CommandAgentSource::ModeAgent;

/// 与 `MODE_COMMAND_AGENT_SOURCES` 一致（目前只有 `modeAgent`）。
pub fn command_agent_sources() -> &'static [CommandAgentSource] {
    &[MODE_COMMAND_AGENT_SOURCE]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandAgentSourceDescriptor {
    pub agent_source: CommandAgentSource,
    pub directory_source: SettingsDirectorySource,
    pub user_directory_segments: &'static [&'static str],
    pub workspace_directory_segments: &'static [&'static str],
    pub file_extension: &'static str,
    pub namespace_separator: &'static str,
    pub supports_argument_hint: bool,
}

pub const MODE_COMMAND_DESCRIPTOR: CommandAgentSourceDescriptor = CommandAgentSourceDescriptor {
    agent_source: CommandAgentSource::ModeAgent,
    directory_source: SettingsDirectorySource::Mode,
    user_directory_segments: &[".mode", "commands"],
    workspace_directory_segments: &[".mode", "commands"],
    file_extension: ".md",
    namespace_separator: "/",
    supports_argument_hint: true,
};

const AGENTS_COMMAND_DESCRIPTOR: CommandAgentSourceDescriptor = CommandAgentSourceDescriptor {
    agent_source: CommandAgentSource::ModeAgent,
    directory_source: SettingsDirectorySource::Agents,
    user_directory_segments: &[".agents", "commands"],
    workspace_directory_segments: &[".agents", "commands"],
    file_extension: ".md",
    namespace_separator: "/",
    supports_argument_hint: true,
};

/// `COMMAND_DIRECTORY_SOURCE_DESCRIPTORS`：`.mode` 在前（强优先级来源），`.agents` 兜底。
pub const COMMAND_DIRECTORY_SOURCE_DESCRIPTORS: &[CommandAgentSourceDescriptor] =
    &[MODE_COMMAND_DESCRIPTOR, AGENTS_COMMAND_DESCRIPTOR];

pub fn get_command_source_descriptor(
    _agent_source: CommandAgentSource,
) -> &'static CommandAgentSourceDescriptor {
    // `COMMAND_AGENT_SOURCE_DESCRIPTORS` 里只有 modeAgent 一项。
    &MODE_COMMAND_DESCRIPTOR
}

/// 去掉结尾的 `.md`（大小写不敏感）。只切 ASCII 后缀，不会切坏多字节字符。
fn strip_markdown_extension(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 3 {
        let tail = &bytes[bytes.len() - 3..];
        if tail[0] == b'.' && tail[1].eq_ignore_ascii_case(&b'm') && tail[2].eq_ignore_ascii_case(&b'd')
        {
            return &value[..value.len() - 3];
        }
    }
    value
}

/// `getCommandName`：`/` + 相对命令根的路径（去扩展名、分隔符统一为命名分隔符）。
pub fn command_name(root_dir: &Path, file_path: &Path, namespace_separator: &str) -> String {
    let relative = file_path.strip_prefix(root_dir).unwrap_or(file_path);
    let text = relative.to_string_lossy().replace('\\', "/");
    let without_extension = strip_markdown_extension(&text);
    let joined = without_extension
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(namespace_separator);
    format!("/{joined}")
}

/// `getCommandFileName`：去掉**一个**前导 `/`，把命名分隔符 `:` 还原成目录分隔 `/`，加扩展名。
pub fn command_file_name(config: &CommandConfig, descriptor: &CommandAgentSourceDescriptor) -> String {
    let raw_name = config.name.strip_prefix('/').unwrap_or(&config.name);
    let relative_name = if descriptor.namespace_separator == ":" {
        raw_name.replace(':', "/")
    } else {
        raw_name.to_string()
    };
    format!("{relative_name}{}", descriptor.file_extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(name: &str) -> CommandConfig {
        CommandConfig {
            name: name.to_string(),
            prompt: String::new(),
            description: None,
            argument_hint: None,
            file_path: None,
        }
    }

    #[test]
    fn command_names_use_forward_slashes_and_strip_extension() {
        let root = Path::new("C:/ws/.mode/commands");
        assert_eq!(
            command_name(root, &root.join("a.md"), "/"),
            "/a".to_string()
        );
        assert_eq!(
            command_name(root, &root.join("sub").join("b.MD"), "/"),
            "/sub/b".to_string()
        );
        // Windows 分隔符在输入路径里也要归一。
        assert_eq!(
            command_name(root, Path::new("C:\\ws\\.mode\\commands\\sub\\c.md"), "/"),
            "/sub/c".to_string()
        );
    }

    #[test]
    fn file_names_strip_exactly_one_leading_slash() {
        assert_eq!(command_file_name(&config("/a/b"), &MODE_COMMAND_DESCRIPTOR), "a/b.md");
        assert_eq!(command_file_name(&config("//a"), &MODE_COMMAND_DESCRIPTOR), "/a.md");
    }
}
