//! window-controller 的 sessions-index 被动观察器（真源
//! `packages/desktop/src/host-standalone/windowController/windowHostSessionsIndexObserver.ts`）。
//!
//! - existing-only 订阅（`subscribe_sessions_index_frames_if_running`）：runtime 缺席时
//!   dormant，不拉起 Agent。
//! - 分片经 [`SessionsIndexWireAssembler`] 重组；snapshot 全量替换、deltas 增量维护
//!   `sessionId → SessionSummary`。
//! - 每次变化把 summaries 映射成 overlay（TS `sessionOverlay`）写进
//!   `SessionOverlayStore`，并触发 controller 帧 refresh。
//! - 故障/流结束：清空该 workspace 的 overlay 并停止观察（下次 list/subscribe 重试）。
//!   与 TS 的 resync/重启恢复相比是简化版——已知差距，登记 spec。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_process::{AgentControlClient, AgentWorkspaceTarget, RuntimePolicy};
use agent_protocol::v4_queries::{ConversationResyncParams, ResyncBase};
use agent_protocol::v4_sessions_index::{
    SessionSummary, SessionsIndexDelta, SessionsIndexFramePayload, SessionsIndexTopicFrame,
};
use agent_protocol::v4_snapshot::SessionPhase;
use agent_protocol::v4_transport::{SubscribeBase, SubscribeVisibility};
use agent_protocol::v4_wire_assembler::{
    SessionIndexAssemblyEvent, SessionsIndexWireAssembler,
};
use serde_json::{json, Number, Value};

use crate::window_controller::{SessionOverlayStore, workspace_identity_key};

type OnChange = Arc<dyn Fn() + Send + Sync>;

/// 恢复等待 snapshot 的上限（TS `logicalFrameAssemblyTimeoutMs`）。
const RECOVERY_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);
/// 流结束/不可用时的有限重试（TS 依赖 runtime lifecycle 事件；宿主侧用退避兜底）。
const RESUBSCRIBE_ATTEMPTS: u32 = 5;
const RESUBSCRIBE_DELAY: Duration = Duration::from_millis(1_000);

#[derive(Clone)]
struct OverlayCursor {
    log_epoch: String,
    seq: u64,
}

pub struct SessionsOverlayObserver {
    control: Arc<AgentControlClient>,
    overlays: Arc<SessionOverlayStore>,
    on_change: OnChange,
    /// 同 server 的任务事件总线：phase 终态迁移 → terminal/ready 事件。
    task_events: Option<Arc<crate::mode_task::TaskEventBus>>,
    /// 已观察的 workspaceKey；失败/结束时移除，供下次 ensure 重试。
    observed: Arc<Mutex<HashSet<String>>>,
}

