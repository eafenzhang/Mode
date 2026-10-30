//! v4 `workspace-config` topic 载荷：快照与单 op 增量。

use agent_protocol::v4_workspace_config::{
    WorkspaceConfigDelta, WorkspaceConfigOptionType, WorkspaceConfigSnapshot, WorkspaceConfigValue,
};
use serde_json::{json, Value};

fn round_trip<T>(value: &Value) -> T
where
    T: serde::de::DeserializeOwned + serde::Serialize + PartialEq + std::fmt::Debug,
{
    let parsed: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&parsed).unwrap(), *value);
    parsed
}

fn state() -> Value {
    json!({
        "configOptions": [
            {
                "id": "model",
                "name": "模型",
                "type": "select",
                "currentValue": "glm-5",
                "options": [
                    {
                        "value": "glm-5",
                        "name": "GLM-5",
                        "origin": "native",
                        "modelProviderId": "zai",
                        "modelProviderName": "Z.AI",
                        "modelThoughtLevels": ["low", "high"],
                        "modelDefaultThoughtLevel": "high"
                    }
                ]
            },
            {
                "id": "plan",
                "name": "计划模式",
                "type": "boolean",
                "currentValue": true
            }
        ],
        "slashCommands": [
            { "name": "/help", "description": "帮助", "source": "builtin" }
        ]
    })
}

#[test]
fn snapshot_round_trips_with_typed_option_values() {
    let value = json!({
        "protocolVersion": 1,
        "workspaceId": "/ws/demo",
        "logEpoch": "epoch-1",
        "config": state()
    });
    let snapshot: WorkspaceConfigSnapshot = round_trip(&value);
    snapshot.validate().unwrap();
    assert_eq!(snapshot.config.config_options.len(), 2);
    assert_eq!(
        snapshot.config.config_options[0].option_type,
        WorkspaceConfigOptionType::Select
    );
    assert_eq!(
        snapshot.config.config_options[1].current_value,
        WorkspaceConfigValue::Switch(true)
    );
    assert_eq!(
        snapshot.config.config_options[0]
            .options
            .as_ref()
            .unwrap()[0]
            .model_thought_levels
            .as_ref()
            .map(Vec::len),
        Some(2)
    );
}

#[test]
fn protocol_version_other_than_one_is_rejected() {
    let value = json!({
        "protocolVersion": 2,
        "workspaceId": "/ws/demo",
        "logEpoch": "epoch-1",
        "config": state()
    });
    let snapshot: WorkspaceConfigSnapshot = serde_json::from_value(value).unwrap();
    assert!(snapshot.validate().is_err());
}

#[test]
fn delta_round_trips_and_unknown_keys_are_stripped() {
    let value = json!({ "op": "config.updated", "config": state() });
    let delta: WorkspaceConfigDelta = round_trip(&value);
    match delta {
        WorkspaceConfigDelta::ConfigUpdated(updated) => {
            assert_eq!(updated.config.slash_commands[0].name, "/help");
        }
    }

    // 非严格 schema：未知键被忽略（对应 zod strip）。
    let with_extra = json!({ "op": "config.updated", "config": state(), "future": 1 });
    let parsed: WorkspaceConfigDelta = serde_json::from_value(with_extra).unwrap();
    assert!(matches!(parsed, WorkspaceConfigDelta::ConfigUpdated(_)));
}
