//! 外部 Agent 的来源目录表与 Mode 目标根。
//!
//! 真源：`settingsSyncService.ts` 的 `SUPPORTED_*_AGENT_SOURCES`、
//! `getExternal*Roots` 与各 `resolve*TargetRoot*`。

use std::path::{Path, PathBuf};

pub const MODE_DATA_ROOT_DIR_NAME: &str = ".mode";

#[derive(Debug, Clone, Copy)]
pub struct AgentSource {
    pub agent: &'static str,
    pub project: &'static [&'static str],
    pub global: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub struct McpAgentSource {
    pub agent: &'static str,
    pub project_files: &'static [&'static [&'static str]],
    pub global_files: &'static [&'static [&'static str]],
    pub format: McpConfigFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigFormat {
    McpServersJson,
    McpJson,
    CodexToml,
}

pub const SUPPORTED_SKILL_AGENT_SOURCES: &[AgentSource] = &[
    AgentSource {
        agent: "claudeCode",
        project: &[".claude", "skills"],
        global: &[".claude", "skills"],
    },
    AgentSource {
        agent: "codexCli",
        project: &[".codex", "skills"],
        global: &[".codex", "skills"],
    },
    AgentSource {
        agent: "openCode",
        project: &[".opencode", "skills"],
        global: &[".config", "opencode", "skills"],
    },
    AgentSource {
        agent: "openClaw",
        project: &["skills"],
        global: &[".openclaw", "skills"],
    },
    AgentSource {
        agent: "augment",
        project: &[".augment", "skills"],
        global: &[".augment", "skills"],
    },
    AgentSource {
        agent: "continue",
        project: &[".continue", "skills"],
        global: &[".continue", "skills"],
    },
    AgentSource {
        agent: "goose",
        project: &[".goose", "skills"],
        global: &[".config", "goose", "skills"],
    },
    AgentSource {
        agent: "qwenCode",
        project: &[".qwen", "skills"],
        global: &[".qwen", "skills"],
    },
    AgentSource {
        agent: "qode",
        project: &[".qoder", "skills"],
        global: &[".qoder", "skills"],
    },
    AgentSource {
        agent: "qodeCn",
        project: &[".qoder", "skills"],
        global: &[".qoder-cn", "skills"],
    },
    AgentSource {
        agent: "windsurf",
        project: &[".windsurf", "skills"],
        global: &[".codeium", "windsurf", "skills"],
    },
    AgentSource {
        agent: "trae",
        project: &[".trae", "skills"],
        global: &[".trae", "skills"],
    },
    AgentSource {
        agent: "traeCn",
        project: &[".trae", "skills"],
        global: &[".trae-cn", "skills"],
    },
    AgentSource {
        agent: "kiroCli",
        project: &[".kiro", "skills"],
        global: &[".kiro", "skills"],
    },
    AgentSource {
        agent: "roo",
        project: &[".roo", "skills"],
        global: &[".roo", "skills"],
    },
    AgentSource {
        agent: "codeBuddy",
        project: &[".codebuddy", "skills"],
        global: &[".codebuddy", "skills"],
    },
];

pub const SUPPORTED_COMMAND_AGENT_SOURCES: &[AgentSource] = &[
    AgentSource {
        agent: "claudeCode",
        project: &[".claude", "commands"],
        global: &[".claude", "commands"],
    },
    AgentSource {
        agent: "codexCli",
        project: &[".codex", "commands"],
        global: &[".codex", "commands"],
    },
    AgentSource {
        agent: "openCode",
        project: &[".opencode", "commands"],
        global: &[".config", "opencode", "commands"],
    },
    AgentSource {
        agent: "openClaw",
        project: &["commands"],
        global: &[".openclaw", "commands"],
    },
    AgentSource {
        agent: "augment",
        project: &[".augment", "commands"],
        global: &[".augment", "commands"],
    },
    AgentSource {
        agent: "continue",
        project: &[".continue", "commands"],
        global: &[".continue", "commands"],
    },
    AgentSource {
        agent: "goose",
        project: &[".goose", "commands"],
        global: &[".config", "goose", "commands"],
    },
    AgentSource {
        agent: "qwenCode",
        project: &[".qwen", "commands"],
        global: &[".qwen", "commands"],
    },
    AgentSource {
        agent: "qode",
        project: &[".qoder", "commands"],
        global: &[".qoder", "commands"],
    },
    AgentSource {
        agent: "qodeCn",
        project: &[".qoder", "commands"],
        global: &[".qoder-cn", "commands"],
    },
    AgentSource {
        agent: "windsurf",
        project: &[".windsurf", "commands"],
        global: &[".codeium", "windsurf", "commands"],
    },
    AgentSource {
        agent: "trae",
        project: &[".trae", "commands"],
        global: &[".trae", "commands"],
    },
    AgentSource {
        agent: "kiroCli",
        project: &[".kiro", "commands"],
        global: &[".kiro", "commands"],
    },
    AgentSource {
        agent: "roo",
        project: &[".roo", "commands"],
        global: &[".roo", "commands"],
    },
    AgentSource {
        agent: "codeBuddy",
        project: &[".codebuddy", "commands"],
        global: &[".codebuddy", "commands"],
    },
];

pub const SUPPORTED_PLUGIN_AGENT_SOURCES: &[AgentSource] = &[
    AgentSource {
        agent: "claudeCode",
        project: &[".claude", "plugins"],
        global: &[".claude", "plugins"],
    },
    AgentSource {
        agent: "codexCli",
        project: &[".codex", "plugins"],
        global: &[".codex", "plugins"],
    },
    AgentSource {
        agent: "openCode",
        project: &[".opencode", "plugins"],
        global: &[".config", "opencode", "plugins"],
    },
    AgentSource {
        agent: "openClaw",
        project: &["plugins"],
        global: &[".openclaw", "plugins"],
    },
    AgentSource {
        agent: "augment",
        project: &[".augment", "plugins"],
        global: &[".augment", "plugins"],
    },
    AgentSource {
        agent: "continue",
        project: &[".continue", "plugins"],
        global: &[".continue", "plugins"],
    },
    AgentSource {
        agent: "goose",
        project: &[".goose", "plugins"],
        global: &[".config", "goose", "plugins"],
    },
    AgentSource {
        agent: "qwenCode",
        project: &[".qwen", "plugins"],
        global: &[".qwen", "plugins"],
    },
    AgentSource {
        agent: "qode",
        project: &[".qoder", "plugins"],
        global: &[".qoder", "plugins"],
    },
    AgentSource {
        agent: "qodeCn",
        project: &[".qoder", "plugins"],
        global: &[".qoder-cn", "plugins"],
    },
    AgentSource {
        agent: "windsurf",
        project: &[".windsurf", "plugins"],
        global: &[".codeium", "windsurf", "plugins"],
    },
    AgentSource {
        agent: "trae",
        project: &[".trae", "plugins"],
        global: &[".trae", "plugins"],
    },
    AgentSource {
        agent: "kiroCli",
        project: &[".kiro", "plugins"],
        global: &[".kiro", "plugins"],
    },
    AgentSource {
        agent: "roo",
        project: &[".roo", "plugins"],
        global: &[".roo", "plugins"],
    },
    AgentSource {
        agent: "codeBuddy",
        project: &[".codebuddy", "plugins"],
        global: &[".codebuddy", "plugins"],
    },
];

pub const SUPPORTED_MCP_AGENT_SOURCES: &[McpAgentSource] = &[
    McpAgentSource {
        agent: "claudeCode",
        project_files: &[&[".claude", "settings.json"], &[".mcp.json"]],
        global_files: &[&[".claude", "settings.json"]],
        format: McpConfigFormat::McpServersJson,
    },
    McpAgentSource {
        agent: "codexCli",
        project_files: &[&[".codex", "config.toml"]],
        global_files: &[&[".codex", "config.toml"]],
        format: McpConfigFormat::CodexToml,
    },
    McpAgentSource {
        agent: "openCode",
        project_files: &[&[".opencode", "opencode.json"]],
        global_files: &[&[".config", "opencode", "opencode.json"]],
        format: McpConfigFormat::McpJson,
    },
    McpAgentSource {
        agent: "openClaw",
        project_files: &[&["settings.json"]],
        global_files: &[&[".openclaw", "settings.json"]],
        format: McpConfigFormat::McpServersJson,
    },
    McpAgentSource {
        agent: "qwenCode",
        project_files: &[&[".qwen", "settings.json"]],
        global_files: &[&[".qwen", "settings.json"]],
        format: McpConfigFormat::McpServersJson,
    },
    McpAgentSource {
        agent: "qode",
        project_files: &[&[".qoder", "settings.json"]],
        global_files: &[&[".qoder", "settings.json"]],
        format: McpConfigFormat::McpServersJson,
    },
    McpAgentSource {
        agent: "qodeCn",
        project_files: &[&[".qoder", "settings.json"]],
        global_files: &[&[".qoder-cn", "settings.json"]],
        format: McpConfigFormat::McpServersJson,
    },
    McpAgentSource {
        agent: "trae",
        project_files: &[&[".trae", "settings.json"]],
        global_files: &[&[".trae", "settings.json"]],
        format: McpConfigFormat::McpServersJson,
    },
    McpAgentSource {
        agent: "kiroCli",
        project_files: &[&[".kiro", "settings.json"]],
        global_files: &[&[".kiro", "settings.json"]],
        format: McpConfigFormat::McpServersJson,
    },
    McpAgentSource {
        agent: "roo",
        project_files: &[&[".roo", "settings.json"]],
        global_files: &[&[".roo", "settings.json"]],
        format: McpConfigFormat::McpServersJson,
    },
    McpAgentSource {
        agent: "codeBuddy",
        project_files: &[&[".codebuddy", "settings.json"]],
        global_files: &[&[".codebuddy", "settings.json"]],
        format: McpConfigFormat::McpServersJson,
    },
    McpAgentSource {
        agent: "agents",
        project_files: &[&[".agents", "mcp.json"]],
        global_files: &[&[".agents", "mcp.json"]],
        format: McpConfigFormat::McpServersJson,
    },
];

#[derive(Debug, Clone)]
pub struct SourceRoot {
    pub agent: &'static str,
    pub root_path: PathBuf,
    pub scope: &'static str,
}

#[derive(Debug, Clone)]
pub struct McpSourceRoot {
    pub agent: &'static str,
    pub file_path: PathBuf,
    pub scope: &'static str,
    pub format: McpConfigFormat,
}

fn join_all(base: &Path, segments: &[&str]) -> PathBuf {
    let mut path = base.to_path_buf();
    for segment in segments {
        path = path.join(segment);
    }
    path
}

pub fn external_roots(
    table: &[AgentSource],
    home: &Path,
    workspace_path: Option<&Path>,
) -> Vec<SourceRoot> {
    let mut roots: Vec<SourceRoot> = table
        .iter()
        .map(|source| SourceRoot {
            agent: source.agent,
            root_path: join_all(home, source.global),
            scope: "user",
        })
        .collect();
    if let Some(workspace_path) = workspace_path {
        roots.extend(table.iter().map(|source| SourceRoot {
            agent: source.agent,
            root_path: join_all(workspace_path, source.project),
            scope: "workspace",
        }));
    }
    roots
}

pub fn external_mcp_roots(home: &Path, workspace_path: Option<&Path>) -> Vec<McpSourceRoot> {
    let mut roots: Vec<McpSourceRoot> = SUPPORTED_MCP_AGENT_SOURCES
        .iter()
        .flat_map(|source| {
            source.global_files.iter().map(move |file| McpSourceRoot {
                agent: source.agent,
                file_path: join_all(home, file),
                scope: "user",
                format: source.format,
            })
        })
        .collect();
    if let Some(workspace_path) = workspace_path {
        roots.extend(SUPPORTED_MCP_AGENT_SOURCES.iter().flat_map(|source| {
            source.project_files.iter().map(move |file| McpSourceRoot {
                agent: source.agent,
                file_path: join_all(workspace_path, file),
                scope: "workspace",
                format: source.format,
            })
        }));
    }
    roots
}

pub fn user_mode_root(home: &Path, leaf: &str) -> PathBuf {
    home.join(MODE_DATA_ROOT_DIR_NAME).join(leaf)
}

pub fn workspace_mode_root(workspace_path: &Path, leaf: &str) -> PathBuf {
    workspace_path.join(MODE_DATA_ROOT_DIR_NAME).join(leaf)
}

pub fn user_cli_config_path(home: &Path) -> PathBuf {
    home.join(MODE_DATA_ROOT_DIR_NAME)
        .join("cli")
        .join("config.json")
}

pub fn workspace_mode_config_path(workspace_path: &Path) -> PathBuf {
    workspace_path
        .join(MODE_DATA_ROOT_DIR_NAME)
        .join("config.json")
}

pub fn claude_user_agents_file(home: &Path) -> PathBuf {
    home.join(".claude").join("CLAUDE.md")
}

/// `path.resolve(x)`：相对路径按当前工作目录展开并做词法归一。
pub fn absolute_path(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    plugin_discovery::js::normalize_lexical(&joined)
}

pub fn user_mode_agents_file(home: &Path) -> PathBuf {
    home.join(MODE_DATA_ROOT_DIR_NAME).join("AGENTS.md")
}

/// `resolveTargetRootForScope`：global → 用户根；project → 工作区根（无 workspace 时为 None）。
pub fn target_root_for_scope(
    home: &Path,
    workspace_path: Option<&Path>,
    target_scope: &str,
    leaf: &str,
) -> Option<PathBuf> {
    if target_scope == "global" {
        return Some(user_mode_root(home, leaf));
    }
    workspace_path.map(|path| workspace_mode_root(path, leaf))
}

/// `resolveCandidateTargetRoot`：候选自带的 user/workspace scope。
pub fn candidate_target_root(
    home: &Path,
    workspace_path: Option<&Path>,
    source_scope: &str,
    leaf: &str,
) -> Option<PathBuf> {
    if source_scope == "workspace" {
        workspace_path.map(|path| workspace_mode_root(path, leaf))
    } else {
        Some(user_mode_root(home, leaf))
    }
}

pub fn config_path_for_scope(
    home: &Path,
    workspace_path: Option<&Path>,
    target_scope: &str,
) -> Option<PathBuf> {
    if target_scope == "global" {
        return Some(user_cli_config_path(home));
    }
    workspace_path.map(workspace_mode_config_path)
}
