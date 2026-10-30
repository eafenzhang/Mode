//! v4 `workflowRuns` 运行态 round-trip。

use agent_protocol::v4_workflow_runs::{WorkflowRunNodePhase, WorkflowRunsState};
use serde_json::json;

#[test]
fn workflow_runs_state_round_trips() {
    let value = json!({
        "revision": 3,
        "runs": [{
            "runId": "run-1",
            "toolCallId": "launch-1",
            "status": "running",
            "resumedFrom": "run-0",
            "usage": { "spentTokens": 100, "nodesUsed": 2, "nodesUnlisted": 1 },
            "resumable": true,
            "resultPreview": "部分结果",
            "actors": [{
                "siteId": "site-1",
                "ordinal": 0,
                "name": "reader-1",
                "sessionId": "sess-child",
                "status": "running",
                "phaseName": "scan"
            }],
            "nodes": [{
                "siteId": "site-1",
                "ordinal": 0,
                "kind": "ask",
                "phase": "executing",
                "actorSiteId": "site-1",
                "actorOrdinal": 0,
                "phaseName": "scan",
                "instructionsHead": "读文件",
                "turn": 1,
                "toolCalls": 2,
                "lastTool": { "name": "Read", "target": "a.txt" }
            }],
            "reports": [{ "siteId": "site-1", "ordinal": 1, "preview": "进度", "artifactId": "art-1" }],
            "pendingQuestions": [{
                "qid": "dwfq-1",
                "actorSiteId": "site-1",
                "actorOrdinal": 0,
                "actorName": "reader-1",
                "question": "缺哪个文件？",
                "context": "上下文",
                "askedAt": 1700000000000u64
            }],
            "concurrency": { "key": "zai/glm-4.6", "cap": 4, "ceiling": 8, "limit": 4, "cooldownMs": 1000 },
            "concurrencyCeiling": 8,
            "subagentModel": "zai/glm-4.6$high",
            "artifacts": [{
                "id": "art-1",
                "kind": "markdown",
                "title": "报告",
                "version": 1,
                "contentType": "text/markdown",
                "bytes": 12,
                "itemCount": 1,
                "primary": true
            }],
            "phases": [{ "name": "scan", "rounds": 1 }],
            "currentPhase": "scan",
            "phaseNames": ["scan", "write"],
            "phaseAlongside": [[], [0]],
            "unlistedByPhase": [{ "phaseName": "scan", "actors": 1, "settled": 0 }],
            "truncated": true,
            "lastEventSequence": 42
        }]
    });

    let state: WorkflowRunsState = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(state.runs.len(), 1);
    let run = &state.runs[0];
    assert_eq!(run.nodes[0].phase, WorkflowRunNodePhase::Executing);
    assert_eq!(run.actors.len(), 1);
    assert!(run.reports.is_some());
    assert!(run.pending_questions.is_some());
    assert_eq!(
        serde_json::to_value(&state).unwrap(),
        value,
        "镜像的序列化结果与线上状态不一致（字段名/缺省处理有偏差）"
    );
}

#[test]
fn empty_runs_state_round_trips() {
    let value = json!({ "revision": 0, "runs": [] });
    let state: WorkflowRunsState = serde_json::from_value(value.clone()).unwrap();
    assert!(state.runs.is_empty());
    assert_eq!(serde_json::to_value(&state).unwrap(), value);
}
