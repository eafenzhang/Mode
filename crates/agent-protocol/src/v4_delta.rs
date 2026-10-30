//! v4 `conversationDelta`：七个封闭 op。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/delta.ts`。规范见
//! `docs/specs/session-write-group.md` §「v4 会话增量」。

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::session_read::deserialize_nullable;
use crate::v4_rows::ConversationRow;
use crate::v4_snapshot::{
    BackgroundWorkSummary, CommandStateSummary, GoalState, InputRouting, PendingInteraction,
    PlanState, QueueState, SessionActionAvailability, SessionConfigState, SessionControl,
    SessionMetaState, SessionModelTransition, SessionUsageState, SharedContextImportState,
    SubagentProjectionState, WorkspaceHookAdmissionState,
};
use crate::v4_workflow_runs::{
    WorkflowRunActor, WorkflowRunArtifactSummary, WorkflowRunConcurrency, WorkflowRunNode,
    WorkflowRunPhase, WorkflowRunPendingQuestion, WorkflowRunReport, WorkflowRunStatus,
    WorkflowRunStopReason, WorkflowRunUnlistedPhase, WorkflowRunUsage, WorkflowRunsState,
};

/// `statePatchSchema`：键级整体替换（`Object.assign`），键集合封闭。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<SessionControl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_context_import: Option<SharedContextImportState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<SessionActionAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_routing: Option<InputRouting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<SessionMetaState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<SessionConfigState>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub model_transition: Option<Option<SessionModelTransition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SessionUsageState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<QueueState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_interactions: Option<Vec<PendingInteraction>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_commands: Option<Vec<CommandStateSummary>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_works: Option<Vec<BackgroundWorkSummary>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<SubagentProjectionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_runs: Option<WorkflowRunsState>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub goal: Option<Option<GoalState>>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub plan: Option<Option<PlanState>>,
    #[serde(
        default,
        deserialize_with = "deserialize_nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_hook_admission: Option<Option<WorkspaceHookAdmissionState>>,
}

/// `workflowRunSchema.omit({actors, nodes})`：run 的 header。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunHeader {
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
    pub resumable: Option<crate::workflow::LiteralTrue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<String>,
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

/// `workflowRunHeaderSchema.partial()`：在场的键整键替换，缺席保持不动。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunHeaderPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkflowRunStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<WorkflowRunStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<WorkflowRunUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumable: Option<crate::workflow::LiteralTrue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_sequence: Option<u64>,
}

/// `workflowRunHeaderSchema.keyof()`：变成「缺席」的 header 键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowRunHeaderKey {
    #[serde(rename = "runId")]
    RunId,
    #[serde(rename = "toolCallId")]
    ToolCallId,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "stopReason")]
    StopReason,
    #[serde(rename = "resumedFrom")]
    ResumedFrom,
    #[serde(rename = "supersededBy")]
    SupersededBy,
    #[serde(rename = "usage")]
    Usage,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "resumable")]
    Resumable,
    #[serde(rename = "resultPreview")]
    ResultPreview,
    #[serde(rename = "reports")]
    Reports,
    #[serde(rename = "pendingQuestions")]
    PendingQuestions,
    #[serde(rename = "concurrency")]
    Concurrency,
    #[serde(rename = "concurrencyCeiling")]
    ConcurrencyCeiling,
    #[serde(rename = "subagentModel")]
    SubagentModel,
    #[serde(rename = "artifacts")]
    Artifacts,
    #[serde(rename = "phases")]
    Phases,
    #[serde(rename = "currentPhase")]
    CurrentPhase,
    #[serde(rename = "phaseNames")]
    PhaseNames,
    #[serde(rename = "phaseAlongside")]
    PhaseAlongside,
    #[serde(rename = "unlistedByPhase")]
    UnlistedByPhase,
    #[serde(rename = "truncated")]
    Truncated,
    #[serde(rename = "lastEventSequence")]
    LastEventSequence,
}

/// `workflowRunEntryRefSchema`：actors/nodes 共用的去重身份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunEntryRef {
    pub site_id: String,
    pub ordinal: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowAppendedOp {
    #[serde(rename = "row.appended")]
    RowAppended,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowAppendedDelta {
    pub op: RowAppendedOp,
    pub row: ConversationRow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowUpsertedOp {
    #[serde(rename = "row.upserted")]
    RowUpserted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowUpsertedDelta {
    pub op: RowUpsertedOp,
    pub row: ConversationRow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowRemovedOp {
    #[serde(rename = "row.removed")]
    RowRemoved,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowRemovedDelta {
    pub op: RowRemovedOp,
    pub from_row_id: Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamablePath {
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "inputText")]
    InputText,
    #[serde(rename = "output.text")]
    OutputText,
    #[serde(rename = "summaryText")]
    SummaryText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RowDeltaOp {
    #[serde(rename = "row.delta")]
    RowDelta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowDeltaDelta {
    pub op: RowDeltaOp,
    pub row_id: Number,
    pub path: StreamablePath,
    pub append: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StateUpdatedOp {
    #[serde(rename = "state.updated")]
    StateUpdated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateUpdatedDelta {
    pub op: StateUpdatedOp,
    pub patch: StatePatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowRunUpdatedOp {
    #[serde(rename = "workflowRun.updated")]
    WorkflowRunUpdated,
}

/// `workflowRun.updated`：header 整键替换、条目按 (siteId, ordinal) 整条 upsert/删除。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunUpdatedDelta {
    pub op: WorkflowRunUpdatedOp,
    pub run_id: String,
    pub revision: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<WorkflowRunHeaderPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared: Option<Vec<WorkflowRunHeaderKey>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_actors: Option<Vec<WorkflowRunEntryRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_nodes: Option<Vec<WorkflowRunEntryRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actors: Option<Vec<WorkflowRunActor>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<WorkflowRunNode>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowRunRemovedOp {
    #[serde(rename = "workflowRun.removed")]
    WorkflowRunRemoved,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunRemovedDelta {
    pub op: WorkflowRunRemovedOp,
    pub run_id: String,
    pub revision: Number,
}

/// `conversationDeltaSchema`：七个 op 的封闭判别联合。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConversationDelta {
    RowAppended(RowAppendedDelta),
    RowUpserted(RowUpsertedDelta),
    RowRemoved(RowRemovedDelta),
    RowDelta(RowDeltaDelta),
    StateUpdated(Box<StateUpdatedDelta>),
    WorkflowRunUpdated(WorkflowRunUpdatedDelta),
    WorkflowRunRemoved(WorkflowRunRemovedDelta),
}
