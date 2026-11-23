//! v4 帧订阅、sessions-index 与只读 query 族（C4 T4c，裁决③「本波全量实现」）。
//!
//! - `v4/conversation/subscribe`（`conversation/<sessionId>` 与
//!   `sessions-index/<workspaceId>` 两种 topic）：ACK + 初始快照帧；conversation
//!   topic 另起 live 转发任务，把会话事件映射成行/状态增量帧。
//! - `resync`（`mode:"resume"`）/ `unsubscribe`。
//! - 只读 query：rowsRange / plans / usage / fileChanges / backgroundBashOutput /
//!   usageStats / workflow run 七查询。
//!
//! 登记差异（契约 §5.9）：createdAtSeq 用 DB sequence 近似事件 seq；cold 会话的
//! usage/contextWindow 缺省；backgroundBashOutput 恒 unavailable；fileChanges 只做
//! stale 守卫 + 空 items；sessions-index 的列表行 live upsert/remove 已接，
//! 会话内 phase/预览/后台工作投影仍未接。

use agent_protocol::v4_queries::{
    AppUsageDailyModelItem, AppUsageDailyModelUsage, AppUsageFavoriteModel, AppUsageHeatmap,
    AppUsageHeatmapCell, AppUsageHeatmapWeek, AppUsageModelUsage, AppUsageRange, AppUsageSource,
    AppUsageSummary, AppUsageToolUsage, BackgroundBashOutput, BackgroundBashOutputKind,
    BackgroundBashOutputParams, BackgroundBashOutputResult, BackgroundBashStatus,
    BackgroundBashUnavailable, BackgroundBashUnavailableKind, ConversationUsageParams,
    ConversationUsageResult, FileChangesParams, FileRewindPreviewParams, FileRewindPreviewResult,
    PlansParams, PlansResult, RewindAction, RewindSafeFile, RewindUnsafeFile, RewindUnsafeReason,
    RowsRangeParams, RowsRangeResult, UsageStatsParams, WorkflowRunEventsParams,
    WorkflowRunEventsResult, WorkflowRunSummary, WorkflowRunsQueryParams, WorkflowRunsQueryResult,
    V4_BACKGROUND_BASH_OUTPUT_METHOD, V4_CONVERSATION_FILE_CHANGES_METHOD,
    V4_CONVERSATION_PLANS_METHOD, V4_CONVERSATION_ROWS_RANGE_METHOD, V4_CONVERSATION_USAGE_METHOD,
    V4_CONVERSATION_WORKFLOW_RUNS_METHOD, V4_CONVERSATION_WORKFLOW_RUN_EVENTS_METHOD,
    V4_USAGE_STATS_METHOD,
};
use agent_protocol::v4_queries::{
    ConversationResyncParams, ConversationUnsubscribeParams, V4_CONVERSATION_RESYNC_METHOD,
    V4_CONVERSATION_UNSUBSCRIBE_METHOD,
};
use agent_protocol::v4_sessions_index::{
    SessionSummary, SessionsIndexSnapshot, SESSIONS_INDEX_TOPIC_PREFIX,
};
use agent_protocol::v4_snapshot::{GoalState, GoalStatus};
use agent_protocol::v4_transport::{
    ConversationSubscribeAck, ConversationSubscribeParams, ConversationSubscribeResult,
    SubscribeMode, V4ClientMode, V4_CONVERSATION_SUBSCRIBE_METHOD,
};
use serde_json::{json, Number, Value};
use std::collections::BTreeMap;

use crate::dispatch::{AgentServer, PendingV4Forward};
use crate::server::ServerError;
use crate::sessions_index::summary_from_record;
use crate::v4_projection::{build_snapshot, project_rows, ProjectedMessage};

use super::parse_params;

/// `load_projected` 的行视图：message id / role / sequence / time / data / parts。
pub(crate) type LoadedMessage = (
    String,
    String,
    Option<i64>,
    i64,
    Value,
    Vec<(String, Option<i64>, i64, Value)>,
);
/// `dwf_run` 查询行：id/toolCallId/name/updated/status/resumed/failure。
type WorkflowRunRowTuple = (
    String,
    Option<String>,
    Option<String>,
    i64,
    String,
    Option<String>,
    Option<String>,
);

// ── logEpoch / 订阅台账 ─────────────────────────────────────────────────────

fn base36(value: u128) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    let mut rest = value;
    while rest > 0 {
        out.push(ALPHABET[(rest % 36) as usize]);
        rest /= 36;
    }
    out.iter().rev().map(|byte| *byte as char).collect()
}

fn next_epoch(server: &AgentServer) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() % 0x2_0000_0000)
        .unwrap_or(0);
    format!(
        "{}-{}",
        base36(nanos),
        base36(server.v4_state().next_epoch_counter())
    )
}

fn log_epoch_for(server: &AgentServer, key: &str) -> String {
    server.v4_state().log_epoch_for(key, || next_epoch(server))
}

fn current_seq_revision(server: &AgentServer, session_id: &str) -> (u64, u64, bool) {
    match server.find_runtime(session_id) {
        Some(runtime) => (runtime.event_seq(), runtime.state_revision(), true),
        None => (0, 0, false),
    }
}

/// message/part → 投影视图（避免生命周期问题：克隆数据）。
fn load_projected(
    store: &session_store::SessionStore,
    session_id: &str,
) -> Result<Vec<LoadedMessage>, ServerError> {
    let messages = store
        .messages(session_id, None, None)
        .map_err(crate::methods::store_error)?;
    Ok(messages
        .into_iter()
        .map(|with_parts| {
            (
                with_parts.message.id.clone(),
                with_parts
                    .message
                    .data
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                with_parts.message.sequence,
                with_parts.message.time_created,
                with_parts.message.data.clone(),
                with_parts
                    .parts
                    .into_iter()
                    .map(|part| (part.id, part.sequence, part.time_created, part.data))
                    .collect(),
            )
        })
        .collect())
}

