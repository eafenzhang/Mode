//! `v4/commands/query`：参数与结果的严格镜像（`commandsQueryParamsSchema` /
//! `commandsQueryResultSchema`）。
//!
//! 重点：`sessionId` 是「必填但可为 null」，不能用裸 `Option` 把缺失也吞成 None；
//! clock 探针只允许全局 key；结果里的 `result` 是 ACK 与 `"unknown"` 的联合。

use agent_protocol::v4::{
    CommandKey, CommandQueryItem, CommandQueryOutcome, CommandsQueryParams, CommandsQueryResult,
    LocalTtftClock, UnknownCommand, COMMANDS_QUERY_MAX,
};
use serde_json::{json, Number};

#[test]
fn params_require_session_id_key_and_keep_null_distinct_from_missing() {
    let missing: Result<CommandsQueryParams, _> =
        serde_json::from_value(json!({ "commands": [{ "commandId": "cmd-1" }] }));
    assert!(
        missing.is_err(),
        "sessionId 必须出现（值可为 null），实际 {missing:?}"
    );

    let params: CommandsQueryParams = serde_json::from_value(json!({
        "commands": [
            { "sessionId": null, "commandId": "cmd-1" },
            { "sessionId": "sess-1", "commandId": "cmd-2" }
        ]
    }))
    .expect("合法参数");
    params.validate().expect("无 clock 时允许多来源混合");
    assert!(params.commands[0].session_id.0.is_none());
    assert_eq!(params.commands[1].session_id.0.as_deref(), Some("sess-1"));
}

#[test]
fn params_reject_unknown_keys_clock_false_and_clock_with_session_commands() {
    let unknown: Result<CommandsQueryParams, _> = serde_json::from_value(json!({
        "commands": [{ "sessionId": null, "commandId": "cmd-1", "extra": 1 }]
    }));
    assert!(unknown.is_err());

    let clock_false = CommandsQueryParams {
        commands: vec![CommandKey::global("cmd-1")],
        clock: Some(false),
    };
    assert!(clock_false.validate().is_err());

    let clock_with_session = CommandsQueryParams {
        commands: vec![CommandKey::session("sess-1", "cmd-1")],
        clock: Some(true),
    };
    assert!(clock_with_session.validate().is_err());

    let empty = CommandsQueryParams {
        commands: Vec::new(),
        clock: None,
    };
    assert!(empty.validate().is_err());

    let too_many = CommandsQueryParams {
        commands: (0..=COMMANDS_QUERY_MAX)
            .map(|index| CommandKey::global(format!("cmd-{index}")))
            .collect(),
        clock: None,
    };
    assert!(too_many.validate().is_err());
}

#[test]
fn result_accepts_ack_and_unknown_and_rejects_unknown_keys() {
    let result: CommandsQueryResult = serde_json::from_value(json!({
        "results": [
            {
                "key": { "sessionId": "sess-1", "commandId": "cmd-1" },
                "result": {
                    "commandId": "cmd-1",
                    "status": "accepted",
                    "revisionAtDecision": 3.5,
                    "memoryEnabled": false
                }
            },
            {
                "key": { "sessionId": null, "commandId": "cmd-2" },
                "result": "unknown"
            }
        ],
        "clock": { "instanceId": "cli-1", "receivedAt": 10, "sentAt": 20.5 }
    }))
    .expect("合法结果");
    result.validate().expect("两条结果在 1..=64 内");

    match &result.results[0].result {
        CommandQueryOutcome::Ack(ack) => {
            assert_eq!(ack.command_id, "cmd-1");
            assert_eq!(ack.revision_at_decision, Number::from_f64(3.5).unwrap());
        }
        other => panic!("期望 ACK，实际 {other:?}"),
    }
    assert_eq!(
        result.results[1].result,
        CommandQueryOutcome::Unknown(UnknownCommand::Unknown)
    );

    let extra: Result<CommandQueryItem, _> = serde_json::from_value(json!({
        "key": { "sessionId": null, "commandId": "cmd-1" },
        "result": "unknown",
        "extra": true
    }));
    assert!(extra.is_err());
}

#[test]
fn result_validate_rejects_empty_and_invalid_clock() {
    let empty = CommandsQueryResult {
        results: Vec::new(),
        clock: None,
    };
    assert!(empty.validate().is_err());

    let negative_clock = CommandsQueryResult {
        results: vec![CommandQueryItem {
            key: CommandKey::global("cmd-1"),
            result: CommandQueryOutcome::Unknown(UnknownCommand::Unknown),
        }],
        clock: Some(LocalTtftClock {
            instance_id: "cli-1".to_string(),
            received_at: Number::from_f64(-1.0).unwrap(),
            sent_at: Number::from(0),
        }),
    };
    assert!(negative_clock.validate().is_err());
}
