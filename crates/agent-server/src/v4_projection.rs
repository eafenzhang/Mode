//! v4 快照/行投影（C4 T4c）：DB 与事件 → `ConversationSnapshot` / `ConversationDelta`。
//!
//! 行 id 口径：session 内单调、按投影顺序分配（快照 = DB 顺序 1..N；live 从快照尾部续）。
//! `createdAtSeq` 用 message/part 的 `sequence` 近似（oracle 是事件 seq，登记差异）。
//! `turnId` 用该 turn 的 user message id（oracle 是 turn.started 的 messageId，同源）。

use std::collections::BTreeMap;

use agent_core::SessionEvent;
use agent_protocol::session_event::{
    ModelStreamingKind, SessionEventType, ToolUpdatedPayload, TurnCompletedPayload, TurnResultType,
};
use agent_protocol::v4_delta::{
    ConversationDelta, RowAppendedDelta, RowDeltaDelta, RowUpsertedDelta, StatePatch,
    StateUpdatedDelta, StreamablePath,
};
use agent_protocol::v4_queries::{FileChangeItem, FileChangesResult, FileDiffHunk};
use agent_protocol::v4_rows::{
    AssistantTextKind, AssistantTextRow, AssistantTextState, ConversationRow, RowBase,
    ToolCallKind, ToolCallRow, ToolCallStatus, ToolOutput, TurnFileChangeState, TurnFileChanges,
    TurnHeaderKind, TurnHeaderRow, TurnOrigin, TurnState, UserInputKind, UserInputOrigin,
    UserInputRow,
};
use agent_protocol::v4_snapshot::{
    ActionAvailability, ContextWindowUsage, CumulativeUsage, DeniedAction, GoalState, GoalStatus,
    InputRouting, InputRoutingMode, PlanState, QueueState, RowsWindow, SessionActionAvailability,
    SessionControl, SessionMetaState, SessionPhase, SessionUsageState, StopState, StopTargetKind,
    SubagentProjectionState, TitleSource,
};
use agent_protocol::workflow::LiteralFalse;
use agent_protocol::workflow::LiteralTrue;
use serde_json::{Number, Value};

/// DB message/part 投影输入（`session-store::MessageWithParts` 的只读视图）。
pub struct ProjectedMessage<'a> {
    pub id: &'a str,
    pub role: &'a str,
    pub sequence: Option<i64>,
    pub time_created: i64,
    pub data: &'a Value,
    pub parts: &'a [(String, Option<i64>, i64, Value)],
}

/// DB → v4 行投影（快照/rowsRange 共用）。
///
/// 只产观察到的事实行：turnHeader / userInput / assistantText / toolCall；
/// reasoning / artifact / hook / timeline marker 未接（登记）。
pub fn project_rows(messages: &[ProjectedMessage<'_>]) -> Vec<ConversationRow> {
    let mut rows = Vec::new();
    // 行 id 按 session 内投影顺序单调分配（1 起）；`row_id` 的稳定性见模块注释。
    struct RowIdGen(u64);
    impl RowIdGen {
        fn base(
            &mut self,
            turn_id: String,
            entity_id: String,
            created_at: i64,
            created_at_seq: i64,
        ) -> RowBase {
            self.0 += 1;
            RowBase {
                row_id: Number::from(self.0),
                turn_id,
                entity_id: Some(entity_id),
                product_turn_id: None,
                visibility: Some(agent_protocol::v4_rows::RowVisibility::Visible),
                created_at: Number::from(created_at),
                created_at_seq: Number::from(created_at_seq),
                actions: Some(agent_protocol::v4_rows::RowActions::default()),
            }
        }
    }
    let mut assign = RowIdGen(0);
    for message in messages {
        let semantics = message
            .data
            .get("semantics")
            .and_then(Value::as_object)
            .cloned();
        let role = message.role;
        if role == "user" {
            let turn_id = message.id.to_string();
            rows.push(ConversationRow::TurnHeader(TurnHeaderRow {
                base: assign.base(
                    turn_id.clone(),
                    message.id.to_string(),
                    message.time_created,
                    message.sequence.unwrap_or(0),
                ),
                kind: TurnHeaderKind::TurnHeader,
                origin: turn_origin(semantics.as_ref()),
                execution_kind: None,
                source_command_id: None,
                history_round_count: None,
                state: TurnState::CompletedSuccess,
                started_at: Number::from(message.time_created),
                ended_at: Some(Number::from(last_time(message.parts))),
                active_ms: None,
                work_segments: None,
                origin_meta: None,
                workflow_launch: None,
                file_changes: None,
            }));
            // 用户正文在 text part 里（message.data 只带 role/semantics 等元数据）。
            let text = message
                .parts
                .iter()
                .filter(|(_, _, _, data)| data.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|(_, _, _, data)| data.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            rows.push(ConversationRow::UserInput(UserInputRow {
                base: assign.base(
                    turn_id.clone(),
                    message.id.to_string(),
                    message.time_created,
                    message.sequence.unwrap_or(0),
                ),
                kind: UserInputKind::UserInput,
                text,
                epilogue_start: None,
                origin: input_origin(semantics.as_ref()),
                origin_meta: None,
                workflow_launch: None,
                guided: None,
                source_command_id: None,
                root_source_command_id: None,
                client_id: None,
                attachments: None,
            }));
            continue;
        }
        for (part_id, part_sequence, part_time, part_data) in message.parts {
            match part_data.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = part_data
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    rows.push(ConversationRow::AssistantText(AssistantTextRow {
                        base: assign.base(
                            turn_id_of(messages, message.id),
                            part_id.clone(),
                            *part_time,
                            part_sequence.unwrap_or(0),
                        ),
                        kind: AssistantTextKind::AssistantText,
                        assistant_response_id: Some(message.id.to_string()),
                        text,
                        state: AssistantTextState::Complete,
                        model: None,
                        feedback: None,
                    }));
                }
                Some("tool") => {
                    let state = part_data.get("state").cloned().unwrap_or(Value::Null);
                    let status = match state.get("status").and_then(Value::as_str) {
                        Some("running") | Some("pending") => ToolCallStatus::Running,
                        Some("error") => ToolCallStatus::Error,
                        Some("cancelled") | Some("canceled") => ToolCallStatus::Cancelled,
                        _ => ToolCallStatus::Success,
                    };
                    let output =
                        state
                            .get("output")
                            .and_then(Value::as_str)
                            .map(|text| ToolOutput {
                                text: text.to_string(),
                                display: state.get("display").cloned(),
                                truncated: None,
                            });
                    rows.push(ConversationRow::ToolCall(ToolCallRow {
                        base: assign.base(
                            turn_id_of(messages, message.id),
                            part_id.clone(),
                            *part_time,
                            part_sequence.unwrap_or(0),
                        ),
                        kind: ToolCallKind::ToolCall,
                        assistant_response_id: Some(message.id.to_string()),
                        tool_call_id: state
                            .get("callID")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        tool_name: part_data
                            .get("tool")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        status,
                        input_text: state
                            .get("input")
                            .map(|input| input.to_string())
                            .unwrap_or_default(),
                        input: state.get("input").cloned(),
                        cua_app: None,
                        output,
                        display: None,
                        error: None,
                        progress: None,
                        output_preview: None,
                        approval_interaction_id: None,
                        backgrounded: None,
                        work_id: None,
                        started_at: Some(Number::from(*part_time)),
                        ended_at: Some(Number::from(*part_time)),
                    }));
                }
                _ => {}
            }
        }
    }
    // C4 T6：turnHeader 的 fileChanges 摘要（oracle 观察到 `{additions,deletions,files,state}`）。
    for row in &mut rows {
        if let ConversationRow::TurnHeader(header) = row {
            let changes = file_changes_for_turn(messages, &header.base.turn_id);
            if changes.files > 0 {
                header.file_changes = Some(TurnFileChanges {
                    additions: Number::from(changes.additions),
                    deletions: Number::from(changes.deletions),
                    files: Number::from(changes.files),
                    state: Some(TurnFileChangeState::Active),
                });
            }
        }
    }
    rows
}

