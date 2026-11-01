//! v4 sessions-index topic：侧栏列表的活性数据源（conflated 最新态，永不溢出）。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/sessions-index.ts`、
//! `sessions-index-workflow-activity.ts` 与 `transport.ts` 的
//! `sessionsIndexTopicFrameSchema` / `createTopicWireFrameSchema`。
//!
//! 与 conversation topic 的差异：
//! - 帧没有 `ttft` / `ttftRelated`（那两个只挂在 conversation subscribe ACK 与帧上）；
//! - topic 必须形如 `sessions-index/<workspaceId>`，workspaceId 非空；
//! - 快照/增量共用同一套 `SessionSummary`，`sessionId` 是 conflation key。
//!
//! `SessionSummary` 的内层 schema 是普通 `z.object`（strip 未知键），所以这里不加
//! `deny_unknown_fields`；数组上界与名字长度在 `validate` 里补上（serde 解码不做 zod 的
//! 长度校验）。

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::v4_snapshot::{GoalStatus, InteractionAutoResolution, SessionPhase, TitleSource};
use crate::v4_transport::{
    FragmentWireFrame, FramePayloadKindDeltas, FramePayloadKindSnapshot, WireDeliveryKind,
    WireFrameKindComplete,
};
use crate::v4_workflow_runs::{WorkflowRunStatus, WorkflowRunStopReason, WORKFLOW_RUNS_MAX_PHASES};

pub const SESSIONS_INDEX_TOPIC_PREFIX: &str = "sessions-index/";
pub const SESSION_WORKFLOW_ACTIVITY_MAX_RUNS: usize = 4;
pub const SESSION_WORKFLOW_MAX_PHASES: usize = WORKFLOW_RUNS_MAX_PHASES;
pub const SESSION_WORKFLOW_MAX_PHASE_NAME_LENGTH: usize = 128;

/// `sessionsIndexTopic(workspaceId)`。
pub fn sessions_index_topic(workspace_id: &str) -> String {
    format!("{SESSIONS_INDEX_TOPIC_PREFIX}{workspace_id}")
}

/// `parseSessionsIndexTopic`：前缀 + 非空 workspaceId。
pub fn parse_sessions_index_topic(topic: &str) -> Option<&str> {
    let workspace_id = topic.strip_prefix(SESSIONS_INDEX_TOPIC_PREFIX)?;
    (!workspace_id.is_empty()).then_some(workspace_id)
}