impl SessionsOverlayObserver {
    pub fn new(
        control: Arc<AgentControlClient>,
        overlays: Arc<SessionOverlayStore>,
        on_change: OnChange,
        task_events: Option<Arc<crate::mode_task::TaskEventBus>>,
    ) -> Self {
        Self {
            control,
            overlays,
            on_change,
            task_events,
            observed: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// 确保某 workspace 有观察任务（幂等）。runtime 不可用/无 tokio 上下文时静默跳过。
    pub fn ensure_workspace(&self, workspace_path: &str, workspace_identity: Option<&str>) {
        let key = workspace_identity_key(workspace_identity, workspace_path);
        {
            let mut observed = self.observed.lock().expect("sessions observer poisoned");
            if !observed.insert(key.clone()) {
                return;
            }
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            // 无异步运行时（纯同步装配/测试）：保持 dormant。
            self.observed
                .lock()
                .expect("sessions observer poisoned")
                .remove(&key);
            return;
        };
        let control = Arc::clone(&self.control);
        let overlays = Arc::clone(&self.overlays);
        let on_change = Arc::clone(&self.on_change);
        let observed = Arc::clone(&self.observed);
        let task_events = self.task_events.clone();
        let workspace_path = workspace_path.to_string();
        let workspace_identity = workspace_identity.map(str::to_string);
        let task_key = key.clone();
        handle.spawn(async move {
            run_observer(ObserverTask {
                control,
                overlays,
                on_change,
                task_events,
                observed,
                key: task_key,
                workspace_path,
                workspace_identity,
            })
            .await;
        });
    }
}

struct ObserverTask {
    control: Arc<AgentControlClient>,
    overlays: Arc<SessionOverlayStore>,
    on_change: OnChange,
    task_events: Option<Arc<crate::mode_task::TaskEventBus>>,
    observed: Arc<Mutex<HashSet<String>>>,
    key: String,
    workspace_path: String,
    workspace_identity: Option<String>,
}

async fn run_observer(task: ObserverTask) {
    let ObserverTask {
        control,
        overlays,
        on_change,
        task_events,
        observed,
        key,
        workspace_path,
        workspace_identity,
    } = task;
    let target = AgentWorkspaceTarget {
        workspace_path: workspace_path.clone(),
        workspace_identity: workspace_identity.clone(),
        remote_session_id: None,
    };
    let topic = format!("sessions-index/{}", target.workspace_key());
    let workspace_key = target.workspace_key();
    let (restart_tx, mut restart_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let restart_subscription = control.on_runtime_restarted(Arc::new(move |event| {
        if event.workspace_key == workspace_key {
            let _ = restart_tx.send(());
        }
    }));

    let mut summaries: HashMap<String, SessionSummary> = HashMap::new();
    let mut cursor: Option<OverlayCursor> = None;
    let mut attempts: u32 = 0;
    'resubscribe: loop {
        let base = cursor.as_ref().map(|cursor| SubscribeBase {
            log_epoch: cursor.log_epoch.clone(),
            seq: Number::from(cursor.seq),
        });
        let mut subscription = match control
            .subscribe_sessions_index_frames_if_running(
                &target,
                base,
                Some(SubscribeVisibility::Background),
            )
            .await
        {
            Ok(subscription) => subscription,
            Err(_) => {
                // dormant：有限退避重试，避免每次列表查询都打 dormant RPC。
                attempts += 1;
                if attempts > RESUBSCRIBE_ATTEMPTS {
                    break 'resubscribe;
                }
                tokio::time::sleep(RESUBSCRIBE_DELAY).await;
                continue;
            }
        };
        attempts = 0;
        let subscription_id = subscription.ack.subscription_id.clone();
        let mut assembler = SessionsIndexWireAssembler::new();
        let mut awaiting_snapshot = false;
        let mut need_resubscribe = false;

        loop {
            let received = if awaiting_snapshot {
                // resync 后等待 snapshot；超时视为 recovery 失败 → 重建订阅。
                match tokio::time::timeout(RECOVERY_SNAPSHOT_TIMEOUT, subscription.recv()).await {
                    Ok(frame) => frame,
                    Err(_) => {
                        need_resubscribe = true;
                        None
                    }
                }
            } else {
                tokio::select! {
                    frame = subscription.recv() => frame,
                    _ = restart_rx.recv() => {
                        // Agent runtime 换代：清空投影与游标，用新 runtime 重新订阅。
                        summaries.clear();
                        cursor = None;
                        publish_overlays(&overlays, &key, &summaries, &on_change);
                        need_resubscribe = true;
                        None
                    }
                }
            };
            let Some(wire) = received else {
                break;
            };
            let mut request_recovery = false;
            for event in assembler.accept(&wire, now_ms()) {
                match event {
                    SessionIndexAssemblyEvent::Complete { frame, .. } => {
                        let previous_phases = phase_snapshot(&summaries);
                        match apply_sessions_topic_frame(&mut summaries, &mut cursor, &frame) {
                            FrameApply::Applied => {
                                if matches!(
                                    frame.payload,
                                    SessionsIndexFramePayload::Snapshot(_)
                                ) {
                                    awaiting_snapshot = false;
                                }
                                publish_overlays(&overlays, &key, &summaries, &on_change);
                                emit_terminal_transitions(
                                    &previous_phases,
                                    &summaries,
                                    &workspace_path,
                                    workspace_identity.as_deref(),
                                    task_events.as_ref(),
                                );
                            }
                            FrameApply::NeedRecovery => request_recovery = true,
                        }
                    }
                    SessionIndexAssemblyEvent::Fault(_) => request_recovery = true,
                }
            }
            if request_recovery {
                assembler.abort(&topic, &subscription_id);
                let params = ConversationResyncParams {
                    topic: topic.clone(),
                    connection_id: agent_process::V4_LOCAL_CONNECTION_ID.to_string(),
                    subscription_id: subscription_id.clone(),
                    base: cursor.as_ref().map(|cursor| ResyncBase {
                        log_epoch: cursor.log_epoch.clone(),
                        seq: cursor.seq,
                    }),
                    force_snapshot: Some(true),
                };
                match control
                    .resync_conversation_v4(&target, params, RuntimePolicy::ExistingOnly)
                    .await
                {
                    Ok(_) => {
                        awaiting_snapshot = true;
                        assembler = SessionsIndexWireAssembler::new();
                    }
                    Err(_) => {
                        need_resubscribe = true;
                        break;
                    }
                }
            }
        }
        drop(subscription);
        // 重建订阅：不复用旧 base，由服务端新 snapshot 覆盖投影。
        cursor = None;
        attempts += 1;
        if attempts > RESUBSCRIBE_ATTEMPTS {
            break 'resubscribe;
        }
        let _ = need_resubscribe;
        tokio::time::sleep(RESUBSCRIBE_DELAY).await;
    }
    restart_subscription();
    // 最终放弃：清空 overlay 并允许下次 list/subscribe 重建观察。
    overlays.replace(&key, Vec::new());
    on_change();
    observed
        .lock()
        .expect("sessions observer poisoned")
        .remove(&key);
}

fn phase_snapshot(summaries: &HashMap<String, SessionSummary>) -> HashMap<String, SessionPhase> {
    summaries
        .iter()
        .map(|(task_id, summary)| (task_id.clone(), summary.phase))
        .collect()
}

struct TerminalTransition {
    task_id: String,
    failed: bool,
}

/// `phase` 终态：completedSuccess/completedInterrupted → 成功；error → 失败。
fn terminal_phase(phase: SessionPhase) -> Option<bool> {
    match phase {
        SessionPhase::CompletedSuccess | SessionPhase::CompletedInterrupted => Some(false),
        SessionPhase::Error => Some(true),
        _ => None,
    }
}

fn terminal_transitions(
    previous: &HashMap<String, SessionPhase>,
    summaries: &HashMap<String, SessionSummary>,
) -> Vec<TerminalTransition> {
    let mut transitions = Vec::new();
    for (task_id, summary) in summaries {
        let Some(failed) = terminal_phase(summary.phase) else {
            continue;
        };
        // 只在非终态 → 终态迁移时发射；初始快照/新行不触发，避免启动误收口。
        let Some(previous_phase) = previous.get(task_id) else {
            continue;
        };
        if terminal_phase(*previous_phase).is_some() {
            continue;
        }
        transitions.push(TerminalTransition {
            task_id: task_id.clone(),
            failed,
        });
    }
    transitions
}

/// TS `emitTerminalAndReady`：先 turn 终态（收口当前 input），再 prompt ready。
fn emit_terminal_transitions(
    previous_phases: &HashMap<String, SessionPhase>,
    summaries: &HashMap<String, SessionSummary>,
    workspace_path: &str,
    workspace_identity: Option<&str>,
    task_events: Option<&Arc<crate::mode_task::TaskEventBus>>,
) {
    let Some(events) = task_events else {
        return;
    };
    for transition in terminal_transitions(previous_phases, summaries) {
        let input_id =
            events.take_active_input(workspace_path, workspace_identity, &transition.task_id);
        events.notify_terminal(
            &transition.task_id,
            input_id.as_deref(),
            if transition.failed { "failed" } else { "succeeded" },
        );
        events.notify_ready(
            &transition.task_id,
            if transition.failed {
                "prompt_failed"
            } else {
                "prompt_completed"
            },
        );
    }
}

fn publish_overlays(
    overlays: &SessionOverlayStore,
    key: &str,
    summaries: &HashMap<String, SessionSummary>,
    on_change: &OnChange,
) {
    let rows: Vec<Value> = summaries.values().map(session_overlay).collect();
    overlays.replace(key, rows);
    on_change();
}

enum FrameApply {
    Applied,
    NeedRecovery,
}

/// snapshot 全量替换并重置游标；deltas 必须与游标连续，否则请求 recovery。
fn apply_sessions_topic_frame(
    summaries: &mut HashMap<String, SessionSummary>,
    cursor: &mut Option<OverlayCursor>,
    frame: &SessionsIndexTopicFrame,
) -> FrameApply {
    match &frame.payload {
        SessionsIndexFramePayload::Snapshot(snapshot) => {
            summaries.clear();
            for session in &snapshot.snapshot.sessions {
                summaries.insert(session.session_id.clone(), session.clone());
            }
            *cursor = Some(OverlayCursor {
                log_epoch: snapshot.snapshot.log_epoch.clone(),
                seq: frame.to_seq.as_u64().unwrap_or(0),
            });
            FrameApply::Applied
        }
        SessionsIndexFramePayload::Deltas(deltas) => {
            let Some(current) = cursor.as_mut() else {
                return FrameApply::NeedRecovery;
            };
            if frame.from_seq.as_u64() != Some(current.seq) {
                return FrameApply::NeedRecovery;
            }
            for delta in &deltas.deltas {
                match delta {
                    SessionsIndexDelta::SessionUpserted { session } => {
                        summaries.insert(session.session_id.clone(), (**session).clone());
                    }
                    SessionsIndexDelta::SessionRemoved { session_id } => {
                        summaries.remove(session_id);
                    }
                }
            }
            current.seq = frame.to_seq.as_u64().unwrap_or(current.seq);
            FrameApply::Applied
        }
    }
}

/// TS `sessionOverlay`：summary → `WindowHostControllerSessionOverlay`。
pub fn session_overlay(summary: &SessionSummary) -> Value {
    let phase = serde_json::to_value(summary.phase)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    let waiting = summary.pending_interaction.is_some()
        || summary
            .pending_interaction_summary
            .as_ref()
            .is_some_and(|pending| pending.permission_count + pending.user_input_count > 0);
    let live_status = if waiting {
        "waiting"
    } else {
        match phase.as_str() {
            "prewarming" | "running" => "running",
            "error" => "error",
            "completedSuccess" | "completedInterrupted" => "completed",
            _ => "idle",
        }
    };
    let mut activity = json!({
        "phase": phase,
        "lastActivityAt": summary.last_activity_at,
        "hasBackgroundWork": summary.has_background_work,
    });
    if let Some(pending) = &summary.pending_interaction_summary {
        activity["pendingInteractions"] = serde_json::to_value(pending).unwrap_or(Value::Null);
    }
    if let Some(workflow) = &summary.workflow_activity {
        activity["workflowActivity"] = serde_json::to_value(workflow).unwrap_or(Value::Null);
    }
    let mut overlay = json!({
        "taskId": summary.session_id,
        "liveStatus": live_status,
        "title": summary.title,
        "updatedAt": summary.last_activity_at,
        "activity": activity,
    });
    if let Some(title_source) = &summary.title_source {
        if let Ok(value) = serde_json::to_value(title_source) {
            overlay["titleSource"] = value;
        }
    }
    if let Some(pending) = &summary.pending_interaction {
        let mut value = json!({
            "interactionId": pending.interaction_id,
            "kind": serde_json::to_value(pending.kind).unwrap_or(Value::Null),
        });
        if let Some(tool_name) = &pending.tool_name {
            value["toolName"] = json!(tool_name);
        }
        if let Some(auto_resolution) = &pending.auto_resolution {
            value["autoResolution"] =
                serde_json::to_value(auto_resolution).unwrap_or(Value::Null);
        }
        overlay["pendingInteraction"] = value;
    }
    overlay
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(value: Value) -> SessionSummary {
        serde_json::from_value(value).expect("valid session summary")
    }

    fn base(session_id: &str, phase: &str) -> Value {
        json!({
            "sessionId": session_id,
            "workspaceId": "ws-1",
            "title": "任务",
            "phase": phase,
            "sessionEnded": false,
            "hasBackgroundWork": false,
            "lastActivityAt": 1_000,
            "createdAt": 900,
        })
    }

    fn topic_frame(value: Value) -> SessionsIndexTopicFrame {
        serde_json::from_value(value).expect("valid topic frame")
    }

    #[test]
    fn snapshot_replaces_deltas_apply_and_gap_needs_recovery() {
        let mut summaries = HashMap::new();
        let mut cursor = None;
        let snapshot = topic_frame(json!({
            "topic": "sessions-index/ws-1",
            "subscriptionId": "sub-1",
            "fromSeq": 0,
            "toSeq": 2,
            "sentAt": 1,
            "payload": {
                "kind": "snapshot",
                "snapshot": {
                    "protocolVersion": 1,
                    "workspaceId": "ws-1",
                    "logEpoch": "epoch-1",
                    "sessions": [base("s1", "running"), base("s2", "draft")]
                }
            }
        }));
        assert!(matches!(
            apply_sessions_topic_frame(&mut summaries, &mut cursor, &snapshot),
            FrameApply::Applied
        ));
        assert_eq!(summaries.len(), 2);
        let current = cursor.as_ref().expect("cursor");
        assert_eq!(current.log_epoch, "epoch-1");
        assert_eq!(current.seq, 2);

        let deltas = topic_frame(json!({
            "topic": "sessions-index/ws-1",
            "subscriptionId": "sub-1",
            "fromSeq": 2,
            "toSeq": 3,
            "sentAt": 2,
            "payload": {
                "kind": "deltas",
                "deltas": [
                    { "op": "session.upserted", "session": base("s1", "completedSuccess") },
                    { "op": "session.removed", "sessionId": "s2" }
                ]
            }
        }));
        assert!(matches!(
            apply_sessions_topic_frame(&mut summaries, &mut cursor, &deltas),
            FrameApply::Applied
        ));
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries["s1"].phase,
            agent_protocol::v4_snapshot::SessionPhase::CompletedSuccess
        );
        assert_eq!(cursor.as_ref().expect("cursor").seq, 3);

        // fromSeq 与游标不连续 → 请求 recovery，且不应用增量。
        let gap = topic_frame(json!({
            "topic": "sessions-index/ws-1",
            "subscriptionId": "sub-1",
            "fromSeq": 5,
            "toSeq": 6,
            "sentAt": 3,
            "payload": { "kind": "deltas", "deltas": [] }
        }));
        assert!(matches!(
            apply_sessions_topic_frame(&mut summaries, &mut cursor, &gap),
            FrameApply::NeedRecovery
        ));
        assert_eq!(cursor.as_ref().expect("cursor").seq, 3);

        // 无游标的增量同样需要 recovery。
        let mut empty_cursor = None;
        assert!(matches!(
            apply_sessions_topic_frame(&mut summaries, &mut empty_cursor, &deltas),
            FrameApply::NeedRecovery
        ));
    }

    fn summaries_with(entries: &[(&str, &str)]) -> HashMap<String, SessionSummary> {
        entries
            .iter()
            .map(|(task_id, phase)| {
                (
                    task_id.to_string(),
                    summary(base(task_id, phase)),
                )
            })
            .collect()
    }

    #[test]
    fn terminal_transitions_only_on_non_terminal_to_terminal() {
        let previous = summaries_with(&[("s1", "running"), ("s2", "completedSuccess")]);
        let mut next = previous.clone();
        next.insert("s1".to_string(), summary(base("s1", "completedSuccess")));
        // s2 已是终态，再变 error 不重复发射；s3 新行终态也不发射。
        next.insert("s2".to_string(), summary(base("s2", "error")));
        next.insert("s3".to_string(), summary(base("s3", "error")));
        let phases: HashMap<String, SessionPhase> = previous
            .iter()
            .map(|(task_id, summary)| (task_id.clone(), summary.phase))
            .collect();
        let transitions = terminal_transitions(&phases, &next);
        assert_eq!(transitions.len(), 1, "{}", transitions.len());
        assert_eq!(transitions[0].task_id, "s1");
        assert!(!transitions[0].failed);

        let previous = summaries_with(&[("s4", "prewarming")]);
        let next = summaries_with(&[("s4", "error")]);
        let phases: HashMap<String, SessionPhase> = previous
            .iter()
            .map(|(task_id, summary)| (task_id.clone(), summary.phase))
            .collect();
        let transitions = terminal_transitions(&phases, &next);
        assert_eq!(transitions.len(), 1);
        assert!(transitions[0].failed);
    }

    #[tokio::test]
    async fn terminal_transition_emits_terminal_then_ready_with_input_id() {
        let bus = Arc::new(crate::mode_task::TaskEventBus::new());
        bus.record_active_input("D:/work/app", None, "s1", "input-1");
        let terminal = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let ready = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let terminal_sink = Arc::clone(&terminal);
        let ready_sink = Arc::clone(&ready);
        let _terminal_sub = bus.subscribe_terminal(
            Some("s1".to_string()),
            Arc::new(move |value| {
                if let rpc_wire::Value::Json(text) = value {
                    terminal_sink
                        .lock()
                        .expect("terminal")
                        .push(serde_json::from_str(&text).expect("terminal json"));
                }
            }),
        );
        let _ready_sub = bus.subscribe_ready(
            Some("s1".to_string()),
            Arc::new(move |value| {
                if let rpc_wire::Value::Json(text) = value {
                    ready_sink
                        .lock()
                        .expect("ready")
                        .push(serde_json::from_str(&text).expect("ready json"));
                }
            }),
        );
        let previous = summaries_with(&[("s1", "running")]);
        let next = summaries_with(&[("s1", "error")]);
        let phases: HashMap<String, SessionPhase> = previous
            .iter()
            .map(|(task_id, summary)| (task_id.clone(), summary.phase))
            .collect();
        emit_terminal_transitions(&phases, &next, "D:/work/app", None, Some(&bus));

        let terminal = terminal.lock().expect("terminal");
        assert_eq!(terminal.len(), 1, "{terminal:?}");
        assert_eq!(terminal[0]["taskId"], "s1");
        assert_eq!(terminal[0]["inputId"], "input-1");
        assert_eq!(terminal[0]["outcome"], "failed");
        let ready = ready.lock().expect("ready");
        assert_eq!(ready.len(), 1, "{ready:?}");
        assert_eq!(ready[0]["taskId"], "s1");
        assert_eq!(ready[0]["reason"], "prompt_failed");
        // inputId 已被消费：再次迁移不再携带。
        assert!(bus.take_active_input("D:/work/app", None, "s1").is_none());
    }

    #[test]
    fn overlays_map_live_status_and_activity() {
        let overlay = session_overlay(&summary(base("s1", "running")));
        assert_eq!(overlay["taskId"], "s1");
        assert_eq!(overlay["liveStatus"], "running");
        assert_eq!(overlay["activity"]["phase"], "running");
        assert_eq!(overlay["activity"]["lastActivityAt"], 1_000);
        assert_eq!(overlay["activity"]["hasBackgroundWork"], false);
        assert!(overlay.get("pendingInteraction").is_none());

        assert_eq!(
            session_overlay(&summary(base("s1", "completedSuccess")))["liveStatus"],
            "completed"
        );
        assert_eq!(
            session_overlay(&summary(base("s1", "draft")))["liveStatus"],
            "idle"
        );

        let mut value = base("s2", "running");
        value["pendingInteraction"] = json!({
            "interactionId": "i1",
            "kind": "permission",
            "toolName": "bash"
        });
        let overlay = session_overlay(&summary(value));
        assert_eq!(overlay["liveStatus"], "waiting");
        assert_eq!(overlay["pendingInteraction"]["interactionId"], "i1");
        assert_eq!(overlay["pendingInteraction"]["kind"], "permission");
        assert_eq!(overlay["pendingInteraction"]["toolName"], "bash");

        let mut value = base("s3", "prewarming");
        value["pendingInteractionSummary"] = json!({ "permissionCount": 0, "userInputCount": 2 });
        value["titleSource"] = json!("custom");
        let overlay = session_overlay(&summary(value));
        assert_eq!(overlay["liveStatus"], "waiting");
        assert_eq!(overlay["titleSource"], "custom");
        assert_eq!(
            overlay["activity"]["pendingInteractions"]["userInputCount"],
            2
        );
    }
}