/// 一个 turn 内 Write/Edit 的文件变更投影（`fileChanges` 查询与 turnHeader 摘要共用）。
///
/// hunk 行号是近似值（DB 没有基线快照；登记差异）；新增/删除行数按工具入参文本行数计。
pub fn file_changes_for_turn(
    messages: &[ProjectedMessage<'_>],
    turn_id: &str,
) -> FileChangesResult {
    struct Acc {
        path: String,
        additions: u64,
        deletions: u64,
        write_count: u64,
        tool_names: Vec<String>,
        patches: Vec<FileDiffHunk>,
    }
    // 行上的 `turnId` 是产生该 turn 的 user message id（模块注释）；工具 part 的
    // `anchor.turnId` 是运行时 turn id，先用 user 行的 anchor 做一次映射。
    let anchor_turn = messages
        .iter()
        .find(|message| message.id == turn_id)
        .and_then(|message| message.data.get("anchor"))
        .and_then(|anchor| anchor.get("turnId"))
        .and_then(Value::as_str)
        .unwrap_or(turn_id);
    let mut order: Vec<String> = Vec::new();
    let mut by_path: BTreeMap<String, Acc> = BTreeMap::new();
    for message in messages {
        if message
            .data
            .get("anchor")
            .and_then(|anchor| anchor.get("turnId"))
            .and_then(Value::as_str)
            != Some(anchor_turn)
        {
            continue;
        }
        for (_part_id, _sequence, _time, data) in message.parts {
            if data.get("type").and_then(Value::as_str) != Some("tool") {
                continue;
            }
            let tool = data.get("tool").and_then(Value::as_str).unwrap_or_default();
            if tool != "Write" && tool != "Edit" {
                continue;
            }
            let Some(state) = data.get("state") else {
                continue;
            };
            // 只有成功落盘的工具调用产生文件变更；running/error/cancelled 不计。
            if state
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status != "completed")
            {
                continue;
            }
            let input = state.get("input").cloned().unwrap_or(Value::Null);
            let Some(path) = input.get("file_path").and_then(Value::as_str) else {
                continue;
            };
            let entry = by_path.entry(path.to_string()).or_insert_with(|| {
                order.push(path.to_string());
                Acc {
                    path: path.to_string(),
                    additions: 0,
                    deletions: 0,
                    write_count: 0,
                    tool_names: Vec::new(),
                    patches: Vec::new(),
                }
            });
            entry.write_count += 1;
            if !entry.tool_names.iter().any(|name| name == tool) {
                entry.tool_names.push(tool.to_string());
            }
            if tool == "Write" {
                let content = input
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let lines: Vec<&str> = content.lines().collect();
                entry.additions += lines.len() as u64;
                if !lines.is_empty() {
                    entry.patches.push(FileDiffHunk {
                        old_start: Number::from(1),
                        old_lines: Number::from(0),
                        new_start: Number::from(1),
                        new_lines: Number::from(lines.len()),
                        lines: lines.iter().map(|line| format!("+{line}")).collect(),
                    });
                }
            } else {
                let old = input
                    .get("old_string")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let new = input
                    .get("new_string")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let old_lines: Vec<&str> = old.lines().collect();
                let new_lines: Vec<&str> = new.lines().collect();
                entry.additions += new_lines.len() as u64;
                entry.deletions += old_lines.len() as u64;
                if !old_lines.is_empty() || !new_lines.is_empty() {
                    let mut lines: Vec<String> =
                        old_lines.iter().map(|line| format!("-{line}")).collect();
                    lines.extend(new_lines.iter().map(|line| format!("+{line}")));
                    entry.patches.push(FileDiffHunk {
                        old_start: Number::from(1),
                        old_lines: Number::from(old_lines.len()),
                        new_start: Number::from(1),
                        new_lines: Number::from(new_lines.len()),
                        lines,
                    });
                }
            }
        }
    }
    let items: Vec<FileChangeItem> = order
        .into_iter()
        .filter_map(|path| by_path.remove(&path))
        .map(|acc| FileChangeItem {
            path: acc.path,
            additions: acc.additions,
            deletions: acc.deletions,
            write_count: acc.write_count,
            tool_names: acc.tool_names,
            patches: acc.patches,
        })
        .collect();
    FileChangesResult {
        files: items.len() as u64,
        additions: items.iter().map(|item| item.additions).sum(),
        deletions: items.iter().map(|item| item.deletions).sum(),
        state: None,
        items,
    }
}

