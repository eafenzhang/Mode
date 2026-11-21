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

use agent_process::{AgentControlClient, AgentWorkspaceTarget};
use agent_protocol::v4_sessions_index::{
    SessionSummary, SessionsIndexDelta, SessionsIndexFramePayload,
};
use agent_protocol::v4_transport::SubscribeVisibility;
use agent_protocol::v4_wire_assembler::{
    SessionIndexAssemblyEvent, SessionsIndexWireAssembler,
};
use serde_json::{json, Value};

use crate::window_controller::{SessionOverlayStore, workspace_identity_key};

type OnChange = Arc<dyn Fn() + Send + Sync>;

pub struct SessionsOverlayObserver {
    control: Arc<AgentControlClient>,
    overlays: Arc<SessionOverlayStore>,
    on_change: OnChange,
    /// 已观察的 workspaceKey；失败/结束时移除，供下次 ensure 重试。
    observed: Arc<Mutex<HashSet<String>>>,
}

impl SessionsOverlayObserver {
    pub fn new(
        control: Arc<AgentControlClient>,
        overlays: Arc<SessionOverlayStore>,
        on_change: OnChange,
    ) -> Self {
        Self {
            control,
            overlays,
            on_change,
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
        let workspace_path = workspace_path.to_string();
        let workspace_identity = workspace_identity.map(str::to_string);
        let task_key = key.clone();
        handle.spawn(async move {
            run_observer(
                control,
                overlays,
                on_change,
                observed,
                task_key,
                workspace_path,
                workspace_identity,
            )
            .await;
        });
    }
}

async fn run_observer(
    control: Arc<AgentControlClient>,
    overlays: Arc<SessionOverlayStore>,
    on_change: OnChange,
    observed: Arc<Mutex<HashSet<String>>>,
    key: String,
    workspace_path: String,
    workspace_identity: Option<String>,
) {
    let target = AgentWorkspaceTarget {
        workspace_path: workspace_path.clone(),
        workspace_identity: workspace_identity.clone(),
        remote_session_id: None,
    };
    let mut subscription = match control
        .subscribe_sessions_index_frames_if_running(
            &target,
            None,
            Some(SubscribeVisibility::Background),
        )
        .await
    {
        Ok(subscription) => subscription,
        Err(_) => {
            // dormant：下次 list/subscribe 再试。
            observed.lock().expect("sessions observer poisoned").remove(&key);
            return;
        }
    };
    let mut summaries: HashMap<String, SessionSummary> = HashMap::new();
    let mut assembler = SessionsIndexWireAssembler::new();
    while let Some(wire) = subscription.recv().await {
        let mut failed = false;
        for event in assembler.accept(&wire, now_ms()) {
            match event {
                SessionIndexAssemblyEvent::Complete { frame, .. } => {
                    apply_sessions_frame(&mut summaries, &frame.payload);
                    let overlay_rows: Vec<Value> = summaries.values().map(session_overlay).collect();
                    overlays.replace(&key, overlay_rows);
                    on_change();
                }
                SessionIndexAssemblyEvent::Fault(_) => {
                    failed = true;
                }
            }
        }
        if failed {
            break;
        }
    }
    // 流结束/失败：清空 overlay 并允许下次重建订阅。
    overlays.replace(&key, Vec::new());
    on_change();
    observed.lock().expect("sessions observer poisoned").remove(&key);
}

/// snapshot 全量替换 / deltas 增量维护。
fn apply_sessions_frame(
    summaries: &mut HashMap<String, SessionSummary>,
    payload: &SessionsIndexFramePayload,
) {
    match payload {
        SessionsIndexFramePayload::Snapshot(snapshot) => {
            summaries.clear();
            for session in &snapshot.snapshot.sessions {
                summaries.insert(session.session_id.clone(), session.clone());
            }
        }
        SessionsIndexFramePayload::Deltas(deltas) => {
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

    #[test]
    fn snapshot_replaces_and_deltas_apply() {
        let mut summaries = HashMap::new();
        let frame: SessionsIndexFramePayload = serde_json::from_value(json!({
            "kind": "snapshot",
            "snapshot": {
                "protocolVersion": 1,
                "workspaceId": "ws-1",
                "logEpoch": "epoch-1",
                "sessions": [base("s1", "running"), base("s2", "draft")]
            }
        }))
        .expect("snapshot payload");
        apply_sessions_frame(&mut summaries, &frame);
        assert_eq!(summaries.len(), 2);

        let frame: SessionsIndexFramePayload = serde_json::from_value(json!({
            "kind": "deltas",
            "deltas": [
                { "op": "session.upserted", "session": base("s1", "completedSuccess") },
                { "op": "session.removed", "sessionId": "s2" }
            ]
        }))
        .expect("deltas payload");
        apply_sessions_frame(&mut summaries, &frame);
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries["s1"].phase,
            agent_protocol::v4_snapshot::SessionPhase::CompletedSuccess
        );
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
