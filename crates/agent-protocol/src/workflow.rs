//! 已保存工作流只读面：`workflows/list`、`workflows/get`、`workflows/runs` 的严格镜像。
//!
//! 真源：`packages/shared/src/mode-protocol/index.ts` 的工作流段（与
//! `@mode/contracts` 的 saved-workflow 形状逐字对齐）。规范见
//! `docs/specs/session-write-group.md` §「工作流只读面」。

use std::collections::BTreeMap;
use std::fmt;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Number, Value};

use crate::session_read::SessionWorkspaceRef;

pub const WORKFLOWS_LIST_METHOD: &str = "workflows/list";
pub const WORKFLOWS_GET_METHOD: &str = "workflows/get";
pub const WORKFLOWS_RUNS_METHOD: &str = "workflows/runs";
pub const WORKFLOWS_RUNS_MAX_LIMIT: u64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SavedWorkflowScope {
    #[serde(rename = "project")]
    Project,
    #[serde(rename = "global")]
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SavedWorkflowArgType {
    #[serde(rename = "string")]
    String,
    #[serde(rename = "number")]
    Number,
    #[serde(rename = "boolean")]
    Boolean,
    #[serde(rename = "json")]
    Json,
}

/// `modeSavedWorkflowArgDeclarationSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedWorkflowArgDeclaration {
    #[serde(rename = "type")]
    pub arg_type: SavedWorkflowArgType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// `z.unknown().optional()`：显式 null 与省略分开保留。
    #[serde(
        default,
        deserialize_with = "crate::session_read::deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub default: Option<Option<Value>>,
}

pub type SavedWorkflowArgs = BTreeMap<String, SavedWorkflowArgDeclaration>;

/// `modeSavedWorkflowMetaSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedWorkflowMeta {
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<SavedWorkflowArgs>,
}

/// `modeSavedWorkflowEntrySchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedWorkflowEntry {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<SavedWorkflowArgs>,
    pub scope: SavedWorkflowScope,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedWorkflowInvalidEntry {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SavedWorkflowFailureReason {
    #[serde(rename = "invalid_name")]
    InvalidName,
    #[serde(rename = "not_found")]
    NotFound,
    #[serde(rename = "parse_error")]
    ParseError,
    #[serde(rename = "read_error")]
    ReadError,
}

/// `modeWorkflowsListParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowsListParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<SavedWorkflowScope>,
}

/// `modeWorkflowsListResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowsListResult {
    pub workflows: Vec<SavedWorkflowEntry>,
    pub invalid: Vec<SavedWorkflowInvalidEntry>,
    pub dir: String,
}

/// `modeWorkflowsGetParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowsGetParams {
    pub workspace: SessionWorkspaceRef,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<SavedWorkflowScope>,
}

/// `z.literal(true)` / `z.literal(false)` 的 serde 表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiteralTrue;

impl Serialize for LiteralTrue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for LiteralTrue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Ok(LiteralTrue)
        } else {
            Err(de::Error::custom("expected true"))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiteralFalse;

impl Serialize for LiteralFalse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for LiteralFalse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Err(de::Error::custom("expected false"))
        } else {
            Ok(LiteralFalse)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowGetOk {
    pub ok: LiteralTrue,
    pub name: String,
    pub path: String,
    pub scope: SavedWorkflowScope,
    pub meta: SavedWorkflowMeta,
    /// 脚本本体（frontmatter 之后逐字节）。
    pub script: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedWorkflowFailure {
    pub ok: LiteralFalse,
    pub reason: SavedWorkflowFailureReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `modeWorkflowsGetResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkflowsGetResult {
    Ok(WorkflowGetOk),
    Failure(SavedWorkflowFailure),
}

/// `modeWorkflowsRunsParamsSchema`；`limit` 必须落在 1..=50。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowsRunsParams {
    pub workspace: SessionWorkspaceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub limit: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<SavedWorkflowScope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WorkflowRunStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "errored")]
    Errored,
    #[serde(rename = "stopped")]
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WorkflowRunStopReason {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "model")]
    Model,
    #[serde(rename = "provider")]
    Provider,
    #[serde(rename = "interrupted")]
    Interrupted,
    #[serde(rename = "superseded")]
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowArtifactKind {
    #[serde(rename = "file")]
    File,
    #[serde(rename = "markdown")]
    Markdown,
    #[serde(rename = "chart")]
    Chart,
    #[serde(rename = "table")]
    Table,
    #[serde(rename = "metrics")]
    Metrics,
    #[serde(rename = "board")]
    Board,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunArtifact {
    pub id: String,
    pub kind: WorkflowArtifactKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `z.number()`：允许小数，保留原样。
    pub version: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

/// `modeSavedWorkflowRunSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedWorkflowRun {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub status: WorkflowRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<WorkflowRunStopReason>,
    pub created_at: Number,
    pub updated_at: Number,
    pub spent_tokens: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<WorkflowRunArtifact>>,
}

/// `modeWorkflowsRunsResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowsRunsResult {
    pub runs: Vec<SavedWorkflowRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<LiteralTrue>,
}

impl fmt::Display for SavedWorkflowFailureReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidName => "invalid_name",
            Self::NotFound => "not_found",
            Self::ParseError => "parse_error",
            Self::ReadError => "read_error",
        })
    }
}

pub const WORKFLOWS_UPDATE_META_METHOD: &str = "workflows/updateMeta";
pub const WORKFLOWS_DELETE_METHOD: &str = "workflows/delete";
pub const WORKFLOWS_MOVE_METHOD: &str = "workflows/move";

/// `modeWorkflowsUpdateMetaParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowsUpdateMetaParams {
    pub workspace: SessionWorkspaceRef,
    pub name: String,
    pub meta: SavedWorkflowMeta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<SavedWorkflowScope>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowOkPath {
    pub ok: LiteralTrue,
    pub path: String,
}

/// `modeWorkflowsUpdateMetaResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkflowsUpdateMetaResult {
    Ok(WorkflowOkPath),
    Failure(SavedWorkflowFailure),
}

/// `modeWorkflowsDeleteParamsSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowsDeleteParams {
    pub workspace: SessionWorkspaceRef,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<SavedWorkflowScope>,
}

/// `modeWorkflowsDeleteResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkflowsDeleteResult {
    Ok(WorkflowOkPath),
    Failure(SavedWorkflowFailure),
}

/// `modeWorkflowsMoveParamsSchema`（`workspace` 既是载体也是目标项目）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowsMoveParams {
    pub workspace: SessionWorkspaceRef,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowsMoveFailureReason {
    #[serde(rename = "invalid_name")]
    InvalidName,
    #[serde(rename = "not_found")]
    NotFound,
    #[serde(rename = "target_exists")]
    TargetExists,
    #[serde(rename = "read_error")]
    ReadError,
    #[serde(rename = "write_error")]
    WriteError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowsMoveFailure {
    pub ok: LiteralFalse,
    pub reason: WorkflowsMoveFailureReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowsMoveOk {
    pub ok: LiteralTrue,
    pub from: String,
    pub to: String,
}

/// `modeWorkflowsMoveResultSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkflowsMoveResult {
    Ok(WorkflowsMoveOk),
    Failure(WorkflowsMoveFailure),
}