fn turn_id_of<'a>(messages: &'a [ProjectedMessage<'a>], message_id: &str) -> String {
    // assistant 行的 turnId 是产生它的 user message id：取它**前面最近**的一条
    // user 行（不能取第一条，否则整段历史都会被并进第一个 turn）。
    let mut current = message_id.to_string();
    for message in messages {
        if message.id == message_id {
            return current;
        }
        if message.role == "user" {
            current = message.id.to_string();
        }
    }
    message_id.to_string()
}

fn last_time(parts: &[(String, Option<i64>, i64, Value)]) -> i64 {
    parts
        .last()
        .map(|(_, _, time, _)| *time)
        .unwrap_or_default()
}

fn turn_origin(semantics: Option<&serde_json::Map<String, Value>>) -> TurnOrigin {
    match semantics
        .and_then(|map| map.get("kind"))
        .and_then(Value::as_str)
    {
        Some("background_notification") => TurnOrigin::BackgroundResult,
        Some("goal_continuation") => TurnOrigin::GoalContinuation,
        _ => TurnOrigin::UserInput,
    }
}

fn input_origin(semantics: Option<&serde_json::Map<String, Value>>) -> UserInputOrigin {
    match semantics
        .and_then(|map| map.get("kind"))
        .and_then(Value::as_str)
    {
        Some("background_notification") => UserInputOrigin::BackgroundResult,
        Some("goal_continuation") => UserInputOrigin::GoalContinuation,
        _ => UserInputOrigin::RealUser,
    }
}

/// 快照 state 部分（rows 之外）的输入。
pub struct SnapshotStateFacts<'a> {
    pub session_id: &'a str,
    pub log_epoch: &'a str,
    pub seq: u64,
    pub revision: u64,
    /// 快照 `queue.items`（待执行输入，admission 顺序）。
    pub queue_items: Vec<agent_protocol::v4_snapshot::QueueItem>,
    /// 快照 `queue.autoDrain`（`setAutoDrain`）。
    pub auto_drain: bool,
    /// 快照 `config.followupMode`（`setFollowupMode`）。
    pub followup_mode: agent_protocol::v4_snapshot::FollowupMode,
    pub title: &'a str,
    pub title_source: &'a str,
    pub mode: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub running: bool,
    pub goal: Option<GoalState>,
    pub subagents: SubagentProjectionState,
    pub workflow_runs: Option<agent_protocol::v4_workflow_runs::WorkflowRunsState>,
    pub rows: Vec<ConversationRow>,
    /// 冷会话没有 contextWindow 事实（oracle 暖会话有；登记）。
    pub context_window: Option<ContextWindowUsage>,
}