fn rows_from_loaded(loaded: &[LoadedMessage]) -> Vec<agent_protocol::v4_rows::ConversationRow> {
    let projected: Vec<ProjectedMessage<'_>> = loaded
        .iter()
        .map(|(id, role, sequence, time, data, parts)| ProjectedMessage {
            id,
            role,
            sequence: *sequence,
            time_created: *time,
            data,
            parts,
        })
        .collect();
    project_rows(&projected)
}

fn projected_from_loaded(loaded: &[LoadedMessage]) -> Vec<ProjectedMessage<'_>> {
    loaded
        .iter()
        .map(|(id, role, sequence, time, data, parts)| ProjectedMessage {
            id,
            role,
            sequence: *sequence,
            time_created: *time,
            data,
            parts,
        })
        .collect()
}

fn goal_from_store(
    store: &session_store::SessionStore,
    session_id: &str,
) -> Result<Option<GoalState>, ServerError> {
    Ok(store
        .get_target(session_id)
        .map_err(crate::methods::store_error)?
        .map(|target| GoalState {
            target_id: target.target_id,
            objective: target.objective,
            summary_title: Some(Some(target.summary_title.unwrap_or_default())),
            time_used_seconds: target.time_used_seconds.max(0) as u64,
            active_run_started_at_ms: target
                .active_run_started_at
                .filter(|value| *value > 0)
                .map(|value| value as u64),
            status: match target.status.as_str() {
                "paused" => GoalStatus::Paused,
                "complete" => GoalStatus::Verified,
                "budget_limited" => GoalStatus::Failed,
                _ => GoalStatus::Active,
            },
            iteration: Number::from(1u64),
            verifications: Vec::new(),
            iterations: Vec::new(),
        }))
}

// ── subscribe（conversation + sessions-index 两种 topic）────────────────────