/// `sessionPendingInteractionSummarySchema`：只允许 permission / userInput。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionPendingInteractionKind {
    #[serde(rename = "permission")]
    Permission,
    #[serde(rename = "userInput")]
    UserInput,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPendingInteractionSummary {
    pub interaction_id: String,
    pub kind: SessionPendingInteractionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resolution: Option<InteractionAutoResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingInteractionSummary {
    pub permission_count: u64,
    pub user_input_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionWorkflowPhaseStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "failed")]
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWorkflowPhaseSummary {
    pub name: String,
    pub status: SessionWorkflowPhaseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alongside: Option<Vec<u64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWorkflowRunSummary {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub status: WorkflowRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<WorkflowRunStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Number>,
    pub phases: Vec<SessionWorkflowPhaseSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_phase: Option<String>,
    pub agents_working: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWorkflowActivity {
    pub runs: Vec<SessionWorkflowRunSummary>,
}

/// `sessionSummarySchema`。可选字段只区分「缺席」；`titleSource` 的缺席表示旧 frame。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub session_id: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_source: Option<TitleSource>,
    pub phase: SessionPhase,
    pub session_ended: bool,
    pub has_background_work: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_activity: Option<SessionWorkflowActivity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_interaction: Option<SessionPendingInteractionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_interaction_summary: Option<PendingInteractionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_status: Option<GoalStatus>,
    pub last_activity_at: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_assistant_preview: Option<String>,
    pub created_at: Number,
}

impl SessionSummary {
    /// 补上 zod 的长度/上界约束（serde 解码不做这些）。
    pub fn validate(&self) -> Result<(), String> {
        if let Some(activity) = &self.workflow_activity {
            if activity.runs.len() > SESSION_WORKFLOW_ACTIVITY_MAX_RUNS {
                return Err(format!(
                    "workflowActivity.runs exceeds {SESSION_WORKFLOW_ACTIVITY_MAX_RUNS}"
                ));
            }
            for run in &activity.runs {
                run.validate()?;
            }
        }
        Ok(())
    }
}

impl SessionWorkflowRunSummary {
    fn validate(&self) -> Result<(), String> {
        if self.run_id.is_empty() {
            return Err("workflow run runId must not be empty".to_string());
        }
        if matches!(self.tool_call_id.as_deref(), Some("")) {
            return Err("workflow run toolCallId must not be empty".to_string());
        }
        if matches!(self.name.as_deref(), Some("")) {
            return Err("workflow run name must not be empty".to_string());
        }
        if self.phases.len() > SESSION_WORKFLOW_MAX_PHASES {
            return Err(format!(
                "workflow run phases exceed {SESSION_WORKFLOW_MAX_PHASES}"
            ));
        }
        if let Some(current) = &self.current_phase {
            validate_phase_name(current)?;
        }
        for phase in &self.phases {
            validate_phase_name(&phase.name)?;
            for other in phase.alongside.iter().flatten() {
                if *other as usize >= self.phases.len() {
                    return Err("workflow phase alongside index out of range".to_string());
                }
            }
        }
        Ok(())
    }
}

fn validate_phase_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > SESSION_WORKFLOW_MAX_PHASE_NAME_LENGTH {
        return Err(format!(
            "workflow phase name must be 1..={SESSION_WORKFLOW_MAX_PHASE_NAME_LENGTH}"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsIndexSnapshot {
    pub protocol_version: u64,
    pub workspace_id: String,
    pub log_epoch: String,
    pub sessions: Vec<SessionSummary>,
}

impl SessionsIndexSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version != 1 {
            return Err(format!(
                "unsupported sessions-index protocolVersion {}",
                self.protocol_version
            ));
        }
        for session in &self.sessions {
            session.validate()?;
        }
        Ok(())
    }
}

/// `sessionsIndexDeltaSchema`：discriminated union on `op`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SessionsIndexDelta {
    #[serde(rename = "session.upserted")]
    SessionUpserted { session: Box<SessionSummary> },
    #[serde(rename = "session.removed")]
    SessionRemoved { session_id: String },
}

impl SessionsIndexDelta {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::SessionUpserted { session } => session.validate(),
            Self::SessionRemoved { .. } => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionsIndexSnapshotPayload {
    pub kind: FramePayloadKindSnapshot,
    pub snapshot: SessionsIndexSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionsIndexDeltasPayload {
    pub kind: FramePayloadKindDeltas,
    pub deltas: Vec<SessionsIndexDelta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionsIndexFramePayload {
    Snapshot(Box<SessionsIndexSnapshotPayload>),
    Deltas(SessionsIndexDeltasPayload),
}

/// `sessionsIndexTopicFrameSchema`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionsIndexTopicFrame {
    pub topic: String,
    pub subscription_id: String,
    pub from_seq: Number,
    pub to_seq: Number,
    pub sent_at: Number,
    pub payload: SessionsIndexFramePayload,
}

impl SessionsIndexTopicFrame {
    /// `superRefine` + 内层 zod 约束。
    pub fn validate(&self) -> Result<(), String> {
        if parse_sessions_index_topic(&self.topic).is_none() {
            return Err("invalid sessions-index topic".to_string());
        }
        match &self.payload {
            SessionsIndexFramePayload::Snapshot(payload) => payload.snapshot.validate(),
            SessionsIndexFramePayload::Deltas(payload) => {
                for delta in &payload.deltas {
                    delta.validate()?;
                }
                Ok(())
            }
        }
    }
}

/// `createTopicWireFrameSchema(sessionsIndexTopicFrameSchema)` 的 complete 分支。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionsIndexCompleteWireFrame {
    pub wire_version: u64,
    pub kind: WireFrameKindComplete,
    pub delivery_kind: WireDeliveryKind,
    pub logical_frame_id: String,
    pub logical_frame_ordinal: u64,
    pub topic: String,
    pub subscription_id: String,
    pub frame: SessionsIndexTopicFrame,
}

impl SessionsIndexCompleteWireFrame {
    pub fn validate(&self) -> Result<(), String> {
        self.frame.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionsIndexWireFrame {
    Complete(Box<SessionsIndexCompleteWireFrame>),
    Fragment(FragmentWireFrame),
}

impl SessionsIndexWireFrame {
    pub fn subscription_id(&self) -> &str {
        match self {
            Self::Complete(frame) => &frame.subscription_id,
            Self::Fragment(frame) => &frame.subscription_id,
        }
    }

    /// 分片只校验 topic 前缀；完整帧再校验 payload。
    pub fn validate(&self) -> Result<(), String> {
        let topic = match self {
            Self::Complete(frame) => {
                frame.validate()?;
                &frame.topic
            }
            Self::Fragment(frame) => &frame.topic,
        };
        if parse_sessions_index_topic(topic).is_none() {
            return Err("invalid sessions-index topic".to_string());
        }
        Ok(())
    }
}