/// 组装完整快照（21 键全量；per-key 的登记差异见契约 §5.9）。
#[allow(clippy::too_many_arguments)]
pub fn build_snapshot(
    facts: SnapshotStateFacts<'_>,
) -> agent_protocol::v4_snapshot::ConversationSnapshot {
    let running = facts.running;
    let phase = if running {
        SessionPhase::Running
    } else {
        SessionPhase::Draft
    };
    let goal_status = facts.goal.as_ref().map(|goal| goal.status);
    let total_rows = facts.rows.len() as u64;
    agent_protocol::v4_snapshot::ConversationSnapshot {
        protocol_version: 1,
        session_id: facts.session_id.to_string(),
        log_epoch: facts.log_epoch.to_string(),
        seq: Number::from(facts.seq),
        revision: Number::from(facts.revision),
        control: SessionControl {
            phase,
            session_ended: false,
            can_stop: running,
            stop_state: if running {
                StopState::Stoppable
            } else {
                StopState::Idle
            },
            stop_target_kind: if running {
                StopTargetKind::Assistant
            } else {
                StopTargetKind::Unknown
            },
            active_works: Vec::new(),
            last_error: None,
            api_retry: None,
        },
        availability: SessionActionAvailability {
            fork: ActionAvailability::Denied(DeniedAction {
                allowed: LiteralFalse,
                reason_code: "forkTargetNotStable".to_string(),
            }),
            compact: ActionAvailability::Denied(DeniedAction {
                allowed: LiteralFalse,
                reason_code: "idleCannotCompact".to_string(),
            }),
            switch_model_config: ActionAvailability::Allowed(
                agent_protocol::v4_snapshot::AllowedAction {
                    allowed: LiteralTrue,
                },
            ),
            set_followup_mode: ActionAvailability::Allowed(
                agent_protocol::v4_snapshot::AllowedAction {
                    allowed: LiteralTrue,
                },
            ),
            queue_edit: ActionAvailability::Allowed(agent_protocol::v4_snapshot::AllowedAction {
                allowed: LiteralTrue,
            }),
            send_queued_now: if running {
                ActionAvailability::Allowed(agent_protocol::v4_snapshot::AllowedAction {
                    allowed: LiteralTrue,
                })
            } else {
                ActionAvailability::Denied(DeniedAction {
                    allowed: LiteralFalse,
                    reason_code: "sendQueuedNowRequiresRunning".to_string(),
                })
            },
            pause_goal: goal_status
                .map(|status| match status {
                    GoalStatus::Active | GoalStatus::Verifying => {
                        ActionAvailability::Allowed(agent_protocol::v4_snapshot::AllowedAction {
                            allowed: LiteralTrue,
                        })
                    }
                    _ => ActionAvailability::Denied(DeniedAction {
                        allowed: LiteralFalse,
                        reason_code: "noGoalToPause".to_string(),
                    }),
                })
                .unwrap_or_else(|| {
                    ActionAvailability::Denied(DeniedAction {
                        allowed: LiteralFalse,
                        reason_code: "noGoalToPause".to_string(),
                    })
                }),
            resume_goal: goal_status
                .map(|status| match status {
                    GoalStatus::Paused => {
                        ActionAvailability::Allowed(agent_protocol::v4_snapshot::AllowedAction {
                            allowed: LiteralTrue,
                        })
                    }
                    _ => ActionAvailability::Denied(DeniedAction {
                        allowed: LiteralFalse,
                        reason_code: "noGoalToResume".to_string(),
                    }),
                })
                .unwrap_or_else(|| {
                    ActionAvailability::Denied(DeniedAction {
                        allowed: LiteralFalse,
                        reason_code: "noGoalToResume".to_string(),
                    })
                }),
        },
        input_routing: InputRouting {
            mode: if running {
                InputRoutingMode::Enqueue
            } else {
                InputRoutingMode::StartNow
            },
            reason_code: None,
        },
        meta: SessionMetaState {
            title: facts.title.to_string(),
            title_source: match facts.title_source {
                "generated" => TitleSource::Generated,
                "custom" => TitleSource::Custom,
                _ => TitleSource::Default,
            },
        },
        shared_context_import: None,
        config: agent_protocol::v4_snapshot::SessionConfigState {
            model_selection: None,
            provider: facts.provider.to_string(),
            model: facts.model.to_string(),
            thought: String::new(),
            thought_levels: Vec::new(),
            followup_mode: facts.followup_mode,
            mode: facts.mode.to_string(),
            plan_enabled: None,
            permission_grant: None,
            plan_transition: None,
        },
        // 快照 21 键里 modelTransition / workspaceHookAdmission 键必在（null 表示无迁移）。
        model_transition: Some(None),
        usage: SessionUsageState {
            context_window: facts.context_window,
            cumulative: CumulativeUsage {
                input_tokens: Number::from(0u64),
                output_tokens: Number::from(0u64),
                cache_read_tokens: Number::from(0u64),
                cache_write_tokens: Number::from(0u64),
            },
        },
        queue: QueueState {
            items: facts.queue_items,
            auto_drain: facts.auto_drain,
            pause_reason: None,
        },
        pending_interactions: Vec::new(),
        pending_commands: Vec::new(),
        background_works: Vec::new(),
        subagents: Some(facts.subagents),
        workflow_runs: facts.workflow_runs,
        goal: facts.goal,
        plan: Some(PlanState {
            items: Vec::new(),
            updated_at: Number::from(0u64),
        }),
        workspace_hook_admission: Some(None),
        rows: RowsWindow {
            window: facts.rows,
            total_count: Number::from(total_rows),
            first_row_id: if total_rows > 0 {
                Some(Number::from(1u64))
            } else {
                None
            },
        },
    }
}

/// live 增量映射器的会话侧状态。
#[derive(Default)]
pub struct LiveDeltaState {
    pub next_row_id: u64,
    /// 当前 assistant 文本行（assistantMessageId → rowId）。
    pub text_rows: BTreeMap<String, u64>,
    /// 当前 turn（messageId → rowId 对：turnHeader/userInput）。
    pub turn_rows: Option<(String, u64, u64)>,
    /// 工具行（toolCallId → rowId）。
    pub tool_rows: BTreeMap<String, u64>,
}

impl LiveDeltaState {
    pub fn from_snapshot(rows_total: u64) -> Self {
        Self {
            next_row_id: rows_total + 1,
            text_rows: BTreeMap::new(),
            turn_rows: None,
            tool_rows: BTreeMap::new(),
        }
    }

