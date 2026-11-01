//! v4 sessions-index：topic、快照/增量与 wire frame 的严格镜像。

use agent_protocol::v4_sessions_index::{
    parse_sessions_index_topic, sessions_index_topic, SessionWorkflowActivity,
    SessionWorkflowPhaseStatus, SessionWorkflowPhaseSummary, SessionWorkflowRunSummary,
    SessionsIndexDelta, SessionsIndexFramePayload, SessionsIndexSnapshot, SessionsIndexTopicFrame,
    SessionsIndexWireFrame, SESSION_WORKFLOW_ACTIVITY_MAX_RUNS, SESSION_WORKFLOW_MAX_PHASES,
};
use agent_protocol::v4_transport::WireDeliveryKind;
use serde_json::{json, Number};

fn frame_json(topic: &str, payload: serde_json::Value) -> serde_json::Value {
    json!({
        "topic": topic,
        "subscriptionId": "sub-1",
        "fromSeq": 0,
        "toSeq": 3,
        "sentAt": 10.5,
        "payload": payload
    })
}

fn session_json() -> serde_json::Value {
    json!({
        "sessionId": "sess-1",
        "workspaceId": "ws-1",
        "title": "Fix login",
        "titleSource": "generated",
        "phase": "running",
        "sessionEnded": false,
        "hasBackgroundWork": true,
        "workflowActivity": {
            "runs": [{
                "runId": "run-1",
                "toolCallId": "launch-1",
                "name": "Review",
                "status": "running",
                "startedAt": 5,
                "phases": [
                    { "name": "plan", "status": "running", "alongside": [1] },
                    { "name": "build", "status": "pending" }
                ],
                "currentPhase": "plan",
                "agentsWorking": 2
            }]
        },
        "pendingInteraction": { "interactionId": "int-1", "kind": "userInput", "toolName": "AskUserQuestion" },
        "pendingInteractionSummary": { "permissionCount": 0, "userInputCount": 1 },
        "goalStatus": "active",
        "lastActivityAt": 20,
        "lastAssistantPreview": "done",
        "createdAt": 1
    })
}

#[test]
fn topic_round_trip_and_rejects_empty_workspace() {
    assert_eq!(sessions_index_topic("ws-1"), "sessions-index/ws-1");
    assert_eq!(
        parse_sessions_index_topic("sessions-index/ws-1"),
        Some("ws-1")
    );
    assert_eq!(parse_sessions_index_topic("sessions-index/"), None);
    assert_eq!(parse_sessions_index_topic("conversation/sess-1"), None);
}

#[test]
fn snapshot_frame_parses_optional_and_workflow_fields() {
    let value = frame_json(
        "sessions-index/ws-1",
        json!({
            "kind": "snapshot",
            "snapshot": {
                "protocolVersion": 1,
                "workspaceId": "ws-1",
                "logEpoch": "epoch-1",
                "sessions": [session_json()]
            }
        }),
    );
    let frame: SessionsIndexTopicFrame = serde_json::from_value(value).expect("合法帧");
    frame.validate().expect("帧校验通过");
    let SessionsIndexFramePayload::Snapshot(payload) = &frame.payload else {
        panic!("期望 snapshot payload");
    };
    assert_eq!(payload.snapshot.sessions[0].session_id, "sess-1");
    let activity = payload.snapshot.sessions[0]
        .workflow_activity
        .as_ref()
        .expect("workflowActivity");
    assert_eq!(
        activity.runs[0].phases[0].status,
        SessionWorkflowPhaseStatus::Running
    );
    assert_eq!(activity.runs[0].phases[0].alongside, Some(vec![1]));

    // 旧 frame：titleSource / workflowActivity / pending 摘要整体缺席。
    let old = frame_json(
        "sessions-index/ws-1",
        json!({
            "kind": "snapshot",
            "snapshot": {
                "protocolVersion": 1,
                "workspaceId": "ws-1",
                "logEpoch": "epoch-1",
                "sessions": [{
                    "sessionId": "sess-2",
                    "workspaceId": "ws-1",
                    "title": "old",
                    "phase": "draft",
                    "sessionEnded": false,
                    "hasBackgroundWork": false,
                    "lastActivityAt": 1,
                    "createdAt": 1
                }]
            }
        }),
    );
    let old_frame: SessionsIndexTopicFrame = serde_json::from_value(old).expect("旧帧仍可读");
    old_frame.validate().expect("旧帧校验通过");
}

#[test]
fn deltas_parse_upsert_and_remove_and_reject_unknown_ops() {
    let _upsert: SessionsIndexDelta =
        serde_json::from_value(json!({ "op": "session.upserted", "session": session_json() }))
            .expect("upsert delta");
    let _payload: agent_protocol::v4_sessions_index::SessionsIndexDeltasPayload =
        serde_json::from_value(json!({
            "kind": "deltas",
            "deltas": [
                { "op": "session.upserted", "session": session_json() },
                { "op": "session.removed", "sessionId": "sess-1" }
            ]
        }))
        .expect("deltas payload");
    let frame: SessionsIndexTopicFrame = serde_json::from_value(frame_json(
        "sessions-index/ws-1",
        json!({
            "kind": "deltas",
            "deltas": [
                { "op": "session.upserted", "session": session_json() },
                { "op": "session.removed", "sessionId": "sess-1" }
            ]
        }),
    ))
    .expect("合法增量帧");
    frame.validate().expect("增量帧校验通过");
    let SessionsIndexFramePayload::Deltas(payload) = &frame.payload else {
        panic!("期望 deltas payload");
    };
    assert!(matches!(
        payload.deltas[0],
        SessionsIndexDelta::SessionUpserted { .. }
    ));
    assert!(matches!(
        payload.deltas[1],
        SessionsIndexDelta::SessionRemoved { .. }
    ));

    let unknown = serde_json::from_value::<SessionsIndexTopicFrame>(frame_json(
        "sessions-index/ws-1",
        json!({ "kind": "deltas", "deltas": [{ "op": "session.moved", "sessionId": "s" }] }),
    ));
    assert!(unknown.is_err());
}

