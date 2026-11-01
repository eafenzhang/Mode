//! workspace-config topic：帧/wire frame 与 topic 前缀。

use agent_protocol::v4_workspace_config::{
    parse_workspace_config_topic, workspace_config_topic, WorkspaceConfigDelta,
    WorkspaceConfigFramePayload, WorkspaceConfigSnapshot, WorkspaceConfigTopicFrame,
    WorkspaceConfigWireFrame,
};
use serde_json::json;

fn frame_json(topic: &str, payload: serde_json::Value) -> serde_json::Value {
    json!({
        "topic": topic,
        "subscriptionId": "sub-1",
        "fromSeq": 0,
        "toSeq": 1,
        "sentAt": 1,
        "payload": payload
    })
}

#[test]
fn topic_round_trip_and_snapshot_validation() {
    assert_eq!(workspace_config_topic("ws-1"), "workspace-config/ws-1");
    assert_eq!(
        parse_workspace_config_topic("workspace-config/ws-1"),
        Some("ws-1")
    );
    assert_eq!(parse_workspace_config_topic("workspace-config/"), None);

    let value = frame_json(
        "workspace-config/ws-1",
        json!({
            "kind": "snapshot",
            "snapshot": {
                "protocolVersion": 1,
                "workspaceId": "ws-1",
                "logEpoch": "epoch-1",
                "config": {
                    "configOptions": [{
                        "id": "mode",
                        "name": "Mode",
                        "type": "boolean",
                        "currentValue": true
                    }],
                    "slashCommands": [{ "name": "/review", "description": "Review" }]
                }
            }
        }),
    );
    let frame: WorkspaceConfigTopicFrame = serde_json::from_value(value).expect("合法帧");
    frame.validate().expect("帧校验通过");
    match &frame.payload {
        WorkspaceConfigFramePayload::Snapshot(payload) => {
            assert_eq!(payload.snapshot.config.config_options.len(), 1);
            assert_eq!(payload.snapshot.config.slash_commands[0].name, "/review");
        }
        other => panic!("unexpected payload: {other:?}"),
    }

    let bad_topic = frame_json(
        "conversation/sess-1",
        json!({
            "kind": "snapshot",
            "snapshot": {
                "protocolVersion": 1,
                "workspaceId": "ws-1",
                "logEpoch": "e",
                "config": { "configOptions": [], "slashCommands": [] }
            }
        }),
    );
    let bad: WorkspaceConfigTopicFrame = serde_json::from_value(bad_topic).expect("形状可解析");
    assert!(bad.validate().is_err());

    let bad_version = WorkspaceConfigSnapshot {
        protocol_version: 2,
        workspace_id: "ws-1".to_string(),
        log_epoch: "e".to_string(),
        config: agent_protocol::v4_workspace_config::WorkspaceConfigState {
            config_options: Vec::new(),
            slash_commands: Vec::new(),
        },
    };
    assert!(bad_version.validate().is_err());
}

#[test]
fn delta_and_wire_frame_parse() {
    let value = frame_json(
        "workspace-config/ws-1",
        json!({
            "kind": "deltas",
            "deltas": [{
                "op": "config.updated",
                "config": { "configOptions": [], "slashCommands": [] }
            }]
        }),
    );
    let frame: WorkspaceConfigTopicFrame = serde_json::from_value(value).expect("增量帧");
    frame.validate().expect("增量帧校验通过");
    let WorkspaceConfigFramePayload::Deltas(payload) = &frame.payload else {
        panic!("期望 deltas");
    };
    assert!(matches!(
        payload.deltas[0],
        WorkspaceConfigDelta::ConfigUpdated(_)
    ));

    let complete = json!({
        "wireVersion": 3,
        "kind": "complete",
        "deliveryKind": "online",
        "logicalFrameId": "wc-1",
        "logicalFrameOrdinal": 1,
        "topic": "workspace-config/ws-1",
        "subscriptionId": "sub-1",
        "frame": frame_json(
            "workspace-config/ws-1",
            json!({
                "kind": "snapshot",
                "snapshot": {
                    "protocolVersion": 1,
                    "workspaceId": "ws-1",
                    "logEpoch": "e",
                    "config": { "configOptions": [], "slashCommands": [] }
                }
            })
        )
    });
    let wire: WorkspaceConfigWireFrame = serde_json::from_value(complete).expect("complete wire");
    wire.validate().expect("wire 校验通过");
    assert_eq!(wire.subscription_id(), "sub-1");
}
