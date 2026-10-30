//! v4 只读查询：`conversationRowsRange` 与 `conversationPlans`。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/transport.ts`。规范见
//! `docs/specs/session-write-group.md` §「v4 只读查询」。
//!
//! 两者都是「只读、无状态、超时重发安全」的查询，刻意不是 v4 command：没有 ACK、
//! 没有幂等表，调用方按陈旧读防护自行丢弃结果。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

use crate::v4_rows::{ConversationRow, ToolCallRow};
use crate::v4_transport::V4ClientMode;
use crate::v4_workflow_runs::{WorkflowRunStatus, WorkflowRunStopReason};
use crate::workflow::{LiteralTrue, WorkflowArtifactKind};

pub const V4_CONVERSATION_ROWS_RANGE_METHOD: &str = "v4/conversation/rowsRange";
pub const V4_CONVERSATION_PLANS_METHOD: &str = "v4/conversation/plans";
/// 协议限制 `1..=rowsRangeMaxLimit`；由 Agent 侧 schema 拒绝，客户端不改写。
pub const ROWS_RANGE_MAX_LIMIT: u64 = 200;

/// `v4ConversationRowsRangeParamsSchema`：zod 未加 `.strict()`，未知键按 strip 忽略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsRangeParams {
    pub session_id: String,
    /// Host attachment 注入的可信位；renderer 侧调用缺省省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_mode: Option<V4ClientMode>,
    /// 取 `rowId < beforeRowId` 的行；缺省从当前尾部向前。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_row_id: Option<Number>,
    pub limit: Number,
}

/// `v4ConversationRowsRangeResultSchema`：`rows` 按 `rowId` 升序，未知键忽略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsRangeResult {
    pub rows: Vec<ConversationRow>,
    pub at_seq: Number,
    pub at_revision: Number,
    pub at_log_epoch: String,
    pub has_more: bool,
}

/// `v4ConversationPlansParamsSchema`：严格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlansParams {
    pub session_id: String,
}

/// `v4ConversationPlansResultSchema`：严格；`plans` 为当前有效分支的终态 ExitPlanMode，
/// `rowId` 降序。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlansResult {
    pub plans: Vec<ToolCallRow>,
    pub at_seq: Number,
    pub at_log_epoch: String,
}

// ── workflow run 事件日志 ──
// 读的是 journal（`dwf_event`），与 conversation log 无关，cursor 永不失效：
// 所以这一族**刻意不带** `atSeq` / `atLogEpoch`（陈旧读防护在那里定义不出正确语义）。

pub const V4_CONVERSATION_WORKFLOW_RUN_EVENTS_METHOD: &str =
    "v4/conversation/workflowRunEvents";
pub const V4_CONVERSATION_WORKFLOW_RUNS_METHOD: &str = "v4/conversation/workflowRuns";
pub const WORKFLOW_RUN_EVENTS_MAX_LIMIT: u64 = 500;
pub const WORKFLOW_RUNS_QUERY_MAX_LIMIT: u64 = 64;

/// `v4ConversationWorkflowRunEventsParamsSchema`：严格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunEventsParams {
    pub session_id: String,
    pub run_id: String,
    /// 只取 `sequence > afterSequence` 的事件；缺省从头取。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// `v4ConversationWorkflowRunEventsResultSchema`：严格。`payload` 已由 CLI 侧有界化，
/// 读端按 `type` 解释，这里保留为开放对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunEventsResult {
    pub events: Vec<WorkflowRunEvent>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunEvent {
    pub sequence: u64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// `v4ConversationWorkflowRunsParamsSchema`：严格；`limit` 的缺省与钳制在 CLI 侧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunsQueryParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// `v4ConversationWorkflowRunsResultSchema`：严格；journal-backed 的重启后发现面。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunsQueryResult {
    pub runs: Vec<WorkflowRunSummary>,
}

/// `v4ConversationWorkflowRunSummarySchema`：严格。与 `workflowRuns` 投影共用状态词汇，
/// 但 schema 独立（查询状态变化不改变投影侧）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunSummary {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    pub status: WorkflowRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<WorkflowRunStopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_message: Option<String>,
    pub resumable: bool,
}

// ── 用户面产物（`artifact.*` 发布给用户看的产出）──
// 术语：这是**脚本发布给用户看的产物**，不是引擎内部对「脚本顶层返回值」的叫法
// （后者在 `workflow.rs` 的 `WorkflowRunArtifact` / mode v1）。字节永不进清单 schema，
// 只有 `workflowRunArtifactRead` 一次一块的 base64。

pub const V4_CONVERSATION_WORKFLOW_RUN_ARTIFACTS_METHOD: &str =
    "v4/conversation/workflowRunArtifacts";
pub const V4_CONVERSATION_WORKFLOW_RUN_ARTIFACT_DATA_METHOD: &str =
    "v4/conversation/workflowRunArtifactData";
