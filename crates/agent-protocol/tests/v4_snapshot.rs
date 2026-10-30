//! v4 `conversationSnapshot` 的 round-trip 与关键字段解析。

use agent_protocol::v4_snapshot::{
    ConversationSnapshot, GoalStatus, PendingInteractionKind, SharedContextImportState,
};
use serde_json::json;

#[test]
fn full_snapshot_round_trips() {
    let value = json!({
        "protocolVersion": 1,
        "sessionId": "sess-1",
        "logEpoch": "epoch-1",
        "seq": 10,
        "revision": 2,
        "control": {
            "phase": "running",
            "sessionEnded": false,
            "canStop": true,
            "stopState": "stoppable",
            "stopTargetKind": "assistant",
            "activeWorks": [{ "kind": "primaryTurn", "startedAt": 1 }],
            "lastError": null,
            "apiRetry": null
        },
        "availability": {
            "fork": { "allowed": true },
            "compact": { "allowed": false, "reasonCode": "busy" },
            "switchModelConfig": { "allowed": true },
            "setFollowupMode": { "allowed": true },
            "queueEdit": { "allowed": true },
            "sendQueuedNow": { "allowed": true },
            "pauseGoal": { "allowed": false, "reasonCode": "no-goal" },
            "resumeGoal": { "allowed": false, "reasonCode": "no-goal" }
        },
        "inputRouting": { "mode": "startNow" },
        "meta": { "title": "标题", "titleSource": "custom" },
        "sharedContextImport": {
            "contextId": "ctx-1",
            "title": "共享",
            "shareUrl": "https://example.com/cn/share/abc",
            "status": "attached"
        },
        "config": {
            "provider": "zai",
            "model": "glm-4.6",
            "thought": "high",
            "thoughtLevels": ["high"],
            "followupMode": "queue",
            "mode": "build"
        },
        "modelTransition": null,
        "usage": {
            "contextWindow": {
                "usedTokens": 10,
                "maxTokens": 1000,
                "autoCompactThresholdTokens": null
            },
            "cumulative": {
                "inputTokens": 10,
                "outputTokens": 5,
                "cacheReadTokens": 0,
                "cacheWriteTokens": 0
            }
        },
        "queue": {
            "items": [{
                "sourceCommandId": "cmd-1",
                "queueItemId": "q-1",
                "clientId": "client-1",
                "kind": "sendText",
                "text": "排队消息",
                "attachments": [],
                "delivery": { "requested": "auto", "admitted": "queue" },
                "order": { "admissionSeq": 1, "queuePosition": 0 },
                "steer": { "state": "notRequested" },
                "dispatch": { "state": "queued" },
                "admittedAt": 1700000000000u64
            }],
            "autoDrain": true
        },
        "pendingInteractions": [{
            "interactionId": "perm-1",
            "kind": "permission",
            "anchorRowId": null,
            "createdAt": 1700000000000u64,
            "payload": {
                "kind": "permission",
                "toolCallId": "call-1",
                "toolName": "Bash",
                "summary": "执行命令",
                "detail": { "command": "ls" },
                "options": [{ "optionId": "allow_once", "label": "允许一次", "kind": "allowOnce" }]
            }
        }, {
            "interactionId": "ui-1",
            "kind": "userInput",
            "anchorRowId": 1,
            "createdAt": 1700000000001u64,
            "payload": {
                "kind": "userInput",
                "prompt": "选择",
                "freeText": false,
                "options": [{ "optionId": "a", "label": "A" }]
            }
        }],
        "pendingCommands": [{
            "commandId": "cmd-1",
            "clientId": "client-1",
            "type": "sendText",
            "state": "accepted",
            "at": 1
        }],
        "backgroundWorks": [{
            "workId": "work-1",
            "kind": "bash",
            "title": "后台命令",
            "status": "running",
            "startedAt": 1,
            "anchorRowId": null
        }],
        "subagents": {
            "revision": 1,
            "childSessionIds": ["sess-child"],
            "running": [],
            "endedTotal": 0
        },
        "workflowRuns": { "revision": 1, "runs": [] },
        "goal": null,
        "plan": {
            "items": [{ "id": "1", "content": "计划", "status": "pending" }],
            "updatedAt": 1
        },
        "workspaceHookAdmission": null,
        "rows": {
            "window": [{
                "rowId": 1,
                "turnId": "turn-1",
                "createdAt": 1,
                "createdAtSeq": 1,
                "kind": "userInput",
                "text": "你好",
                "origin": "realUser"
            }],
            "totalCount": 1,
            "firstRowId": 1
        }
    });

    let snapshot: ConversationSnapshot = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(snapshot.session_id, "sess-1");
    assert_eq!(snapshot.pending_interactions.len(), 2);
    assert_eq!(
        snapshot.pending_interactions[0].kind,
        PendingInteractionKind::Permission
    );
    assert!(matches!(
        snapshot.shared_context_import,
        Some(SharedContextImportState::V2(_))
    ));
    assert_eq!(snapshot.rows.window.len(), 1);
    assert!(snapshot.workflow_runs.is_some());
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap(),
        value,
        "镜像的序列化结果与线上快照不一致（字段名/缺省处理有偏差）"
    );
}

#[test]
fn goal_state_round_trips_with_defaults_and_nullable_title() {
    let value = json!({
        "protocolVersion": 1,
        "sessionId": "sess-1",
        "logEpoch": "e",
        "seq": 1,
        "revision": 0,
        "control": {
            "phase": "draft",
            "sessionEnded": false,
            "canStop": false,
            "stopState": "idle",
            "stopTargetKind": "unknown",
            "activeWorks": [],
            "lastError": null,
            "apiRetry": null
        },
        "availability": {
            "fork": { "allowed": true },
            "compact": { "allowed": true },
            "switchModelConfig": { "allowed": true },
            "setFollowupMode": { "allowed": true },
            "queueEdit": { "allowed": true },
            "sendQueuedNow": { "allowed": true },
            "pauseGoal": { "allowed": true },
            "resumeGoal": { "allowed": true }
        },
        "inputRouting": { "mode": "enqueue" },
        "meta": { "title": "", "titleSource": "default" },
        "config": {
            "provider": "zai",
            "model": "glm-4.6",
            "thought": "low",
            "thoughtLevels": [],
            "followupMode": "guide",
            "mode": "build"
        },
        "modelTransition": null,
        "usage": {
            "contextWindow": null,
            "cumulative": {
                "inputTokens": 0,
                "outputTokens": 0,
                "cacheReadTokens": 0,
                "cacheWriteTokens": 0
            }
        },
        "queue": { "items": [], "autoDrain": false },
        "pendingInteractions": [],
        "pendingCommands": [],
        "backgroundWorks": [],
        "goal": {
            "targetId": "",
            "objective": "目标",
            "summaryTitle": null,
            "timeUsedSeconds": 0,
            "activeRunStartedAtMs": null,
            "status": "active",
            "iteration": 1,
            "verifications": [],
            "iterations": []
        },
        "plan": null,
        "workspaceHookAdmission": { "pendingCount": 1, "bundleDigest": "d" },
        "rows": { "window": [], "totalCount": 0, "firstRowId": null }
    });

    let snapshot: ConversationSnapshot = serde_json::from_value(value.clone()).unwrap();
    let goal = snapshot.goal.as_ref().unwrap();
    assert_eq!(goal.status, GoalStatus::Active);
    assert_eq!(goal.summary_title, Some(None));
    assert!(snapshot.plan.is_none());
    assert!(snapshot.workspace_hook_admission.is_some());
    assert_eq!(serde_json::to_value(&snapshot).unwrap(), value);
}
