//! workspace 只读面：`workspace/readPresentation` 与 `process/childProcesses` 的严格镜像。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的 `modeWorkspacePresentationSchema` /
//! `modeProcessChildProcessesResultSchema`。规范见 `docs/specs/session-write-group.md`
//! §「workspace 只读面」。

use serde::{Deserialize, Serialize};

use crate::session_read::{SessionMode, SessionWorkspaceRef};
use crate::snapshot::SlashCommand;

pub const WORKSPACE_READ_PRESENTATION_METHOD: &str = "workspace/readPresentation";
pub const PROCESS_CHILD_PROCESSES_METHOD: &str = "process/childProcesses";

/// `workspace/readPresentation` 的 params。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePresentationParams {
    pub workspace: SessionWorkspaceRef,
}

/// `modeWorkspacePresentationSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePresentation {
    pub workspace: SessionWorkspaceRef,
    pub mode: SessionMode,
    pub slash_commands: Vec<SlashCommand>,
}

/// `process/childProcesses` 的 params（空对象）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildProcessesParams {}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum McpSource {
    #[serde(rename = "builtin")]
    Builtin,
    #[serde(rename = "plugin")]
    Plugin,
    #[serde(rename = "custom")]
    Custom,
}

/// `modeProcessChildProcessSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChildProcess {
    pub pid: u64,
    pub server_name: String,
    pub mcp_source: McpSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
}

/// `modeProcessChildProcessesResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChildProcessesResult {
    pub processes: Vec<ChildProcess>,
}
