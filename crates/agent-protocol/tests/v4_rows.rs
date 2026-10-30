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

#[test]
fn tool_call_round_trips_with_output_progress_and_preview() {
    use agent_protocol::v4_rows::{ToolCallRow, ToolCallStatus};

    let value = json!({
        "rowId": 6,
        "turnId": "turn-1",
        "createdAt": 1700000000000u64,
        "createdAtSeq": 8,
        "kind": "toolCall",
        "assistantResponseId": "resp-1",
        "toolCallId": "call-1",
        "toolName": "Bash",
        "status": "running",
        "inputText": "{\"command\":\"ls\"}",
        "input": { "command": "ls" },
        "cuaApp": { "pid": 42, "name": "Finder", "bundleId": "com.apple.finder" },
        "output": {
            "text": "a.txt",
            "display": { "kind": "anything" },
            "truncated": { "totalBytes": 10, "ref": "ref-1" }
        },
        "display": { "kind": "node_repl_images" },
        "error": { "code": "E1", "message": "boom" },
        "progress": { "bytes": 12, "previewLine": "a", "updatedAt": 1700000000001u64 },
        "outputPreview": {
            "text": "a",
            "fullText": "abc",
            "totalLines": 1,
            "totalBytes": 3,
            "linesEstimated": false
        },
        "approvalInteractionId": "perm-1",
        "backgrounded": true,
        "workId": "work-1",
        "startedAt": 1700000000002u64,
        "endedAt": 1700000000003u64
    });
    let row: ToolCallRow = round_trip(&value);
    assert_eq!(row.status, ToolCallStatus::Running);
    assert_eq!(row.output.as_ref().unwrap().text, "a.txt");
    assert!(row.output_preview.is_some());
}

#[test]
fn artifact_and_subagent_rows_round_trip() {
    use agent_protocol::v4_rows::{ArtifactRow, ConversationArtifactType, SubagentRow};

    let artifact = json!({
        "rowId": 7,
        "turnId": "turn-1",
        "createdAt": 1700000000000u64,
        "createdAtSeq": 9,
        "kind": "artifact",
        "artifactVersionId": "v1",
        "logicalArtifactKey": "report",
        "displayName": "报告",
        "artifactType": "pdf",
        "mimeType": "application/pdf",
        "sizeBytes": 12,
        "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "ref": "artifact-ref",
        "state": "current"
    });
    let row: ArtifactRow = round_trip(&artifact);
    assert_eq!(row.artifact_type, ConversationArtifactType::Pdf);

    let subagent = json!({
        "rowId": 8,
        "turnId": "turn-1",
        "createdAt": 1700000000000u64,
        "createdAtSeq": 10,
        "kind": "subagent",
        "parentToolCallId": "call-1",
        "subagentType": "explore",
        "status": "success",
        "summaryText": "完成",
        "childSessionId": "sess-child",
        "backgrounded": true,
        "workId": "work-1",
        "startedAt": 1700000000001u64,
        "endedAt": 1700000000002u64
    });
    let row: SubagentRow = round_trip(&subagent);
    assert_eq!(row.summary_text, "完成");
    assert!(row.backgrounded.is_some());
}

#[test]
fn hook_invocation_round_trips_with_execution_projection() {
    use agent_protocol::v4_rows::{HookInvocationRow, HookProjectionSourceKind};

    let value = json!({
        "rowId": 9,
        "turnId": "turn-1",
        "createdAt": 1700000000000u64,
        "createdAtSeq": 11,
        "kind": "hookInvocation",
        "hookInvocationId": "hook-1",
        "hookEventName": "PreToolUse",
        "hookCount": 1,
        "state": "completed",
        "startedAt": 1700000000001u64,
        "endedAt": 1700000000002u64,
        "durationMs": 1000,
        "lane": "toolBefore",
        "anchorToolCallId": "call-1",
        "executions": [{
            "hookRunId": "run-1",
            "hookIndex": 0,
            "didExecute": true,
            "state": "completed",
            "outcome": "success",
            "startedAt": 1700000000001u64,
            "endedAt": 1700000000002u64,
            "durationMs": 1000,
            "displayName": "demo hook",
            "sourceKind": "plugin",
            "pluginName": "demo-plugin",
            "toolName": "Bash"
        }]
    });
    let row: HookInvocationRow = round_trip(&value);
    assert_eq!(row.executions.len(), 1);
    assert_eq!(
        row.executions[0].source_kind,
        HookProjectionSourceKind::Plugin
    );
}