pub(crate) fn handle_conversation_subscribe(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: ConversationSubscribeParams =
        parse_params(V4_CONVERSATION_SUBSCRIBE_METHOD, params)?;
    params.validate().map_err(ServerError::invalid_params)?;
    if let Some(workspace_id) = params.topic.strip_prefix(SESSIONS_INDEX_TOPIC_PREFIX) {
        return subscribe_sessions_index(server, workspace_id);
    }
    if let Some(workspace_id) =
        agent_protocol::v4_workspace_config::parse_workspace_config_topic(&params.topic)
    {
        return subscribe_workspace_config(server, workspace_id, params.client_mode);
    }
    let session_id = params.topic.strip_prefix("conversation/").ok_or_else(|| {
        ServerError::invalid_params(format!(
            "topic must be conversation/<sessionId> or {SESSIONS_INDEX_TOPIC_PREFIX}<workspaceId>: {}",
            params.topic
        ))
    })?;
    let store = server.store()?;
    let record = store
        .get_session(session_id)
        .map_err(crate::methods::store_error)?
        .ok_or_else(|| crate::methods::session_not_found(session_id))?;
    let (seq, revision, running) = current_seq_revision(server, session_id);
    let log_epoch = log_epoch_for(server, session_id);
    let loaded = load_projected(&store, session_id)?;
    let rows = rows_from_loaded(&loaded);
    let goal = goal_from_store(&store, session_id)?;
    let (provider, model) = match server.find_runtime(session_id) {
        Some(runtime) => {
            let config = runtime.config();
            (config.provider_id.clone(), config.model_id.clone())
        }
        None => (String::new(), String::new()),
    };
    let mode = super::mode_to_string(super::permission_mode(&record));
    let snapshot = build_snapshot(crate::v4_projection::SnapshotStateFacts {
        session_id,
        log_epoch: &log_epoch,
        seq,
        revision,
        title: &record.title,
        title_source: &record.title_source,
        mode: &mode,
        provider: &provider,
        model: &model,
        queue_items: Vec::new(),
        auto_drain: true,
        followup_mode: agent_protocol::v4_snapshot::FollowupMode::Queue,
        running,
        goal,
        subagents: agent_protocol::v4_snapshot::SubagentProjectionState {
            revision: 1,
            child_session_ids: subagent_children(&db_path_of(server)?, session_id),
            running: Vec::new(),
            ended_total: 0,
        },
        workflow_runs: None,
        rows,
        context_window: None,
    });
    let subscription_id = format!(
        "sub-{}-{}-1",
        log_epoch,
        base36(server.v4_state().next_epoch_counter())
    );
    let rows_total = snapshot.rows.total_count.as_u64().unwrap_or(0);
    let cancel = server.v4_state().register_subscription(
        &subscription_id,
        session_id,
        &params.topic,
        params.client_mode,
        rows_total,
    );
    server.queue_v4_forward(crate::dispatch::PendingV4Forward::Conversation {
        session_id: session_id.to_string(),
        topic: params.topic.clone(),
        subscription_id: subscription_id.clone(),
        client_mode: params.client_mode,
        log_epoch: log_epoch.clone(),
        last_seq: seq,
        cancel,
    });
    serde_json::to_value(ConversationSubscribeResult {
        ack: ConversationSubscribeAck {
            subscription_id,
            mode: SubscribeMode::Snapshot,
            log_epoch: log_epoch.clone(),
            open_timing: Some(agent_protocol::v4_transport::ConversationOpenTiming {
                version: 1,
                session_runtime_state: Some(if running {
                    agent_protocol::v4_transport::SessionRuntimeState::Warm
                } else {
                    agent_protocol::v4_transport::SessionRuntimeState::Cold
                }),
                snapshot_row_count: Some(rows_total),
                host_prepare_ms: None,
                provider_registry_sync_ms: None,
                task_meta_read_ms: None,
                cli_request_ms: None,
                cli_bootstrap_ms: None,
                cli_session_restore_ms: None,
                initial_frame_encode_ms: None,
                cli_process_state: None,
            }),
        },
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

fn db_path_of(server: &AgentServer) -> Result<std::path::PathBuf, ServerError> {
    Ok(server.store()?.db_path().to_path_buf())
}

fn subagent_children(db_path: &Path, session_id: &str) -> Vec<String> {
    let Ok(connection) =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut statement) =
        connection.prepare("select id from session where parent_id = ?1 order by rowid")
    else {
        return Vec::new();
    };
    statement
        .query_map([session_id], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

use std::path::Path;

fn subscribe_sessions_index(
    server: &AgentServer,
    workspace_id: &str,
) -> Result<Value, ServerError> {
    if workspace_id.trim().is_empty() {
        return Err(ServerError::invalid_params("workspaceId must be non-empty"));
    }
    let topic = format!("{SESSIONS_INDEX_TOPIC_PREFIX}{workspace_id}");
    let log_epoch = log_epoch_for(server, &topic);
    let subscription_id = format!(
        "six-{}-{}-1",
        log_epoch,
        base36(server.v4_state().next_epoch_counter())
    );
    let sessions = sessions_index_sessions(server, workspace_id);
    server
        .sessions_index()
        .ensure_workspace(workspace_id, &sessions);
    let sessions = server.sessions_index().snapshot(workspace_id);
    let cancel = server.v4_state().register_subscription(
        &subscription_id,
        &topic,
        &topic,
        V4ClientMode::DesktopContinuous,
        0,
    );
    server.queue_v4_forward(crate::dispatch::PendingV4Forward::SessionsIndex {
        workspace_id: workspace_id.to_string(),
        topic: topic.clone(),
        subscription_id: subscription_id.clone(),
        log_epoch: log_epoch.clone(),
        snapshot: SessionsIndexSnapshot {
            protocol_version: 1,
            workspace_id: workspace_id.to_string(),
            log_epoch: log_epoch.clone(),
            sessions,
        },
        cancel,
    });
    serde_json::to_value(ConversationSubscribeResult {
        ack: ConversationSubscribeAck {
            subscription_id,
            mode: SubscribeMode::Snapshot,
            log_epoch: log_epoch.clone(),
            open_timing: Some(agent_protocol::v4_transport::ConversationOpenTiming {
                version: 1,
                snapshot_row_count: None,
                host_prepare_ms: None,
                provider_registry_sync_ms: None,
                task_meta_read_ms: None,
                cli_request_ms: None,
                cli_bootstrap_ms: None,
                cli_session_restore_ms: None,
                initial_frame_encode_ms: None,
                cli_process_state: None,
                session_runtime_state: None,
            }),
        },
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

/// workspace-config topic：Host 用它同步 workspace 级配置目录（模型/mode/slash 命令）。
///
/// 本仓库当前只提供空目录（schema 合法）；配置目录的远端灰度不在这里重新发明。
fn subscribe_workspace_config(
    server: &AgentServer,
    workspace_id: &str,
    client_mode: agent_protocol::v4_transport::V4ClientMode,
) -> Result<Value, ServerError> {
    let workspace_id = workspace_id.trim();
    if workspace_id.is_empty() {
        return Err(ServerError::invalid_params("workspaceId must be non-empty"));
    }
    let topic = agent_protocol::v4_workspace_config::workspace_config_topic(workspace_id);
    let log_epoch = log_epoch_for(server, &topic);
    let subscription_id = format!(
        "wcfg-{}-{}-1",
        log_epoch,
        base36(server.v4_state().next_epoch_counter())
    );
    let snapshot = agent_protocol::v4_workspace_config::WorkspaceConfigSnapshot {
        protocol_version: 1,
        workspace_id: workspace_id.to_string(),
        log_epoch: log_epoch.clone(),
        config: agent_protocol::v4_workspace_config::WorkspaceConfigState {
            config_options: Vec::new(),
            slash_commands: Vec::new(),
        },
    };
    server
        .v4_state()
        .register_subscription(&subscription_id, workspace_id, &topic, client_mode, 0);
    server.queue_v4_forward(crate::dispatch::PendingV4Forward::WorkspaceConfig {
        topic: topic.clone(),
        subscription_id: subscription_id.clone(),
        log_epoch: log_epoch.clone(),
        snapshot,
        recovery: false,
    });
    serde_json::to_value(ConversationSubscribeResult {
        ack: ConversationSubscribeAck {
            subscription_id,
            mode: SubscribeMode::Snapshot,
            log_epoch,
            open_timing: None,
        },
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

fn sessions_index_sessions(server: &AgentServer, workspace_id: &str) -> Vec<SessionSummary> {
    let Ok(store) = server.store() else {
        return Vec::new();
    };
    let records = store
        .list_sessions(workspace_id, true, None)
        .unwrap_or_default();
    records
        .into_iter()
        .map(|record| {
            let mut summary = summary_from_record(&record, false);
            summary.workspace_id = workspace_id.to_string();
            summary
        })
        .collect()
}

// ── resync / unsubscribe ────────────────────────────────────────────────────

pub(crate) fn handle_conversation_resync(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    // 线形状按 §5.4：`{topic,connectionId,subscriptionId,base:{logEpoch,seq}}`；
    // ACK 回显同一 subscriptionId、`mode:"resume"`。
    let params: ConversationResyncParams = parse_params(V4_CONVERSATION_RESYNC_METHOD, params)?;
    let log_epoch = match &params.base {
        Some(base) => base.log_epoch.clone(),
        None => log_epoch_for(server, &params.topic),
    };
    let subscription_id = params.subscription_id.clone();
    server.v4_state().touch_subscription(&subscription_id);
    // 恢复帧必须落在原 subscription 的 live 转发上：UI 的 resync ACK 后等待
    // deliveryKind:"recovery" 的快照帧，只有 ACK 会触发 recoveryFrameTimedOut。
    if let Some(session_id) = params.topic.strip_prefix("conversation/") {
        server.queue_v4_forward(PendingV4Forward::ConversationRecovery {
            session_id: session_id.to_string(),
            topic: params.topic.clone(),
            subscription_id: subscription_id.clone(),
            log_epoch: log_epoch.clone(),
            from_seq: params.base.as_ref().map(|base| base.seq).unwrap_or(0),
        });
    } else if let Some(workspace_id) =
        agent_protocol::v4_workspace_config::parse_workspace_config_topic(&params.topic)
    {
        let snapshot = agent_protocol::v4_workspace_config::WorkspaceConfigSnapshot {
            protocol_version: 1,
            workspace_id: workspace_id.to_string(),
            log_epoch: log_epoch.clone(),
            config: agent_protocol::v4_workspace_config::WorkspaceConfigState {
                config_options: Vec::new(),
                slash_commands: Vec::new(),
            },
        };
        server.queue_v4_forward(PendingV4Forward::WorkspaceConfig {
            topic: params.topic.clone(),
            subscription_id: subscription_id.clone(),
            log_epoch: log_epoch.clone(),
            snapshot,
            recovery: true,
        });
    }
    serde_json::to_value(ConversationSubscribeResult {
        ack: ConversationSubscribeAck {
            subscription_id,
            mode: SubscribeMode::Resume,
            log_epoch,
            open_timing: None,
        },
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_conversation_unsubscribe(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: ConversationUnsubscribeParams =
        parse_params(V4_CONVERSATION_UNSUBSCRIBE_METHOD, params)?;
    let _ = params.connection_id;
    // 先按 subscriptionId 精确取消；缺失/不匹配时再按 topic 兜底（旧客户端）。
    if !server
        .v4_state()
        .cancel_subscription(&params.subscription_id)
    {
        server.v4_state().drop_subscription_by_topic(&params.topic);
    }
    serde_json::to_value(json!({})).map_err(|error| ServerError::internal(error.to_string()))
}

// ── 行投影 query 族 ─────────────────────────────────────────────────────────

fn projection_facts(
    server: &AgentServer,
    session_id: &str,
) -> Result<
    (
        std::sync::Arc<session_store::SessionStore>,
        String,
        u64,
        u64,
        bool,
    ),
    ServerError,
> {
    let store = server.store()?;
    store
        .get_session(session_id)
        .map_err(crate::methods::store_error)?
        .ok_or_else(|| crate::methods::session_not_found(session_id))?;
    let (seq, revision, running) = current_seq_revision(server, session_id);
    Ok((
        store,
        log_epoch_for(server, session_id),
        seq,
        revision,
        running,
    ))
}

pub(crate) fn handle_rows_range(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: RowsRangeParams = parse_params(V4_CONVERSATION_ROWS_RANGE_METHOD, params)?;
    let (store, log_epoch, seq, revision, _running) = projection_facts(server, &params.session_id)?;
    let limit = params.limit.as_u64().unwrap_or(0);
    if limit == 0 || limit > agent_protocol::v4_queries::ROWS_RANGE_MAX_LIMIT {
        return Err(ServerError::invalid_params(format!(
            "limit 必须是 1..={} 的整数",
            agent_protocol::v4_queries::ROWS_RANGE_MAX_LIMIT
        )));
    }
    let loaded = load_projected(&store, &params.session_id)?;
    let rows = rows_from_loaded(&loaded);
    eprintln!(
        "[dbg] rows_range loaded={} rows={}",
        loaded.len(),
        rows.len()
    );
    let before = params.before_row_id.as_ref().and_then(Number::as_u64);
    let eligible: Vec<&agent_protocol::v4_rows::ConversationRow> = rows
        .iter()
        .filter(|row| {
            let row_id = row_base(row).row_id.as_u64().unwrap_or(0);
            before.is_none_or(|before| row_id < before)
        })
        .collect();
    let skipped = eligible.len().saturating_sub(limit as usize);
    let window: Vec<_> = eligible.into_iter().skip(skipped).cloned().collect();
    let has_more = skipped > 0;
    serde_json::to_value(RowsRangeResult {
        rows: window,
        at_seq: Number::from(seq),
        at_revision: Number::from(revision),
        at_log_epoch: log_epoch,
        has_more,
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

fn row_base(row: &agent_protocol::v4_rows::ConversationRow) -> &agent_protocol::v4_rows::RowBase {
    match row {
        agent_protocol::v4_rows::ConversationRow::TurnHeader(row) => &row.base,
        agent_protocol::v4_rows::ConversationRow::UserInput(row) => &row.base,
        agent_protocol::v4_rows::ConversationRow::AssistantText(row) => &row.base,
        agent_protocol::v4_rows::ConversationRow::Reasoning(row) => &row.base,
        agent_protocol::v4_rows::ConversationRow::ToolCall(row) => &row.base,
        agent_protocol::v4_rows::ConversationRow::Artifact(row) => &row.base,
        agent_protocol::v4_rows::ConversationRow::Subagent(row) => &row.base,
        agent_protocol::v4_rows::ConversationRow::HookInvocation(row) => &row.base,
        agent_protocol::v4_rows::ConversationRow::TimelineMarker(row) => &row.base,
    }
}

pub(crate) fn handle_plans(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: PlansParams = parse_params(V4_CONVERSATION_PLANS_METHOD, params)?;
    let (store, log_epoch, seq, _revision, _running) =
        projection_facts(server, &params.session_id)?;
    let loaded = load_projected(&store, &params.session_id)?;
    let rows = rows_from_loaded(&loaded);
    // 终态 ExitPlanMode 的 toolCall 行（rowId 降序）；本波不产 Plan Mode 工具面。
    let plans = rows
        .into_iter()
        .filter_map(|row| match row {
            agent_protocol::v4_rows::ConversationRow::ToolCall(tool)
                if tool.tool_name == "ExitPlanMode" =>
            {
                Some(tool)
            }
            _ => None,
        })
        .rev()
        .collect();
    serde_json::to_value(PlansResult {
        plans,
        at_seq: Number::from(seq),
        at_log_epoch: log_epoch,
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_usage(server: &AgentServer, params: Value) -> Result<Value, ServerError> {
    let params: ConversationUsageParams = parse_params(V4_CONVERSATION_USAGE_METHOD, params)?;
    let db_path = db_path_of(server)?;
    let connection =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| ServerError::internal(error.to_string()))?;
    let mut statement = connection
        .prepare(
            "select coalesce(sum(computed_total_tokens),0), coalesce(sum(input_tokens),0), \
             coalesce(sum(output_tokens),0), coalesce(sum(reasoning_tokens),0), \
             coalesce(sum(cache_creation_input_tokens),0), coalesce(sum(cache_read_input_tokens),0), \
             coalesce(sum(model_request_count),0), \
             sum(case when status='error' then 1 else 0 end) \
             from turn_usage where session_id = ?1",
        )
        .map_err(|error| ServerError::internal(error.to_string()))?;
    let (total, input, output, reasoning, cache_creation, cache_read, requests, errors): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = statement
        .query_row([params.session_id.as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        })
        .unwrap_or((0, 0, 0, 0, 0, 0, 0, 0));
    let mut baseline_statement = connection
        .prepare(
            "select query_source, coalesce(sum(input_tokens),0) from model_usage \
             where session_id = ?1 group by query_source",
        )
        .map_err(|error| ServerError::internal(error.to_string()))?;
    let baselines: Vec<(String, i64)> = baseline_statement
        .query_map([params.session_id.as_str()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    serde_json::to_value(ConversationUsageResult {
        session_id: params.session_id,
        total_tokens: total.max(0) as u64,
        input_tokens: input.max(0) as u64,
        output_tokens: output.max(0) as u64,
        reasoning_tokens: reasoning.max(0) as u64,
        cache_creation_tokens: cache_creation.max(0) as u64,
        cache_read_tokens: cache_read.max(0) as u64,
        model_request_count: requests.max(0) as u64,
        model_error_count: errors.max(0) as u64,
        input_baseline_by_source: baselines
            .into_iter()
            .map(|(source, tokens)| (source, tokens.max(0) as u64))
            .collect(),
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_file_changes(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: FileChangesParams = parse_params(V4_CONVERSATION_FILE_CHANGES_METHOD, params)?;
    let current = log_epoch_for(server, &params.session_id);
    if params.base_log_epoch.trim() != current {
        return Err(ServerError::rpc(
            -32603,
            "proto.staleLogEpoch",
            Some(serde_json::json!({ "name": "Error" })),
        ));
    }
    let target_turn = turn_target(server, &params.session_id, &params.target)?;
    let store = server.store()?;
    let loaded = load_projected(&store, &params.session_id)?;
    let projected = projected_from_loaded(&loaded);
    serde_json::to_value(crate::v4_projection::file_changes_for_turn(
        &projected,
        &target_turn,
    ))
    .map_err(|error| ServerError::internal(error.to_string()))
}

/// 校验 `ConversationRowTarget` 并返回该行的 `turnId`。
///
/// 观察：fileChanges 的 target 必须是当前投影里的 turnHeader 行；否则
/// `-32603 proto.staleTarget`（黑盒探针 `probe-v4-target.mts`）。
fn turn_target(
    server: &AgentServer,
    session_id: &str,
    target: &agent_protocol::v4_queries::ConversationRowTarget,
) -> Result<String, ServerError> {
    let store = server.store()?;
    let loaded = load_projected(&store, session_id)?;
    let rows = rows_from_loaded(&loaded);
    let expected_entity = target.entity_id.as_str();
    for row in rows {
        if let agent_protocol::v4_rows::ConversationRow::TurnHeader(header) = row {
            if header.base.row_id.as_u64() == Some(target.row_id)
                && header.base.entity_id.as_deref() == Some(expected_entity)
            {
                return Ok(header.base.turn_id);
            }
        }
    }
    Err(ServerError::rpc(
        -32603,
        "proto.staleTarget",
        Some(serde_json::json!({ "name": "Error" })),
    ))
}

pub(crate) fn handle_file_rewind_preview(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: FileRewindPreviewParams = parse_params(
        crate::methods::V4_CONVERSATION_FILE_REWIND_PREVIEW_METHOD,
        params,
    )?;
    let current = log_epoch_for(server, &params.session_id);
    if params.base_log_epoch.trim() != current {
        return Err(ServerError::rpc(
            -32603,
            "proto.staleLogEpoch",
            Some(serde_json::json!({ "name": "Error" })),
        ));
    }
    let target_turn = turn_target(server, &params.session_id, &params.target)?;
    let store = server.store()?;
    let loaded = load_projected(&store, &params.session_id)?;
    let projected = projected_from_loaded(&loaded);
    let changes = crate::v4_projection::file_changes_for_turn(&projected, &target_turn);
    let mut safe_files = Vec::new();
    let mut unsafe_files = Vec::new();
    for item in changes.items {
        // 新建文件的唯一 Write 可安全回退为删除；其余文件没有基线快照，标记
        // `checkpoint_missing`（本波不落 git checkpoint，登记差异）。
        let created_only =
            item.write_count == 1 && item.tool_names.as_slice() == ["Write"] && item.deletions == 0;
        if created_only {
            safe_files.push(RewindSafeFile {
                action: RewindAction::Delete,
                operation_count: item.write_count,
                path: item.path,
                tool_names: item.tool_names,
            });
        } else {
            unsafe_files.push(RewindUnsafeFile {
                current_hash: None,
                expected_hash: None,
                message: Some("No checkpoint is available for this file".to_string()),
                operation_count: item.write_count,
                path: item.path,
                reason: RewindUnsafeReason::CheckpointMissing,
                tool_names: item.tool_names,
            });
        }
    }
    let can_apply = unsafe_files.is_empty() && !safe_files.is_empty();
    serde_json::to_value(FileRewindPreviewResult {
        can_apply,
        ignored_files: Vec::new(),
        safe_files,
        unsafe_files,
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_background_bash_output(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: BackgroundBashOutputParams =
        parse_params(V4_BACKGROUND_BASH_OUTPUT_METHOD, params)?;
    // existing-only：只读活跃 runtime 的 TaskRegistry，不拉起进程。
    let snapshot = server
        .find_runtime(&params.session_id)
        .and_then(|runtime| runtime.background_task_snapshot(&params.work_id));
    let Some(snapshot) = snapshot else {
        return serde_json::to_value(BackgroundBashOutputResult::Unavailable(
            BackgroundBashUnavailable {
                kind: BackgroundBashUnavailableKind::Unavailable,
                work_id: params.work_id,
                code: None,
            },
        ))
        .map_err(|error| ServerError::internal(error.to_string()));
    };
    let status = match snapshot.status {
        agent_core::TaskStatus::Running => BackgroundBashStatus::Running,
        agent_core::TaskStatus::Completed => BackgroundBashStatus::Completed,
        agent_core::TaskStatus::Failed => BackgroundBashStatus::Failed,
        agent_core::TaskStatus::Cancelled => BackgroundBashStatus::Cancelled,
    };
    serde_json::to_value(BackgroundBashOutputResult::Output(BackgroundBashOutput {
        kind: BackgroundBashOutputKind::Output,
        work_id: params.work_id,
        status,
        output: snapshot.output,
        truncated: false,
        output_path: snapshot.output_path.display().to_string(),
    }))
    .map_err(|error| ServerError::internal(error.to_string()))
}

// ── usageStats ──────────────────────────────────────────────────────────────

fn day_of(ms: i64) -> String {
    data_root::iso8601_utc(ms)
        .split('T')
        .next()
        .unwrap_or("")
        .to_string()
}

pub(crate) fn handle_usage_stats(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: UsageStatsParams = parse_params(V4_USAGE_STATS_METHOD, params)?;
    let db_path = db_path_of(server)?;
    let connection =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| ServerError::internal(error.to_string()))?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0);
    let range_days: i64 = match params.range {
        AppUsageRange::SevenDays => 7,
        AppUsageRange::ThirtyDays => 30,
        AppUsageRange::All => i64::MAX / 2,
    };
    let since = if range_days > now_ms / 86_400_000 {
        0
    } else {
        now_ms - range_days * 86_400_000
    };
    // model_usage 聚合。
    let mut model_statement = connection
        .prepare(
            "select model_id, coalesce(sum(computed_total_tokens),0), coalesce(sum(input_tokens),0), \
             coalesce(sum(output_tokens),0), count(*), started_at from model_usage \
             where started_at >= ?1 group by model_id",
        )
        .map_err(|error| ServerError::internal(error.to_string()))?;
    let model_rows: Vec<(Option<String>, i64, i64, i64, i64, i64)> = model_statement
        .query_map([since], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    let total_model_tokens: i64 = model_rows.iter().map(|(_, tokens, ..)| *tokens).sum();
    let mut turn_statement = connection
        .prepare(
            "select coalesce(sum(computed_total_tokens),0), count(*), \
             sum(case when status='error' then 1 else 0 end), coalesce(avg(duration_ms),0), \
             coalesce(max(duration_ms),0), session_id, started_at from turn_usage \
             where started_at >= ?1",
        )
        .map_err(|error| ServerError::internal(error.to_string()))?;
    let turn_rows: Vec<(i64, i64, i64, f64, i64, String, i64)> = turn_statement
        .query_map([since], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    let mut tool_statement = connection
        .prepare(
            "select tool_name, count(*), sum(case when status='error' then 1 else 0 end), \
             coalesce(avg(duration_ms),0) from tool_usage where started_at >= ?1 group by tool_name",
        )
        .map_err(|error| ServerError::internal(error.to_string()))?;
    let tool_rows: Vec<(String, i64, i64, f64)> = tool_statement
        .query_map([since], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    let total_sessions = turn_rows
        .iter()
        .map(|(_, _, _, _, _, session_id, _)| session_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .len() as i64;
    let total_turns: i64 = turn_rows.iter().map(|(_, count, ..)| *count).sum();
    let tool_calls: i64 = tool_rows.iter().map(|(_, calls, _, _)| *calls).sum();
    let tool_errors: i64 = tool_rows.iter().map(|(_, _, errors, _)| *errors).sum();
    let _model_requests: i64 = model_rows.iter().map(|(.., count, _)| *count).sum();
    let model_errors = 0i64;
    let input_tokens: i64 = model_rows.iter().map(|(_, _, input, ..)| *input).sum();
    let output_tokens: i64 = model_rows.iter().map(|(_, _, _, output, ..)| *output).sum();
    // heatmap：按日聚合（model_usage），7/30 天从窗口末尾往前排。
    let mut by_day: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for (.., started_at) in &model_rows {
        let day = day_of(*started_at);
        let entry = by_day.entry(day).or_insert((0, 0));
        entry.0 += 0;
        entry.1 += 0;
    }
    let mut turn_by_day: BTreeMap<String, (i64, i64, i64)> = BTreeMap::new();
    for (_, _, _, _, _, _, started_at) in &turn_rows {
        let day = day_of(*started_at);
        let entry = turn_by_day.entry(day).or_insert((0, 0, 0));
        entry.1 += 1;
    }
    let (start_date, end_date) = if params.range == AppUsageRange::All {
        (None, None)
    } else {
        (Some(day_of(since)), Some(day_of(now_ms)))
    };
    let mut weeks: Vec<AppUsageHeatmapWeek> = Vec::new();
    if let (Some(start), Some(end)) = (&start_date, &end_date) {
        // 简化：按出现过的日期逐日给 cell（协议允许缺失日为 null）。
        let mut week_index = 0i64;
        let mut buffer: Vec<Option<AppUsageHeatmapCell>> = Vec::new();
        for day in &turn_by_day {
            if day.0.as_str() < start.as_str() || day.0.as_str() > end.as_str() {
                continue;
            }
            if buffer.len() == 7 {
                weeks.push(AppUsageHeatmapWeek {
                    week_index: Number::from(week_index),
                    days: std::mem::take(&mut buffer),
                });
                week_index += 1;
            }
            buffer.push(Some(AppUsageHeatmapCell {
                date: day.0.clone(),
                level: 1,
                total_tokens: Number::from(0i64),
                turn_count: Number::from(day.1 .1),
                tool_call_count: Number::from(0i64),
            }));
        }
        if !buffer.is_empty() {
            weeks.push(AppUsageHeatmapWeek {
                week_index: Number::from(week_index),
                days: buffer,
            });
        }
    }
    let days_active = turn_by_day.len() as i64;
    let peak_day = turn_by_day
        .values()
        .map(|(tokens, _, _)| *tokens)
        .max()
        .unwrap_or(0);
    let favorite =
        model_rows
            .iter()
            .max_by_key(|(_, tokens, ..)| *tokens)
            .map(|(model_id, tokens, ..)| AppUsageFavoriteModel {
                model_id: model_id.clone(),
                total_tokens: Number::from(*tokens),
                share: Number::from(if total_model_tokens > 0 {
                    ((*tokens as f64) / (total_model_tokens as f64) * 1000.0).round() as i64
                } else {
                    0
                }),
            });
    let generated_at = Number::from(now_ms);
    let summary = AppUsageSummary {
        total_tokens: Number::from(total_model_tokens),
        input_tokens: Number::from(input_tokens),
        output_tokens: Number::from(output_tokens),
        reasoning_tokens: Number::from(0i64),
        cache_creation_tokens: Number::from(0i64),
        cache_read_tokens: Number::from(0i64),
        cache_hit_rate: Number::from(0i64),
        total_sessions: Number::from(total_sessions),
        total_turns: Number::from(total_turns),
        tool_call_count: Number::from(tool_calls),
        tool_error_rate: Number::from(if tool_calls > 0 {
            tool_errors * 1000 / tool_calls
        } else {
            0
        }),
        model_error_rate: Number::from(model_errors),
        avg_time_to_first_token_ms: None,
        avg_turn_duration_ms: turn_rows
            .first()
            .map(|(_, _, _, avg, ..)| Number::from(*avg as i64)),
        active_days: Number::from(days_active),
        current_streak_days: Number::from(days_active.min(1)),
        longest_session_ms: Number::from(
            turn_rows
                .iter()
                .map(|(_, _, _, _, longest, _, _)| *longest)
                .max()
                .unwrap_or(0),
        ),
        longest_streak_days: Number::from(days_active.min(1)),
        peak_day_tokens: Number::from(peak_day),
        favorite_model: favorite,
    };
    serde_json::to_value(agent_protocol::v4_queries::AppUsageSnapshot {
        range: params.range,
        generated_at,
        time_zone: params
            .time_zone
            .clone()
            .unwrap_or_else(|| "UTC".to_string()),
        source: AppUsageSource::AgentDb,
        summary,
        heatmap: AppUsageHeatmap {
            start_date,
            end_date,
            max_tokens: Number::from(peak_day),
            weeks,
        },
        daily_model_usage: model_rows
            .iter()
            .map(
                |(model_id, tokens, .., started_at)| AppUsageDailyModelUsage {
                    date: day_of(*started_at),
                    models: vec![AppUsageDailyModelItem {
                        model_id: model_id.clone(),
                        total_tokens: Number::from(*tokens),
                    }],
                },
            )
            .collect(),
        models: model_rows
            .iter()
            .map(
                |(model_id, tokens, input, output, count, _)| AppUsageModelUsage {
                    model_id: model_id.clone(),
                    total_tokens: Number::from(*tokens),
                    input_tokens: Number::from(*input),
                    output_tokens: Number::from(*output),
                    request_count: Number::from(*count),
                    share: Number::from(if total_model_tokens > 0 {
                        ((*tokens as f64) / (total_model_tokens as f64) * 1000.0).round() as i64
                    } else {
                        0
                    }),
                },
            )
            .collect(),
        tools: tool_rows
            .iter()
            .map(
                |(tool_name, calls, errors, avg_duration)| AppUsageToolUsage {
                    tool_name: tool_name.clone(),
                    call_count: Number::from(*calls),
                    error_count: Number::from(*errors),
                    error_rate: Number::from(if *calls > 0 { errors * 1000 / calls } else { 0 }),
                    avg_duration_ms: Some(Number::from(*avg_duration as i64)),
                },
            )
            .collect(),
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

// ── workflow run 查询族 ─────────────────────────────────────────────────────

fn open_read_db(server: &AgentServer) -> Result<rusqlite::Connection, ServerError> {
    rusqlite::Connection::open_with_flags(
        db_path_of(server)?,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_workflow_runs(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: WorkflowRunsQueryParams =
        parse_params(V4_CONVERSATION_WORKFLOW_RUNS_METHOD, params)?;
    let connection = open_read_db(server)?;
    let mut statement = connection
        .prepare(
            "select id, tool_call_id, name, time_updated, status, resumed_from, failure_json \
             from dwf_run where parent_session_id = ?1 order by time_updated desc",
        )
        .map_err(|error| ServerError::internal(error.to_string()))?;
    let rows: Vec<WorkflowRunRowTuple> = statement
        .query_map([params.session_id.as_str()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .map(|rows| rows.flatten().collect())
        .map_err(|error| ServerError::internal(error.to_string()))?;
    let runs = rows
        .into_iter()
        .take(params.limit.unwrap_or(20).clamp(1, 64) as usize)
        .map(
            |(run_id, tool_call_id, label, updated_at, status, resumed_from, failure_json)| {
                let failure: Option<Value> =
                    failure_json.and_then(|raw| serde_json::from_str(&raw).ok());
                WorkflowRunSummary {
                    run_id,
                    tool_call_id,
                    label,
                    updated_at: Some(updated_at.max(0) as u64),
                    status: workflow_status(&status),
                    stop_reason: None,
                    resumed_from,
                    superseded_by: None,
                    failure_code: failure
                        .as_ref()
                        .and_then(|value| value.get("code"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    failure_message: failure
                        .as_ref()
                        .and_then(|value| value.get("message"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    resumable: false,
                }
            },
        )
        .collect();
    serde_json::to_value(WorkflowRunsQueryResult { runs })
        .map_err(|error| ServerError::internal(error.to_string()))
}

fn workflow_status(status: &str) -> agent_protocol::v4_workflow_runs::WorkflowRunStatus {
    match status {
        "pending" => agent_protocol::v4_workflow_runs::WorkflowRunStatus::Pending,
        "running" => agent_protocol::v4_workflow_runs::WorkflowRunStatus::Running,
        "failed" => agent_protocol::v4_workflow_runs::WorkflowRunStatus::Errored,
        "stopped" => agent_protocol::v4_workflow_runs::WorkflowRunStatus::Stopped,
        _ => agent_protocol::v4_workflow_runs::WorkflowRunStatus::Completed,
    }
}

pub(crate) fn handle_workflow_events(
    server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: WorkflowRunEventsParams =
        parse_params(V4_CONVERSATION_WORKFLOW_RUN_EVENTS_METHOD, params)?;
    let connection = open_read_db(server)?;
    let mut statement = connection
        .prepare(
            "select sequence, type, payload_json from dwf_event where run_id = ?1 \
             and sequence > ?2 order by sequence limit ?3",
        )
        .map_err(|error| ServerError::internal(error.to_string()))?;
    // `afterSequence` 是排他游标，缺省表示「从头取」——`dwf_event.sequence` 从 0 开始，
    // 不能用 0 当缺省（会吞掉 run-started）。
    let after = params
        .after_sequence
        .map(|value| value as i64)
        .unwrap_or(-1);
    let limit = params.limit.unwrap_or(500).clamp(1, 500) as i64;
    let rows: Vec<(i64, String, String)> = statement
        .query_map(rusqlite::params![params.run_id, after, limit + 1], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map(|rows| rows.flatten().collect())
        .map_err(|error| ServerError::internal(error.to_string()))?;
    let has_more = rows.len() > limit as usize;
    let events: Vec<_> = rows
        .into_iter()
        .take(limit as usize)
        .map(
            |(sequence, event_type, payload_json)| agent_protocol::v4_queries::WorkflowRunEvent {
                sequence: sequence.max(0) as u64,
                event_type,
                payload: serde_json::from_str(&payload_json).unwrap_or_default(),
                truncated: None,
            },
        )
        .collect();
    serde_json::to_value(WorkflowRunEventsResult { events, has_more })
        .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_workflow_artifacts(
    _server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: agent_protocol::v4_queries::WorkflowRunArtifactsParams = parse_params(
        agent_protocol::v4_queries::V4_CONVERSATION_WORKFLOW_RUN_ARTIFACTS_METHOD,
        params,
    )?;
    let _ = (params.session_id, params.run_id);
    // 本波执行器不产 artifact（脚本顶层返回值走 run.result_json）；登记为空清单。
    serde_json::to_value(agent_protocol::v4_queries::WorkflowRunArtifactsResult {
        artifacts: Vec::new(),
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_workflow_workspace(
    _server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: agent_protocol::v4_queries::WorkflowRunWorkspaceParams = parse_params(
        agent_protocol::v4_queries::V4_CONVERSATION_WORKFLOW_RUN_WORKSPACE_METHOD,
        params,
    )?;
    let _ = (params.session_id, params.run_id);
    serde_json::to_value(agent_protocol::v4_queries::WorkflowRunWorkspaceResult {
        nodes: Vec::new(),
        truncated: None,
    })
    .map_err(|error| ServerError::internal(error.to_string()))
}

pub(crate) fn handle_workflow_node_result(
    _server: &AgentServer,
    params: Value,
) -> Result<Value, ServerError> {
    let params: agent_protocol::v4_queries::WorkflowRunNodeResultParams = parse_params(
        agent_protocol::v4_queries::V4_CONVERSATION_WORKFLOW_RUN_NODE_RESULT_METHOD,
        params,
    )?;
    let _ = (
        params.session_id,
        params.run_id,
        &params.site_id,
        params.ordinal,
    );
    Err(ServerError::rpc(
        -32603,
        "fault.workflowRun.nodeNotFound",
        Some(serde_json::json!({ "name": "Error" })),
    ))
}