    /// 用刚发出的快照行初始化 live 状态：rowId 续接快照，且把仍在流式中的
    /// assistant/tool 行登记进映射，后续 `row.delta` 才会更新原行而不是新建重复行。
    pub fn from_snapshot_rows(rows_total: u64, rows: &[ConversationRow]) -> Self {
        let mut state = Self::from_snapshot(rows_total);
        let mut pending_turn: Option<(String, u64)> = None;
        for row in rows {
            match row {
                ConversationRow::TurnHeader(header) => {
                    let row_id = header.base.row_id.as_u64().unwrap_or(0);
                    state.next_row_id = state.next_row_id.max(row_id.saturating_add(1));
                    pending_turn = Some((header.base.turn_id.clone(), row_id));
                    state.turn_rows = None;
                }
                ConversationRow::UserInput(input) => {
                    if let Some((turn_id, header_id)) = pending_turn.take() {
                        state.turn_rows =
                            Some((turn_id, header_id, input.base.row_id.as_u64().unwrap_or(0)));
                    }
                }
                ConversationRow::AssistantText(text) => {
                    let row_id = text.base.row_id.as_u64().unwrap_or(0);
                    state.next_row_id = state.next_row_id.max(row_id.saturating_add(1));
                    let key = text
                        .assistant_response_id
                        .clone()
                        .or_else(|| text.base.entity_id.clone())
                        .unwrap_or_default();
                    if !key.is_empty() {
                        state.text_rows.insert(key, row_id);
                    }
                }
                ConversationRow::ToolCall(tool) => {
                    let row_id = tool.base.row_id.as_u64().unwrap_or(0);
                    state.next_row_id = state.next_row_id.max(row_id.saturating_add(1));
                    if !tool.tool_call_id.is_empty() {
                        state.tool_rows.insert(tool.tool_call_id.clone(), row_id);
                    }
                }
                _ => {}
            }
        }
        state
    }

    fn base(&mut self, turn_id: &str, entity_id: String, at: i64) -> RowBase {
        let row_id = self.next_row_id;
        self.next_row_id += 1;
        RowBase {
            row_id: Number::from(row_id),
            turn_id: turn_id.to_string(),
            entity_id: Some(entity_id),
            product_turn_id: None,
            visibility: Some(agent_protocol::v4_rows::RowVisibility::Visible),
            created_at: Number::from(at),
            created_at_seq: Number::from(at),
            actions: Some(agent_protocol::v4_rows::RowActions::default()),
        }
    }
}