pub const V4_CONVERSATION_WORKFLOW_RUN_ARTIFACT_READ_METHOD: &str =
    "v4/conversation/workflowRunArtifactRead";
pub const ARTIFACT_DATA_MAX_ITEMS_PER_PAGE: u64 = 500;

/// `workflowRunArtifactVersionSchema`：严格；`publishedAt` 必填。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedArtifactVersion {
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<Value>,
    pub published_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<LiteralTrue>,
}

/// `workflowRunArtifactSchema`：严格；顶层字段取最新版的值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedArtifact {
    pub id: String,
    pub kind: WorkflowArtifactKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<Value>,
    pub version: u64,
    /// 版本升序；失败的发布不在这里。
    pub versions: Vec<PublishedArtifactVersion>,
    pub item_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<LiteralTrue>,
}

/// `v4ConversationWorkflowRunArtifactsParamsSchema`：严格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunArtifactsParams {
    pub session_id: String,
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunArtifactsResult {
    /// 按首次出现顺序（journal 里该 id 第一条 artifact 行的 ordinal）。
    pub artifacts: Vec<PublishedArtifact>,
}

/// `v4ConversationWorkflowRunArtifactDataParamsSchema`：严格；limit 缺省 200、钳 [1, 500]
/// 都在网关侧执行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunArtifactDataParams {
    pub session_id: String,
    pub run_id: String,
    pub artifact_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunArtifactDataResult {
    pub items: Vec<WorkflowRunArtifactDataItem>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunArtifactDataItem {
    /// journal sequence——回传成 `afterSequence` 就是下一页的游标。
    pub sequence: u64,
    pub site_id: String,
    pub ordinal: u64,
    /// 条目原值不做预览序列化（看板按字段路径取数）。
    pub item: Value,
}

/// `v4ConversationWorkflowRunArtifactReadParamsSchema`：严格；每块 ≤ 512 KiB。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunArtifactReadParams {
    pub session_id: String,
    pub run_id: String,
    pub artifact_id: String,
    pub version: u64,
    pub offset: u64,
    pub limit: u64,
}

/// `v4ConversationWorkflowRunArtifactReadResultSchema`：严格。`nextOffset` 是 nullable
/// 而非 optional（读到尾时显式为 null）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunArtifactReadResult {
    pub data_base64: String,
    pub media_type: String,
    pub total_bytes: u64,
    pub next_offset: Option<u64>,
}

// ── 工作区 transcript（`files.*` / `git.*` / `world.run` 的卡片回放）──
// 权威在 journal 的 `dwf_node` 行；清单不带正文，正文单独取并按 maxBytes 保形有界化。

pub const V4_CONVERSATION_WORKFLOW_RUN_WORKSPACE_METHOD: &str =
    "v4/conversation/workflowRunWorkspace";
pub const V4_CONVERSATION_WORKFLOW_RUN_NODE_RESULT_METHOD: &str =
    "v4/conversation/workflowRunNodeResult";
pub const WORKFLOW_WORKSPACE_MAX_NODES: u64 = 2000;
pub const WORKFLOW_NODE_RESULT_MAX_BYTES: u64 = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceNodeKind {
    #[serde(rename = "world-read")]
    WorldRead,
    #[serde(rename = "world-run")]
    WorldRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceNodeStatus {
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
}

/// journal `error_json` 的线上镜像（只有 code + message 出协议）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceNodeError {
    pub code: String,
    pub message: String,
}

/// 清单上一行的摘要：不解正文就能报的几个数，哪个在场取决于 op。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceNodeSummary {
    pub result_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_bytes: Option<u64>,
}

/// `workflowRunWorkspaceNodeSchema`：严格。升级前的历史行 `op`/`args` 缺席。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceNode {
    pub site_id: String,
    pub ordinal: u64,
    pub kind: WorkspaceNodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_truncated: Option<LiteralTrue>,
    pub status: WorkspaceNodeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WorkspaceNodeError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<WorkspaceNodeSummary>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunWorkspaceParams {
    pub session_id: String,
    pub run_id: String,
}

/// 清单按落库先后（journal 行 id 升序 = 引擎准入顺序）；超界截尾并置 `truncated`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunWorkspaceResult {
    pub nodes: Vec<WorkspaceNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunNodeResultParams {
    pub session_id: String,
    pub run_id: String,
    pub site_id: String,
    pub ordinal: u64,
    /// 缺省与上限都是 32 KiB；网关钳制。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
}

/// `v4ConversationWorkflowRunNodeResultResultSchema`：严格。`result` 是可选开放对象
/// （running / failed 行缺席）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRunNodeResult {
    pub status: WorkspaceNodeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WorkspaceNodeError>,
    pub truncated: bool,
    pub total_bytes: u64,
}