#[test]
fn validation_enforces_topic_protocol_version_and_bounds() {
    let bad_topic = frame_json(
        "sessions-index/",
        json!({
            "kind": "snapshot",
            "snapshot": { "protocolVersion": 1, "workspaceId": "ws", "logEpoch": "e", "sessions": [] }
        }),
    );
    let frame: SessionsIndexTopicFrame = serde_json::from_value(bad_topic).expect("形状可解析");
    assert!(frame.validate().is_err());

    let snapshot = SessionsIndexSnapshot {
        protocol_version: 2,
        workspace_id: "ws-1".to_string(),
        log_epoch: "e".to_string(),
        sessions: Vec::new(),
    };
    assert!(snapshot.validate().is_err());

    let mut session: agent_protocol::v4_sessions_index::SessionSummary =
        serde_json::from_value(session_json()).expect("session");
    let run = SessionWorkflowRunSummary {
        run_id: "run-1".to_string(),
        tool_call_id: None,
        name: None,
        status: agent_protocol::v4_workflow_runs::WorkflowRunStatus::Running,
        stop_reason: None,
        started_at: Some(Number::from(1)),
        phases: vec![SessionWorkflowPhaseSummary {
            name: "plan".to_string(),
            status: SessionWorkflowPhaseStatus::Running,
            alongside: Some(vec![5]),
        }],
        current_phase: Some("plan".to_string()),
        agents_working: 0,
    };
    session.workflow_activity = Some(SessionWorkflowActivity { runs: vec![run] });
    assert!(session.validate().is_err(), "alongside 越界必须拒绝");

    let mut too_many_runs = Vec::new();
    for index in 0..=SESSION_WORKFLOW_ACTIVITY_MAX_RUNS {
        too_many_runs.push(SessionWorkflowRunSummary {
            run_id: format!("run-{index}"),
            tool_call_id: None,
            name: None,
            status: agent_protocol::v4_workflow_runs::WorkflowRunStatus::Completed,
            stop_reason: None,
            started_at: None,
            phases: Vec::new(),
            current_phase: None,
            agents_working: 0,
        });
    }
    session.workflow_activity = Some(SessionWorkflowActivity {
        runs: too_many_runs,
    });
    assert!(session.validate().is_err(), "runs 上界必须拒绝");

    let mut too_many_phases = Vec::new();
    for index in 0..=SESSION_WORKFLOW_MAX_PHASES {
        too_many_phases.push(SessionWorkflowPhaseSummary {
            name: format!("p{index}"),
            status: SessionWorkflowPhaseStatus::Pending,
            alongside: None,
        });
    }
    session.workflow_activity = Some(SessionWorkflowActivity {
        runs: vec![SessionWorkflowRunSummary {
            run_id: "run-1".to_string(),
            tool_call_id: None,
            name: None,
            status: agent_protocol::v4_workflow_runs::WorkflowRunStatus::Running,
            stop_reason: None,
            started_at: None,
            phases: too_many_phases,
            current_phase: None,
            agents_working: 0,
        }],
    });
    assert!(session.validate().is_err(), "phases 上界必须拒绝");
}

#[test]
fn wire_frames_parse_complete_and_fragment_and_validate_topic() {
    let complete = json!({
        "wireVersion": 3,
        "kind": "complete",
        "deliveryKind": "online",
        "logicalFrameId": "lf-1",
        "logicalFrameOrdinal": 1,
        "topic": "sessions-index/ws-1",
        "subscriptionId": "sub-1",
        "frame": frame_json(
            "sessions-index/ws-1",
            json!({
                "kind": "snapshot",
                "snapshot": { "protocolVersion": 1, "workspaceId": "ws-1", "logEpoch": "e", "sessions": [] }
            })
        )
    });
    let wire: SessionsIndexWireFrame = serde_json::from_value(complete).expect("complete wire");
    assert_eq!(wire.subscription_id(), "sub-1");
    wire.validate().expect("complete wire 校验通过");

    let fragment = json!({
        "wireVersion": 3,
        "kind": "fragment",
        "deliveryKind": "online",
        "logicalFrameId": "lf-1",
        "logicalFrameOrdinal": 1,
        "topic": "sessions-index/ws-1",
        "subscriptionId": "sub-1",
        "fragmentIndex": 0,
        "fragmentCount": 1,
        "logicalBytes": 10,
        "checksum": { "algorithm": "crc32", "value": "0" },
        "dataBase64": "AA=="
    });
    let wire: SessionsIndexWireFrame = serde_json::from_value(fragment).expect("fragment wire");
    wire.validate().expect("fragment 只校验 topic");

    let bad = json!({
        "wireVersion": 3,
        "kind": "complete",
        "deliveryKind": WireDeliveryKind::Online,
        "logicalFrameId": "lf-1",
        "logicalFrameOrdinal": 1,
        "topic": "conversation/x",
        "subscriptionId": "sub-1",
        "frame": frame_json(
            "conversation/x",
            json!({
                "kind": "snapshot",
                "snapshot": { "protocolVersion": 1, "workspaceId": "ws-1", "logEpoch": "e", "sessions": [] }
            })
        )
    });
    let wire: SessionsIndexWireFrame = serde_json::from_value(bad).expect("形状可解析");
    assert!(wire.validate().is_err(), "topic 前缀必须拒绝");
}
