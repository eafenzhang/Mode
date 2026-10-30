//! v4 `workflowRuns`：dwf 引擎 run 的实时运行态。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/workflow-runs.ts` 与
//! `workflow-artifacts.ts` 的 `workflowRunArtifactSummarySchema`。规范见
//! `docs/specs/session-write-group.md` §「v4 工作流运行态」。

use serde::{Deserialize, Serialize};
use crate::workflow::{LiteralTrue, WorkflowArtifactKind};

pub const WORKFLOW_RUNS_MAX_RUNS: usize = 8;
pub const WORKFLOW_RUNS_MAX_ACTORS: usize = 1024;
pub const WORKFLOW_RUNS_MAX_NODES: usize = 1024;
pub const WORKFLOW_RUNS_MAX_REPORTS: usize = 64;
pub const WORKFLOW_RUNS_MAX_PENDING_QUESTIONS: usize = 32;
pub const WORKFLOW_RUNS_MAX_ARTIFACTS: usize = 32;
pub const WORKFLOW_RUNS_MAX_PHASES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunPhase {
    pub name: String,
    pub rounds: u64,
}

/// `workflowRunUnlistedPhaseSchema`（零值可选子键缺席）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunUnlistedPhase {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_name: Option<String>,
    pub actors: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actors_settled: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actors_failed: Option<u64>,
    pub settled: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunUsage {
    pub spent_tokens: u64,
    pub nodes_used: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes_unlisted: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes_unlisted_settled: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowRunActorStatus {
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunActor {
    pub site_id: String,
    pub ordinal: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub status: WorkflowRunActorStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunNodeLastTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowRunNodeKind {
    #[serde(rename = "ask")]
    Ask,
    #[serde(rename = "world-read")]
    WorldRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowRunNodePhase {
    #[serde(rename = "queued")]
    Queued,
    #[serde(rename = "dispatched")]
    Dispatched,
    #[serde(rename = "executing")]
    Executing,
    #[serde(rename = "waiting")]
    Waiting,
    #[serde(rename = "repairing")]
    Repairing,
    #[serde(rename = "nudged")]
    Nudged,
    #[serde(rename = "settled")]
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowRunNodeOutcome {
    #[serde(rename = "ok")]
    Ok,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunNode {
    pub site_id: String,
    pub ordinal: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<WorkflowRunNodeKind>,
    pub phase: WorkflowRunNodePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<WorkflowRunNodeOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_site_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_ordinal: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<WorkflowRunNodeLastTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunConcurrency {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub cap: u64,
    pub ceiling: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunReport {
    pub site_id: String,
    pub ordinal: u64,
    pub preview: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunPendingQuestion {
    pub qid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_site_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_ordinal: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_name: Option<String>,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<u64>,
}

/// `workflowRunArtifactSummarySchema`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunArtifactSummary {
    pub id: String,
    pub kind: WorkflowArtifactKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<LiteralTrue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// `workflowRunSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunState {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub status: WorkflowRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<WorkflowRunStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    pub usage: WorkflowRunUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumable: Option<LiteralTrue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<String>,
    pub actors: Vec<WorkflowRunActor>,
    pub nodes: Vec<WorkflowRunNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reports: Option<Vec<WorkflowRunReport>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_questions: Option<Vec<WorkflowRunPendingQuestion>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<WorkflowRunConcurrency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency_ceiling: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<WorkflowRunArtifactSummary>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phases: Option<Vec<WorkflowRunPhase>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_alongside: Option<Vec<Vec<u64>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlisted_by_phase: Option<Vec<WorkflowRunUnlistedPhase>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    pub last_event_sequence: u64,
}

/// `workflowRunsStateSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunsState {
    pub revision: u64,
    pub runs: Vec<WorkflowRunState>,
}