#[test]
fn timeline_marker_payloads_round_trip() {
    use agent_protocol::v4_rows::{TimelineMarkerPayload, TimelineMarkerRow};

    let cases = [
        json!({ "type": "compact", "origin": "auto", "status": "success", "tokensBefore": 10, "tokensAfter": 4 }),
        json!({ "type": "forkNotice", "parentSessionId": "sess-parent", "parentRowId": 1 }),
        json!({ "type": "forkCreated", "childSessionId": "sess-child", "atRowId": 2 }),
        json!({ "type": "modelChange", "fromProvider": "zai", "fromModel": "glm-4", "toProvider": "zai", "toModel": "glm-4.6", "toThought": "high" }),
        json!({ "type": "modelChange", "toProvider": "zai", "toModel": "glm-4.6", "toThought": "high" }),
        json!({ "type": "goalSet", "objective": "完成目标", "previousObjective": "旧目标" }),
        json!({ "type": "goalVerify", "iteration": 1, "outcome": "pass" }),
        json!({ "type": "retryNotice", "attempt": 2, "reasonCode": "rate_limit" }),
        json!({ "type": "checkpointRestored", "checkpointId": "cp-1" }),
    ];
    for marker in cases {
        let payload: TimelineMarkerPayload =
            serde_json::from_value(marker.clone()).unwrap_or_else(|error| {
                panic!("解不开 marker {marker}: {error}")
            });
        assert_eq!(serde_json::to_value(&payload).unwrap(), marker);
    }

    let row: TimelineMarkerRow = round_trip(&json!({
        "rowId": 10,
        "turnId": "turn-1",
        "createdAt": 1700000000000u64,
        "createdAtSeq": 12,
        "kind": "timelineMarker",
        "sourceCommandId": "cmd-1",
        "lane": "lightBoundary",
        "marker": { "type": "modelChange", "toProvider": "zai", "toModel": "glm-4.6", "toThought": "high" }
    }));
    assert!(row.lane.is_some());
}

#[test]
fn conversation_row_union_parses_each_kind() {
    use agent_protocol::v4_rows::ConversationRow;

    let base = |row_id: u64, kind: &str| {
        json!({
            "rowId": row_id,
            "turnId": "turn-1",
            "createdAt": 1700000000000u64,
            "createdAtSeq": row_id,
            "kind": kind
        })
    };
    let rows = [
        json!({ "rowId": 1, "turnId": "turn-1", "createdAt": 1, "createdAtSeq": 1, "kind": "turnHeader", "origin": "userInput", "state": "running", "startedAt": 1 }),
        json!({ "rowId": 2, "turnId": "turn-1", "createdAt": 1, "createdAtSeq": 2, "kind": "userInput", "text": "hi", "origin": "realUser" }),
        json!({ "rowId": 3, "turnId": "turn-1", "createdAt": 1, "createdAtSeq": 3, "kind": "assistantText", "text": "yo", "state": "complete" }),
        json!({ "rowId": 4, "turnId": "turn-1", "createdAt": 1, "createdAtSeq": 4, "kind": "reasoning", "text": "think", "state": "complete" }),
        json!({ "rowId": 5, "turnId": "turn-1", "createdAt": 1, "createdAtSeq": 5, "kind": "toolCall", "toolCallId": "c", "toolName": "Bash", "status": "running", "inputText": "" }),
        json!({ "rowId": 6, "turnId": "turn-1", "createdAt": 1, "createdAtSeq": 6, "kind": "artifact", "artifactVersionId": "v", "logicalArtifactKey": "k", "displayName": "n", "artifactType": "pdf", "mimeType": "application/pdf", "sizeBytes": 1, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "ref": "r", "state": "current" }),
        json!({ "rowId": 7, "turnId": "turn-1", "createdAt": 1, "createdAtSeq": 7, "kind": "subagent", "subagentType": "explore", "status": "running", "summaryText": "" }),
        json!({ "rowId": 8, "turnId": "turn-1", "createdAt": 1, "createdAtSeq": 8, "kind": "hookInvocation", "hookInvocationId": "h", "hookEventName": "Stop", "hookCount": 1, "state": "running", "startedAt": 1, "lane": "assistantWork", "executions": [] }),
        { let mut row = base(9, "timelineMarker"); row["marker"] = json!({ "type": "checkpointRestored", "checkpointId": "cp" }); row },
    ];

    let mut kinds = Vec::new();
    for row in rows {
        let parsed: ConversationRow = serde_json::from_value(row.clone())
            .unwrap_or_else(|error| panic!("解不开行 {row}: {error}"));
        let label = match &parsed {
            ConversationRow::TurnHeader(_) => "turnHeader",
            ConversationRow::UserInput(_) => "userInput",
            ConversationRow::AssistantText(_) => "assistantText",
            ConversationRow::Reasoning(_) => "reasoning",
            ConversationRow::ToolCall(_) => "toolCall",
            ConversationRow::Artifact(_) => "artifact",
            ConversationRow::Subagent(_) => "subagent",
            ConversationRow::HookInvocation(_) => "hookInvocation",
            ConversationRow::TimelineMarker(_) => "timelineMarker",
        };
        assert_eq!(label, row["kind"]);
        kinds.push(label);
    }
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), 9);
}
