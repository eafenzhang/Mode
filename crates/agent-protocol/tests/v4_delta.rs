//! v4 `conversationDelta` 七个 op 的 round-trip。

use agent_protocol::v4_delta::ConversationDelta;
use serde_json::json;

fn round_trip(value: serde_json::Value) -> ConversationDelta {
    let delta: ConversationDelta =
        serde_json::from_value(value.clone()).unwrap_or_else(|error| {
            panic!("解不开 delta {value}: {error}")
        });
    assert_eq!(serde_json::to_value(&delta).unwrap(), value);
    delta
}

#[test]
fn row_ops_round_trip() {
    let row = json!({
        "rowId": 1,
        "turnId": "turn-1",
        "createdAt": 1,
        "createdAtSeq": 1,
        "kind": "userInput",
        "text": "hi",
        "origin": "realUser"
    });

    assert!(matches!(
        round_trip(json!({ "op": "row.appended", "row": row })),
        ConversationDelta::RowAppended(_)
    ));
    assert!(matches!(
        round_trip(json!({ "op": "row.upserted", "row": row })),
        ConversationDelta::RowUpserted(_)
    ));
    assert!(matches!(
        round_trip(json!({ "op": "row.removed", "fromRowId": 3 })),
        ConversationDelta::RowRemoved(_)
    ));
    assert!(matches!(
        round_trip(json!({
            "op": "row.delta",
            "rowId": 1,
            "path": "output.text",
            "append": "x"
        })),
        ConversationDelta::RowDelta(_)
    ));
}

#[test]
fn state_updated_patch_round_trips_with_nullable_keys() {
    let delta = round_trip(json!({
        "op": "state.updated",
        "patch": {
            "revision": 2,
            "modelTransition": null,
            "goal": null,
            "plan": null,
            "workspaceHookAdmission": null,
            "queue": { "items": [], "autoDrain": true }
        }
    }));
    match delta {
        ConversationDelta::StateUpdated(updated) => {
            assert_eq!(updated.patch.model_transition, Some(None));
            assert_eq!(updated.patch.goal, Some(None));
            assert!(updated.patch.queue.is_some());
        }
        other => panic!("unexpected delta: {other:?}"),
    }
}

#[test]
fn workflow_run_ops_round_trip() {
    let updated = round_trip(json!({
        "op": "workflowRun.updated",
        "runId": "run-1",
        "revision": 4,
        "run": { "status": "running" },
        "cleared": ["error", "reports"],
        "removedActors": [{ "siteId": "site-1", "ordinal": 0 }],
        "removedNodes": [{ "siteId": "site-1", "ordinal": 1 }],
        "actors": [{
            "siteId": "site-1",
            "ordinal": 0,
            "status": "running"
        }],
        "nodes": [{
            "siteId": "site-1",
            "ordinal": 2,
            "phase": "queued"
        }]
    }));
    match updated {
        ConversationDelta::WorkflowRunUpdated(delta) => {
            assert_eq!(delta.run_id, "run-1");
            assert!(delta.run.is_some());
            assert_eq!(delta.cleared.as_ref().unwrap().len(), 2);
            assert_eq!(delta.actors.as_ref().unwrap().len(), 1);
        }
        other => panic!("unexpected delta: {other:?}"),
    }

    assert!(matches!(
        round_trip(json!({ "op": "workflowRun.removed", "runId": "run-1", "revision": 5 })),
        ConversationDelta::WorkflowRunRemoved(_)
    ));
}
