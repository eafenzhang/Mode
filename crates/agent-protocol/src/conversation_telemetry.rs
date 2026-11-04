//! `v4/telemetry/event` 会话事实的严格镜像。
//!
//! 真源：`packages/shared/src/mode-protocol-v4/telemetry.ts`。
//! 每个分支按 `kind` 判别并拒绝未知字段；`inputSource` 等开放词表保留为字符串，
//! 避免把上游枚举复制成难以维护的镜像。`validate` 覆盖 zod 的 `superRefine` 约束。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationTelemetryFactBase {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_enabled: Option<bool>,
    pub version: u8,
    pub event_id: String,
    pub event_seq: u64,
    pub occurred_at: f64,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionKind {
    Agent,
    ControlOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundSource {
    Bash,
    Subagent,
    Workflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OffPeakRunType {
    Init,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskTrigger {
    Schedule,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRequestStatus {
    ModelRequestStarted,
    ModelRequestCompleted,
    ModelRequestFailed,
    ModelRetryScheduled,
    ModelStreamStalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamChannel {
    Thought,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolLifecyclePhase {
    Scheduled,
    Started,
    Progress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Agents,
    Mode,
    Bundled,
    Plugin,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionLifecyclePhase {
    Requested,
    Resolved,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDecision {
    Allow,
    Deny,
    Escalate,
    Modify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentLifecyclePhase {
    Spawned,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowLifecyclePhase {
    ActorSpawned,
    RunSettled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStatus {
    Completed,
    Errored,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowStopReason {
    User,
    Model,
    Provider,
    Interrupted,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnTerminalStatus {
    Success,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    Manual,
    Auto,
    Partial,
    Reactive,
    SessionMemory,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolPerformance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_run_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_output_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_output_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_out: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_read_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_write_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_match_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_file_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunk_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_attempts: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_kind: Option<String>,
}

// 各分支字段数差异大；这是 wire 形状的镜像，装箱会改变调用方匹配体验。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ConversationTelemetryFact {
    #[serde(rename = "turn.started", rename_all = "camelCase")]
    TurnStarted {
        #[serde(flatten)]
        base: ConversationTelemetryFactBase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_kind: Option<ExecutionKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background_source: Option<BackgroundSource>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        automation_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        off_peak_task_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        off_peak_run_type: Option<OffPeakRunType>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_trigger: Option<TaskTrigger>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scheduled_at: Option<f64>,
    },
    #[serde(rename = "model.request.status", rename_all = "camelCase")]
    ModelRequestStatus {
        #[serde(flatten)]
        base: ConversationTelemetryFactBase,
        request_id: String,
        status: ModelRequestStatus,
        provider_id: String,
        model_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_hostname: Option<String>,
        transport: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query_source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query_id: Option<String>,
        attempt: u64,
        max_attempts: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retryable: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status_code: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delay_ms: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_attempt: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idle_ms: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<f64>,
    },
    #[serde(rename = "stream.chunk", rename_all = "camelCase")]
    StreamChunk {
        #[serde(flatten)]
        base: ConversationTelemetryFactBase,
        channel: StreamChannel,
        chunk_length: u64,
        first_chunk: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assistant_message_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        part_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_call_id: Option<String>,
    },
    #[serde(rename = "tool.lifecycle", rename_all = "camelCase")]
    ToolLifecycle {
        #[serde(flatten)]
        base: ConversationTelemetryFactBase,
        phase: ToolLifecyclePhase,
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skill_qualified_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skill_plugin_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skill_source: Option<SkillSource>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        performance: Option<ToolPerformance>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        automation_id: Option<String>,
    },
    #[serde(rename = "permission.lifecycle", rename_all = "camelCase")]
    PermissionLifecycle {
        #[serde(flatten)]
        base: ConversationTelemetryFactBase,
        phase: PermissionLifecyclePhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<PermissionDecision>,
    },
    #[serde(rename = "usage.delta", rename_all = "camelCase")]
    UsageDelta {
        #[serde(flatten)]
        base: ConversationTelemetryFactBase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_kind: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_hostname: Option<String>,
        input_tokens: f64,
        output_tokens: f64,
        total_tokens: f64,
        reasoning_tokens: f64,
        cache_read_tokens: f64,
        cache_write_tokens: f64,
    },
    #[serde(rename = "subagent.lifecycle", rename_all = "camelCase")]
    SubagentLifecycle {
        #[serde(flatten)]
        base: ConversationTelemetryFactBase,
        phase: SubagentLifecyclePhase,
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_type: Option<String>,
        child_session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_call_id: Option<String>,
        background: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
    },
    #[serde(rename = "workflow.lifecycle", rename_all = "camelCase")]
    WorkflowLifecycle {
        #[serde(flatten)]
        base: ConversationTelemetryFactBase,
        phase: WorkflowLifecyclePhase,
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<WorkflowStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<WorkflowStopReason>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
    },
    #[serde(rename = "turn.terminal", rename_all = "camelCase")]
    TurnTerminal {
        #[serde(flatten)]
        base: ConversationTelemetryFactBase,
        status: TurnTerminalStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_count: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_count: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_retryable: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_phase: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background_subagent_result_consumed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_result_consumed: Option<bool>,
    },
    #[serde(rename = "compaction.terminal", rename_all = "camelCase")]
    CompactionTerminal {
        #[serde(flatten)]
        base: ConversationTelemetryFactBase,
        operation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary_message_id: Option<String>,
        status: CompactionStatus,
        trigger: CompactionTrigger,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        compact_reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attempt: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_attempts: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ended_at: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_compact_token_count: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_compact_token_count: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        true_post_compact_token_count: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_provider: Option<String>,
    },
}

impl ConversationTelemetryFact {
    /// zod `superRefine` 的四条组合约束。
    pub fn validate(&self) -> Result<(), String> {
        match self {
            ConversationTelemetryFact::TurnStarted {
                automation_id,
                off_peak_task_id,
                off_peak_run_type,
                ..
            } => {
                if automation_id.is_some() && off_peak_task_id.is_some() {
                    return Err(
                        "automationId and offPeakTaskId are mutually exclusive".to_owned()
                    );
                }
                if off_peak_run_type.is_some() && off_peak_task_id.is_none() {
                    return Err("offPeakRunType requires offPeakTaskId".to_owned());
                }
            }
            ConversationTelemetryFact::WorkflowLifecycle {
                phase,
                agent_id,
                child_session_id,
                status,
                ..
            } => {
                if *phase == WorkflowLifecyclePhase::ActorSpawned
                    && (agent_id.is_none() || child_session_id.is_none())
                {
                    return Err("actor-spawned requires agentId and childSessionId".to_owned());
                }
                if *phase == WorkflowLifecyclePhase::RunSettled && status.is_none() {
                    return Err("run-settled requires status".to_owned());
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(kind_extra: serde_json::Value) -> serde_json::Value {
        let mut value = serde_json::json!({
            "version": 1,
            "eventId": "e1",
            "eventSeq": 1,
            "occurredAt": 1700000000000_i64,
            "sessionId": "s1"
        });
        let map = value.as_object_mut().unwrap();
        for (key, field) in kind_extra.as_object().unwrap() {
            map.insert(key.clone(), field.clone());
        }
        value
    }

    #[test]
    fn parses_turn_started_and_enforces_mutual_exclusion() {
        let value = base(serde_json::json!({
            "kind": "turn.started",
            "automationId": "a1",
            "offPeakTaskId": "o1"
        }));
        let fact: ConversationTelemetryFact = serde_json::from_value(value).unwrap();
        assert!(fact.validate().is_err());
        assert!(matches!(
            fact,
            ConversationTelemetryFact::TurnStarted { .. }
        ));
    }

    #[test]
    fn rejects_unknown_fields_in_nested_performance() {
        let value = base(serde_json::json!({
            "kind": "tool.lifecycle",
            "phase": "completed",
            "toolCallId": "t1",
            "performance": { "totalMs": 1, "extra": true }
        }));
        assert!(serde_json::from_value::<ConversationTelemetryFact>(value).is_err());
    }

    #[test]
    fn workflow_actor_spawned_requires_child_ids() {
        let value = base(serde_json::json!({
            "kind": "workflow.lifecycle",
            "phase": "actor-spawned",
            "runId": "r1"
        }));
        let fact: ConversationTelemetryFact = serde_json::from_value(value).unwrap();
        assert_eq!(
            fact.validate(),
            Err("actor-spawned requires agentId and childSessionId".to_owned())
        );
    }

    #[test]
    fn compaction_fact_round_trips() {
        let value = base(serde_json::json!({
            "kind": "compaction.terminal",
            "operationId": "c1",
            "status": "completed",
            "trigger": "session_memory"
        }));
        let fact: ConversationTelemetryFact = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(fact.validate(), Ok(()));
        let encoded = serde_json::to_value(&fact).unwrap();
        assert_eq!(encoded["kind"], "compaction.terminal");
        assert_eq!(encoded["trigger"], "session_memory");
    }
}
