//! v4 会话行第一批：turnHeader / userInput / assistantText / reasoning 的 round-trip。

use agent_protocol::v4_rows::{
    AssistantTextRow, Feedback, ReasoningRow, TurnHeaderRow, UserInputRow,
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

#[test]
fn turn_header_round_trips_with_actions_segments_and_launch_meta() {
    let value = json!({
        "rowId": 1,
        "turnId": "turn-1",
        "entityId": "entity-1",
        "productTurnId": "turn-1",
        "visibility": "visible",
        "createdAt": 1700000000000u64,
        "createdAtSeq": 3,
        "actions": {
            "canFork": true,
            "canEdit": true,
            "editDisposition": "rewind"
        },
        "kind": "turnHeader",
        "origin": "workflowLaunch",
        "executionKind": "controlOnly",
        "sourceCommandId": "cmd-1",
        "historyRoundCount": 2,
        "state": "completedSuccess",
        "startedAt": 1700000000001u64,
        "endedAt": 1700000000002u64,
        "activeMs": 1000,
        "workSegments": [
            {
                "segmentId": "seg-1",
                "triggerEntityId": "entity-1",
                "startedAt": 1700000000001u64,
                "endedAt": 1700000000002u64,
                "activeMs": 1000
            }
        ],
        "originMeta": {
            "backgroundSource": "workflow",
            "workId": "run-1",
            "title": "后台工作流",
            "workflowNotification": { "kind": "stall", "sinceMs": 1 }
        },
        "workflowLaunch": {
            "runId": "run-1",
            "toolCallId": "launch-1",
            "name": "demo",
            "scope": "project",
            "path": "/ws/demo/.mode/workflows/demo.md",
            "args": { "topic": "demo" },
            "description": "启动说明",
            "script": "print(1)"
        },
        "fileChanges": {
            "additions": 1,
            "deletions": 0,
            "files": 1,
            "state": "active"
        }
    });
    let row: TurnHeaderRow = round_trip(&value);
    assert_eq!(row.base.row_id.as_u64(), Some(1));
    assert_eq!(row.work_segments.as_ref().unwrap().len(), 1);
}

#[test]
fn user_input_round_trips_with_attachments_and_guided() {
    let value = json!({
        "rowId": 2,
        "turnId": "turn-1",
        "createdAt": 1700000000000u64,
        "createdAtSeq": 4,
        "kind": "userInput",
        "text": "你好",
        "epilogueStart": 2,
        "origin": "realUser",
        "originMeta": {
            "backgroundSource": "bash",
            "workId": "work-1",
            "senderSessionId": "sess-2",
            "senderLabel": "后台"
        },
        "guided": true,
        "sourceCommandId": "cmd-1",
        "rootSourceCommandId": "cmd-root",
        "clientId": "client-1",
        "attachments": [
            {
                "ref": "att-1",
                "fileName": "a.png",
                "mime": "image/png",
                "bytes": 12,
                "previewRef": "preview-1"
            }
        ]
    });
    let row: UserInputRow = round_trip(&value);
    assert_eq!(row.attachments.as_ref().unwrap().len(), 1);
    assert!(row.guided.is_some());
}

#[test]
fn assistant_text_and_reasoning_round_trip() {
    let text = json!({
        "rowId": 3,
        "turnId": "turn-1",
        "createdAt": 1700000000000u64,
        "createdAtSeq": 5,
        "kind": "assistantText",
        "assistantResponseId": "resp-1",
        "text": "回答",
        "state": "complete",
        "model": "glm-4.6",
        "feedback": "like"
    });
    let row: AssistantTextRow = round_trip(&text);
    assert_eq!(row.feedback, Some(Feedback::Like));

    let reasoning = json!({
        "rowId": 4,
        "turnId": "turn-1",
        "createdAt": 1700000000000u64,
        "createdAtSeq": 6,
        "kind": "reasoning",
        "assistantResponseId": "resp-1",
        "text": "思考",
        "state": "streaming",
        "durationMs": 12
    });
    let row: ReasoningRow = round_trip(&reasoning);
    assert_eq!(row.text, "思考");
}

#[test]
fn unknown_row_keys_are_ignored_like_zod_strip() {
    let value = json!({
        "rowId": 5,
        "turnId": "turn-1",
        "createdAt": 1700000000000u64,
        "createdAtSeq": 7,
        "kind": "assistantText",
        "text": "x",
        "state": "complete",
        "futureField": { "anything": true }
    });
    let parsed: AssistantTextRow = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.text, "x");
}