/// 会话事件 → v4 增量（本波覆盖 turn/流式文本/工具/标题/goal 五族；其余登记）。
pub fn event_to_deltas(event: &SessionEvent, state: &mut LiveDeltaState) -> Vec<ConversationDelta> {
    let mut deltas = Vec::new();
    let at = event.timestamp;
    match event.event_type {
        SessionEventType::TurnStarted => {
            if let Ok(Some(payload)) =
                event.payload_as::<agent_protocol::session_event::TurnStartedPayload>()
            {
                let turn_id = payload
                    .message_id
                    .clone()
                    .unwrap_or_else(|| format!("turn-{}", event.seq));
                let header_id = format!("{turn_id}:header");
                let input_id = format!("{turn_id}:input");
                let header_row_id = state.next_row_id;
                deltas.push(row_appended(ConversationRow::TurnHeader(TurnHeaderRow {
                    base: state.base(&turn_id, header_id, at),
                    kind: TurnHeaderKind::TurnHeader,
                    origin: TurnOrigin::UserInput,
                    execution_kind: None,
                    source_command_id: payload.input_id.clone(),
                    history_round_count: None,
                    state: TurnState::Running,
                    started_at: Number::from(at),
                    ended_at: None,
                    active_ms: None,
                    work_segments: None,
                    origin_meta: None,
                    workflow_launch: None,
                    file_changes: None,
                })));
                let input_row_id = state.next_row_id;
                deltas.push(row_appended(ConversationRow::UserInput(UserInputRow {
                    base: state.base(&turn_id, input_id, at),
                    kind: UserInputKind::UserInput,
                    text: payload.input.clone(),
                    epilogue_start: None,
                    origin: UserInputOrigin::RealUser,
                    origin_meta: None,
                    workflow_launch: None,
                    guided: None,
                    source_command_id: payload.input_id.clone(),
                    root_source_command_id: None,
                    client_id: None,
                    attachments: None,
                })));
                state.turn_rows = Some((turn_id, header_row_id, input_row_id));
                deltas.push(state_updated_delta(StatePatch {
                    control: Some(SessionControl {
                        phase: SessionPhase::Running,
                        session_ended: false,
                        can_stop: true,
                        stop_state: StopState::Stoppable,
                        stop_target_kind: StopTargetKind::Assistant,
                        active_works: Vec::new(),
                        last_error: None,
                        api_retry: None,
                    }),
                    input_routing: Some(InputRouting {
                        mode: InputRoutingMode::Enqueue,
                        reason_code: None,
                    }),
                    ..empty_patch()
                }));
            }
        }
        SessionEventType::ModelStreaming => {
            if let Ok(Some(payload)) =
                event.payload_as::<agent_protocol::session_event::ModelStreamingPayload>()
            {
                let turn_id = state
                    .turn_rows
                    .as_ref()
                    .map(|(turn_id, _, _)| turn_id.clone())
                    .unwrap_or_else(|| format!("turn-{}", event.seq));
                match payload.kind {
                    ModelStreamingKind::TextDelta => {
                        let message_id = payload
                            .assistant_message_id
                            .clone()
                            .unwrap_or_else(|| "assistant".to_string());
                        let delta_text = payload.delta.clone().unwrap_or_default();
                        match state.text_rows.get(&message_id).cloned() {
                            Some(row_id) => {
                                deltas.push(ConversationDelta::RowDelta(RowDeltaDelta {
                                    op: agent_protocol::v4_delta::RowDeltaOp::RowDelta,
                                    row_id: Number::from(row_id),
                                    path: StreamablePath::Text,
                                    append: delta_text,
                                }));
                            }
                            None => {
                                let row_id = state.next_row_id;
                                state.text_rows.insert(message_id.clone(), row_id);
                                deltas.push(row_appended(ConversationRow::AssistantText(
                                    AssistantTextRow {
                                        base: state.base(&turn_id, message_id.clone(), at),
                                        kind: AssistantTextKind::AssistantText,
                                        assistant_response_id: Some(message_id.clone()),
                                        text: delta_text,
                                        state: AssistantTextState::Streaming,
                                        model: None,
                                        feedback: None,
                                    },
                                )));
                            }
                        }
                    }
                    ModelStreamingKind::ToolCall => {
                        if let (Some(tool_call_id), Some(tool_name)) =
                            (&payload.tool_call_id, &payload.tool_name)
                        {
                            let row_id = state.next_row_id;
                            state.tool_rows.insert(tool_call_id.clone(), row_id);
                            deltas.push(row_appended(ConversationRow::ToolCall(ToolCallRow {
                                base: state.base(&turn_id, format!("{tool_call_id}:row"), at),
                                kind: ToolCallKind::ToolCall,
                                assistant_response_id: payload.assistant_message_id.clone(),
                                tool_call_id: tool_call_id.clone(),
                                tool_name: tool_name.clone(),
                                status: ToolCallStatus::InputStreaming,
                                input_text: payload
                                    .input
                                    .as_ref()
                                    .map(|input| input.to_string())
                                    .unwrap_or_default(),
                                input: payload.input.clone(),
                                cua_app: None,
                                output: None,
                                display: None,
                                error: None,
                                progress: None,
                                output_preview: None,
                                approval_interaction_id: None,
                                backgrounded: None,
                                work_id: None,
                                started_at: Some(Number::from(at)),
                                ended_at: None,
                            })));
                        }
                    }
                    _ => {}
                }
            }
        }
        SessionEventType::ToolUpdated => {
            if let Ok(Some(payload)) = event.payload_as::<ToolUpdatedPayload>() {
                match payload {
                    ToolUpdatedPayload::Started(started) => {
                        if let Some(row_id) =
                            state.tool_rows.get(&started.call.tool_call_id).cloned()
                        {
                            deltas.push(row_upserted_tool(
                                row_id,
                                started.call.tool_call_id.clone(),
                                started.tool_name.clone().unwrap_or_default(),
                                ToolCallStatus::Running,
                                None,
                                None,
                            ));
                        }
                    }
                    ToolUpdatedPayload::Result(result) => {
                        let text = result
                            .result
                            .content
                            .as_ref()
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if let Some(row_id) =
                            state.tool_rows.get(&result.call.tool_call_id).cloned()
                        {
                            deltas.push(row_upserted_tool(
                                row_id,
                                result.call.tool_call_id.clone(),
                                result.tool_name.clone().unwrap_or_default(),
                                ToolCallStatus::Success,
                                Some(ToolOutput {
                                    text,
                                    display: result.result.extra.get("display").cloned(),
                                    truncated: None,
                                }),
                                None,
                            ));
                        }
                    }
                    ToolUpdatedPayload::Error(error) => {
                        if let Some(row_id) = state.tool_rows.get(&error.call.tool_call_id).cloned()
                        {
                            deltas.push(row_upserted_tool(
                                row_id,
                                error.call.tool_call_id.clone(),
                                error.tool_name.clone().unwrap_or_default(),
                                ToolCallStatus::Error,
                                None,
                                Some(error.error.message.clone()),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        SessionEventType::TurnCompleted => {
            if let Ok(Some(payload)) = event.payload_as::<TurnCompletedPayload>() {
                let interrupted = payload.result_type == TurnResultType::Cancelled;
                if let Some((turn_id, header_row_id, _)) = state.turn_rows.clone() {
                    deltas.push(row_upserted_turn_header(
                        header_row_id,
                        &turn_id,
                        if interrupted {
                            TurnState::CompletedInterrupted
                        } else {
                            TurnState::CompletedSuccess
                        },
                    ));
                }
                for row_id in state.text_rows.values().cloned().collect::<Vec<_>>() {
                    deltas.push(row_upserted_assistant_complete(row_id));
                }
                state.text_rows.clear();
                state.turn_rows = None;
                deltas.push(state_updated_delta(StatePatch {
                    control: Some(SessionControl {
                        phase: SessionPhase::Draft,
                        session_ended: false,
                        can_stop: false,
                        stop_state: StopState::Idle,
                        stop_target_kind: StopTargetKind::Unknown,
                        active_works: Vec::new(),
                        last_error: None,
                        api_retry: None,
                    }),
                    input_routing: Some(InputRouting {
                        mode: InputRoutingMode::StartNow,
                        reason_code: None,
                    }),
                    ..empty_patch()
                }));
            }
        }
        SessionEventType::TurnFailed => {
            if let Some((turn_id, header_row_id, _)) = state.turn_rows.clone() {
                deltas.push(row_upserted_turn_header(
                    header_row_id,
                    &turn_id,
                    TurnState::Failed,
                ));
            }
            state.turn_rows = None;
            deltas.push(state_updated_delta(StatePatch {
                control: Some(SessionControl {
                    phase: SessionPhase::Error,
                    session_ended: false,
                    can_stop: false,
                    stop_state: StopState::Idle,
                    stop_target_kind: StopTargetKind::Unknown,
                    active_works: Vec::new(),
                    last_error: None,
                    api_retry: None,
                }),
                ..empty_patch()
            }));
        }
        SessionEventType::SessionTitleUpdated => {
            if let Ok(Some(payload)) =
                event.payload_as::<agent_protocol::session_event::SessionTitleUpdatedPayload>()
            {
                deltas.push(state_updated_delta(StatePatch {
                    meta: Some(SessionMetaState {
                        title: payload.title.clone(),
                        title_source: TitleSource::Default,
                    }),
                    ..empty_patch()
                }));
            }
        }
        _ => {}
    }
    deltas
}

/// StatePatch 无 Default（协议镜像未派生）；这里给全 None 的基线。
fn empty_patch() -> StatePatch {
    StatePatch {
        revision: None,
        control: None,
        shared_context_import: None,
        availability: None,
        input_routing: None,
        meta: None,
        config: None,
        // 快照 21 键里 modelTransition / workspaceHookAdmission 键必在（null 表示无迁移）。
        model_transition: Some(None),
        usage: None,
        queue: None,
        pending_interactions: None,
        pending_commands: None,
        background_works: None,
        subagents: None,
        workflow_runs: None,
        goal: None,
        plan: None,
        workspace_hook_admission: Some(None),
    }
}

fn row_appended(row: ConversationRow) -> ConversationDelta {
    ConversationDelta::RowAppended(RowAppendedDelta {
        op: agent_protocol::v4_delta::RowAppendedOp::RowAppended,
        row,
    })
}

fn row_upserted(row: ConversationRow) -> ConversationDelta {
    ConversationDelta::RowUpserted(RowUpsertedDelta {
        op: agent_protocol::v4_delta::RowUpsertedOp::RowUpserted,
        row,
    })
}

fn state_updated_delta(patch: StatePatch) -> ConversationDelta {
    ConversationDelta::StateUpdated(Box::new(StateUpdatedDelta {
        op: agent_protocol::v4_delta::StateUpdatedOp::StateUpdated,
        patch,
    }))
}

fn row_upserted_tool(
    row_id: u64,
    tool_call_id: String,
    tool_name: String,
    status: ToolCallStatus,
    output: Option<ToolOutput>,
    error_message: Option<String>,
) -> ConversationDelta {
    row_upserted(ConversationRow::ToolCall(ToolCallRow {
        base: RowBase {
            row_id: Number::from(row_id),
            turn_id: String::new(),
            entity_id: None,
            product_turn_id: None,
            visibility: None,
            created_at: Number::from(0i64),
            created_at_seq: Number::from(0i64),
            actions: Some(agent_protocol::v4_rows::RowActions::default()),
        },
        kind: ToolCallKind::ToolCall,
        assistant_response_id: None,
        tool_call_id,
        tool_name,
        status,
        input_text: String::new(),
        input: None,
        cua_app: None,
        output,
        display: None,
        error: error_message.map(|message| agent_protocol::v4_rows::ToolCallError {
            code: "TOOL_EXECUTION_FAILED".to_string(),
            message,
        }),
        progress: None,
        output_preview: None,
        approval_interaction_id: None,
        backgrounded: None,
        work_id: None,
        started_at: None,
        ended_at: None,
    }))
}

fn row_upserted_turn_header(row_id: u64, turn_id: &str, state: TurnState) -> ConversationDelta {
    row_upserted(ConversationRow::TurnHeader(TurnHeaderRow {
        base: RowBase {
            row_id: Number::from(row_id),
            turn_id: turn_id.to_string(),
            entity_id: None,
            product_turn_id: None,
            visibility: None,
            created_at: Number::from(0i64),
            created_at_seq: Number::from(0i64),
            actions: Some(agent_protocol::v4_rows::RowActions::default()),
        },
        kind: TurnHeaderKind::TurnHeader,
        origin: TurnOrigin::UserInput,
        execution_kind: None,
        source_command_id: None,
        history_round_count: None,
        state,
        started_at: Number::from(0i64),
        ended_at: None,
        active_ms: None,
        work_segments: None,
        origin_meta: None,
        workflow_launch: None,
        file_changes: None,
    }))
}

fn row_upserted_assistant_complete(row_id: u64) -> ConversationDelta {
    row_upserted(ConversationRow::AssistantText(AssistantTextRow {
        base: RowBase {
            row_id: Number::from(row_id),
            turn_id: String::new(),
            entity_id: None,
            product_turn_id: None,
            visibility: None,
            created_at: Number::from(0i64),
            created_at_seq: Number::from(0i64),
            actions: Some(agent_protocol::v4_rows::RowActions::default()),
        },
        kind: AssistantTextKind::AssistantText,
        assistant_response_id: None,
        text: String::new(),
        state: AssistantTextState::Complete,
        model: None,
        feedback: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_changes_for_turn_projects_write_parts() {
        let user_data = json!({ "anchor": { "turnId": "turn_1" }, "role": "user" });
        let assistant_data = json!({ "anchor": { "turnId": "turn_1" }, "role": "assistant" });
        let write_part = json!({
            "type": "tool",
            "tool": "Write",
            "callID": "call_1",
            "state": {
                "input": { "file_path": "C:/ws/a.txt", "content": "line one\nline two\n" },
                "status": "completed"
            }
        });
        let user_parts: Vec<(String, Option<i64>, i64, Value)> = Vec::new();
        let assistant_parts = vec![("part_1".to_string(), Some(0), 2, write_part)];
        let messages = vec![
            ProjectedMessage {
                id: "msg_1",
                role: "user",
                sequence: Some(0),
                time_created: 1,
                data: &user_data,
                parts: &user_parts,
            },
            ProjectedMessage {
                id: "msg_2",
                role: "assistant",
                sequence: Some(1),
                time_created: 2,
                data: &assistant_data,
                parts: &assistant_parts,
            },
        ];
        let changes = file_changes_for_turn(&messages, "turn_1");
        assert_eq!(changes.files, 1);
        assert_eq!(changes.additions, 2);
        assert_eq!(changes.deletions, 0);
        assert_eq!(changes.items[0].path, "C:/ws/a.txt");
        assert_eq!(changes.items[0].write_count, 1);
        assert_eq!(changes.items[0].tool_names, vec!["Write"]);
        assert_eq!(changes.items[0].patches[0].old_lines.as_u64(), Some(0));
        assert_eq!(changes.items[0].patches[0].new_lines.as_u64(), Some(2));
        assert_eq!(
            changes.items[0].patches[0].lines,
            vec!["+line one", "+line two"]
        );

        // turnHeader 行带摘要（与 `v4/conversation/rowsRange` 的观察一致）。
        let rows = project_rows(&messages);
        let header = rows
            .iter()
            .find_map(|row| match row {
                ConversationRow::TurnHeader(header) => Some(header),
                _ => None,
            })
            .expect("turnHeader");
        let summary = header.file_changes.clone().expect("fileChanges");
        assert_eq!(summary.additions.as_u64(), Some(2));
        assert_eq!(summary.files.as_u64(), Some(1));
    }

    #[test]
    fn turn_ids_use_the_nearest_user_message_and_tool_status_is_preserved() {
        let user1 = json!({ "role": "user", "anchor": { "turnId": "t1" } });
        let user2 = json!({ "role": "user", "anchor": { "turnId": "t2" } });
        let assistant = json!({ "role": "assistant" });
        let text1 = json!({ "type": "text", "text": "a1" });
        let text2 = json!({ "type": "text", "text": "a2" });
        let failed_tool = json!({
            "type": "tool",
            "tool": "Read",
            "state": { "callID": "call_2", "status": "error", "input": {} }
        });
        let empty: Vec<(String, Option<i64>, i64, Value)> = Vec::new();
        let parts1 = vec![("p1".to_string(), Some(0), 2, text1)];
        let parts2 = vec![
            ("p2".to_string(), Some(0), 4, text2),
            ("p3".to_string(), Some(1), 5, failed_tool),
        ];
        let messages = vec![
            ProjectedMessage {
                id: "msg_1",
                role: "user",
                sequence: Some(0),
                time_created: 1,
                data: &user1,
                parts: &empty,
            },
            ProjectedMessage {
                id: "msg_2",
                role: "assistant",
                sequence: Some(1),
                time_created: 2,
                data: &assistant,
                parts: &parts1,
            },
            ProjectedMessage {
                id: "msg_3",
                role: "user",
                sequence: Some(2),
                time_created: 3,
                data: &user2,
                parts: &empty,
            },
            ProjectedMessage {
                id: "msg_4",
                role: "assistant",
                sequence: Some(3),
                time_created: 4,
                data: &assistant,
                parts: &parts2,
            },
        ];
        let rows = project_rows(&messages);
        let texts: Vec<(String, String)> = rows
            .iter()
            .filter_map(|row| match row {
                ConversationRow::AssistantText(text) => {
                    Some((text.base.turn_id.clone(), text.text.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                ("msg_1".to_string(), "a1".to_string()),
                ("msg_3".to_string(), "a2".to_string())
            ]
        );
        let tool = rows
            .iter()
            .find_map(|row| match row {
                ConversationRow::ToolCall(tool) => Some(tool),
                _ => None,
            })
            .expect("tool row");
        assert_eq!(tool.status, ToolCallStatus::Error);
        assert_eq!(tool.base.turn_id, "msg_3");
    }

    #[test]
    fn live_state_continues_snapshot_row_ids_and_registers_rows() {
        let user1 = json!({ "role": "user", "anchor": { "turnId": "t1" } });
        let user2 = json!({ "role": "user", "anchor": { "turnId": "t2" } });
        let assistant = json!({ "role": "assistant" });
        let text = json!({ "type": "text", "text": "hello" });
        let tool = json!({
            "type": "tool",
            "tool": "Bash",
            "state": { "callID": "call_live", "status": "running", "input": {} }
        });
        let empty: Vec<(String, Option<i64>, i64, Value)> = Vec::new();
        let parts1 = vec![("p1".to_string(), Some(0), 2, text.clone())];
        let parts2 = vec![
            ("p2".to_string(), Some(0), 4, text),
            ("p3".to_string(), Some(1), 5, tool),
        ];
        let messages = vec![
            ProjectedMessage {
                id: "msg_1",
                role: "user",
                sequence: Some(0),
                time_created: 1,
                data: &user1,
                parts: &empty,
            },
            ProjectedMessage {
                id: "msg_2",
                role: "assistant",
                sequence: Some(1),
                time_created: 2,
                data: &assistant,
                parts: &parts1,
            },
            ProjectedMessage {
                id: "msg_3",
                role: "user",
                sequence: Some(2),
                time_created: 3,
                data: &user2,
                parts: &empty,
            },
            ProjectedMessage {
                id: "msg_4",
                role: "assistant",
                sequence: Some(3),
                time_created: 4,
                data: &assistant,
                parts: &parts2,
            },
        ];
        let rows = project_rows(&messages);
        let total = rows.len() as u64;
        let state = LiveDeltaState::from_snapshot_rows(total, &rows);
        assert_eq!(state.next_row_id, total + 1);
        // 两个 assistant 文本行都登记到 messageId，后续 delta 复用而不是新建。
        assert_eq!(state.text_rows.get("msg_2").copied(), Some(3));
        assert_eq!(state.text_rows.get("msg_4").copied(), Some(6));
        assert_eq!(state.tool_rows.get("call_live").copied(), Some(7));
        assert_eq!(state.turn_rows, Some(("msg_3".to_string(), 4, 5)));
    }
}
