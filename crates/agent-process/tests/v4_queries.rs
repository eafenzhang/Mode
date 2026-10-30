//! v4 只读查询：`rowsRange` 与 `plans` 的参数形状与结果解析。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    RuntimePolicy, SpecFactory,
};
use agent_protocol::v4_queries::{
    PlansParams, RowsRangeParams, WorkflowRunArtifactDataParams, WorkflowRunArtifactReadParams,
    WorkflowRunArtifactsParams, WorkflowRunEventsParams, WorkflowRunNodeResultParams,
    WorkflowRunWorkspaceParams, WorkflowRunsQueryParams,
};
use agent_protocol::v4_transport::V4ClientMode;
use serde_json::{json, Value};

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-v4-query-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn frame_params(&self, method: &str) -> Vec<Value> {
        let marker = format!("frame\t{method}\t");
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                let params: Vec<Value> = text
                    .lines()
                    .filter(|line| line.starts_with(&marker))
                    .filter_map(|line| line.splitn(3, '\t').nth(2)?.parse().ok())
                    .collect();
                if !params.is_empty() {
                    return params;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Vec::new()
    }
}

impl Drop for FrameLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn control(mode: &str, log: &FrameLog, spawns: Arc<AtomicUsize>) -> AgentControlClient {
    let mode = mode.to_string();
    let log = log.0.to_string_lossy().into_owned();
    let factory: SpecFactory = Arc::new(move |_target| {
        spawns.fetch_add(1, Ordering::SeqCst);
        let mut spec = AgentProcessSpec::new(FIXTURE);
        if !mode.is_empty() {
            spec.args.push(mode.clone());
        }
        spec.env
            .push(("AGENT_FIXTURE_FRAME_LOG".to_string(), log.clone()));
        Ok(spec)
    });
    AgentControlClient::new(Arc::new(AgentRuntimeRegistry::new(factory)))
}

fn target() -> AgentWorkspaceTarget {
    AgentWorkspaceTarget::new("/ws/demo")
}

fn rows_params(before_row_id: Option<u64>) -> RowsRangeParams {
    RowsRangeParams {
        session_id: "sess-1".to_string(),
        client_mode: Some(V4ClientMode::DesktopContinuous),
        before_row_id: before_row_id.map(serde_json::Number::from),
        limit: serde_json::Number::from(50),
    }
}

#[tokio::test]
async fn rows_range_sends_trusted_client_mode_and_omits_absent_before_row_id() {
    let log = FrameLog::new("rows");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let result = client
        .conversation_rows_range_v4(&target(), rows_params(None), RuntimePolicy::StartIfNeeded)
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    assert!(result.has_more);
    assert_eq!(result.at_log_epoch, "epoch-1");

    let frames = log.frame_params("v4/conversation/rowsRange");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["sessionId"], "sess-1");
    assert_eq!(frames[0]["clientMode"], "desktop-continuous");
    assert_eq!(frames[0]["limit"], 50);
    assert!(
        frames[0].get("beforeRowId").is_none(),
        "beforeRowId 缺省时不得出键"
    );

    client
        .conversation_rows_range_v4(&target(), rows_params(Some(10)), RuntimePolicy::StartIfNeeded)
        .await
        .unwrap();
    let frames = log.frame_params("v4/conversation/rowsRange");
    assert_eq!(frames[1]["beforeRowId"], 10);
}

#[tokio::test]
async fn plans_parses_tool_call_rows() {
    let log = FrameLog::new("plans");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let result = client
        .conversation_plans_v4(
            &target(),
            PlansParams {
                session_id: "sess-1".to_string(),
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap();
    assert_eq!(result.plans.len(), 1);
    assert_eq!(result.plans[0].tool_call_id, "call-1");
    assert_eq!(result.plans[0].tool_name, "ExitPlanMode");

    let frames = log.frame_params("v4/conversation/plans");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0], json!({ "sessionId": "sess-1" }));
}

#[tokio::test]
async fn invalid_results_carry_distinct_prefixes() {
    let rows_log = FrameLog::new("bad-rows");
    let client = control("bad-v4-rows-range", &rows_log, Arc::new(AtomicUsize::new(0)));
    let error = client
        .conversation_rows_range_v4(&target(), rows_params(None), RuntimePolicy::StartIfNeeded)
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => assert!(
            message.starts_with("invalid v4/conversation/rowsRange result"),
            "{message}"
        ),
        other => panic!("unexpected error: {other:?}"),
    }

    let plans_log = FrameLog::new("bad-plans");
    let client = control("bad-v4-plans", &plans_log, Arc::new(AtomicUsize::new(0)));
    let error = client
        .conversation_plans_v4(
            &target(),
            PlansParams {
                session_id: "sess-1".to_string(),
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => assert!(
            message.starts_with("invalid v4/conversation/plans result"),
            "{message}"
        ),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn workflow_run_events_page_parses_and_omits_absent_cursor() {
    let log = FrameLog::new("run-events");
    let client = control("", &log, Arc::new(AtomicUsize::new(0)));

    let result = client
        .conversation_workflow_run_events_v4(
            &target(),
            WorkflowRunEventsParams {
                session_id: "sess-1".to_string(),
                run_id: "run-1".to_string(),
                after_sequence: None,
                limit: None,
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap();
    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].event_type, "phase.started");
    assert_eq!(result.events[0].payload["phase"], "build");
    assert!(result.has_more);

    let frames = log.frame_params("v4/conversation/workflowRunEvents");
    assert_eq!(
        frames[0],
        json!({ "sessionId": "sess-1", "runId": "run-1" }),
        "游标缺省时不得出键"
    );

    client
        .conversation_workflow_run_events_v4(
            &target(),
            WorkflowRunEventsParams {
                session_id: "sess-1".to_string(),
                run_id: "run-1".to_string(),
                after_sequence: Some(6),
                limit: Some(50),
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap();
    let frames = log.frame_params("v4/conversation/workflowRunEvents");
    assert_eq!(frames[1]["afterSequence"], 6);
    assert_eq!(frames[1]["limit"], 50);
}

#[tokio::test]
async fn workflow_runs_enumeration_parses_journal_summaries() {
    let log = FrameLog::new("runs-enum");
    let client = control("", &log, Arc::new(AtomicUsize::new(0)));

    let result = client
        .conversation_workflow_runs_v4(
            &target(),
            WorkflowRunsQueryParams {
                session_id: "sess-1".to_string(),
                limit: Some(10),
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap();
    assert_eq!(result.runs.len(), 1);
    let run = &result.runs[0];
    assert_eq!(run.status, agent_protocol::v4_workflow_runs::WorkflowRunStatus::Stopped);
    assert_eq!(
        run.stop_reason,
        Some(agent_protocol::v4_workflow_runs::WorkflowRunStopReason::Superseded)
    );
    assert_eq!(run.superseded_by.as_deref(), Some("run-2"));
    assert_eq!(run.failure_code.as_deref(), Some("Interrupted"));
    assert!(run.resumable);

    let frames = log.frame_params("v4/conversation/workflowRuns");
    assert_eq!(frames[0], json!({ "sessionId": "sess-1", "limit": 10 }));
}

#[tokio::test]
async fn invalid_workflow_queries_carry_distinct_prefixes() {
    let events_log = FrameLog::new("bad-run-events");
    let client = control(
        "bad-v4-workflow-run-events",
        &events_log,
        Arc::new(AtomicUsize::new(0)),
    );
    let error = client
        .conversation_workflow_run_events_v4(
            &target(),
            WorkflowRunEventsParams {
                session_id: "sess-1".to_string(),
                run_id: "run-1".to_string(),
                after_sequence: None,
                limit: None,
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => assert!(
            message.starts_with("invalid v4/conversation/workflowRunEvents result"),
            "{message}"
        ),
        other => panic!("unexpected error: {other:?}"),
    }

    let runs_log = FrameLog::new("bad-runs-enum");
    let client = control("bad-v4-workflow-runs", &runs_log, Arc::new(AtomicUsize::new(0)));
    let error = client
        .conversation_workflow_runs_v4(
            &target(),
            WorkflowRunsQueryParams {
                session_id: "sess-1".to_string(),
                limit: None,
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => assert!(
            message.starts_with("invalid v4/conversation/workflowRuns result"),
            "{message}"
        ),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn published_artifacts_list_parses_versions_and_primary() {
    let log = FrameLog::new("artifacts");
    let client = control("", &log, Arc::new(AtomicUsize::new(0)));

    let result = client
        .conversation_workflow_run_artifacts_v4(
            &target(),
            WorkflowRunArtifactsParams {
                session_id: "sess-1".to_string(),
                run_id: "run-1".to_string(),
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap();
    assert_eq!(result.artifacts.len(), 1);
    let artifact = &result.artifacts[0];
    assert_eq!(artifact.id, "report");
    assert_eq!(artifact.version, 2);
    assert_eq!(artifact.versions.len(), 1);
    assert_eq!(artifact.versions[0].published_at, 1700000000000);
    assert_eq!(
        artifact.versions[0].uri.as_deref(),
        Some("mode-artifact://run-1/report")
    );
    assert_eq!(artifact.item_count, 0);
    assert!(artifact.primary.is_some());

    let frames = log.frame_params("v4/conversation/workflowRunArtifacts");
    assert_eq!(
        frames[0],
        json!({ "sessionId": "sess-1", "runId": "run-1" })
    );
}

#[tokio::test]
async fn artifact_data_page_parses_items_and_reads_chunk() {
    let log = FrameLog::new("artifact-data");
    let client = control("", &log, Arc::new(AtomicUsize::new(0)));

    let data = client
        .conversation_workflow_run_artifact_data_v4(
            &target(),
            WorkflowRunArtifactDataParams {
                session_id: "sess-1".to_string(),
                run_id: "run-1".to_string(),
                artifact_id: "report".to_string(),
                after_sequence: None,
                limit: Some(200),
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap();
    assert_eq!(data.items.len(), 1);
    assert_eq!(data.items[0].site_id, "report#1");
    assert_eq!(data.items[0].item["timing"]["after"], 12);
    assert!(!data.has_more);

    let frames = log.frame_params("v4/conversation/workflowRunArtifactData");
    assert!(
        frames[0].get("afterSequence").is_none(),
        "afterSequence 缺省时不得出键"
    );
    assert_eq!(frames[0]["limit"], 200);

    let chunk = client
        .conversation_workflow_run_artifact_read_v4(
            &target(),
            WorkflowRunArtifactReadParams {
                session_id: "sess-1".to_string(),
                run_id: "run-1".to_string(),
                artifact_id: "report".to_string(),
                version: 2,
                offset: 0,
                limit: 65536,
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap();
    assert_eq!(chunk.data_base64, "aGVsbG8=");
    assert_eq!(chunk.media_type, "text/markdown");
    assert_eq!(chunk.total_bytes, 5);
    assert_eq!(chunk.next_offset, None);
}

#[tokio::test]
async fn invalid_artifact_queries_carry_distinct_prefixes() {
    let cases: [(&str, &str); 3] = [
        ("bad-v4-artifacts", "invalid v4/conversation/workflowRunArtifacts result"),
        (
            "bad-v4-artifact-data",
            "invalid v4/conversation/workflowRunArtifactData result",
        ),
        (
            "bad-v4-artifact-read",
            "invalid v4/conversation/workflowRunArtifactRead result",
        ),
    ];
    for (mode, prefix) in cases {
        let log = FrameLog::new(mode);
        let client = control(mode, &log, Arc::new(AtomicUsize::new(0)));
        let error = match mode {
            "bad-v4-artifacts" => client
                .conversation_workflow_run_artifacts_v4(
                    &target(),
                    WorkflowRunArtifactsParams {
                        session_id: "sess-1".to_string(),
                        run_id: "run-1".to_string(),
                    },
                    RuntimePolicy::StartIfNeeded,
                )
                .await
                .unwrap_err(),
            "bad-v4-artifact-data" => client
                .conversation_workflow_run_artifact_data_v4(
                    &target(),
                    WorkflowRunArtifactDataParams {
                        session_id: "sess-1".to_string(),
                        run_id: "run-1".to_string(),
                        artifact_id: "report".to_string(),
                        after_sequence: None,
                        limit: None,
                    },
                    RuntimePolicy::StartIfNeeded,
                )
                .await
                .unwrap_err(),
            _ => client
                .conversation_workflow_run_artifact_read_v4(
                    &target(),
                    WorkflowRunArtifactReadParams {
                        session_id: "sess-1".to_string(),
                        run_id: "run-1".to_string(),
                        artifact_id: "report".to_string(),
                        version: 2,
                        offset: 0,
                        limit: 1,
                    },
                    RuntimePolicy::StartIfNeeded,
                )
                .await
                .unwrap_err(),
        };
        match error {
            ControlError::InvalidResult(message) => {
                assert!(message.starts_with(prefix), "{mode}: {message}")
            }
            other => panic!("{mode}: unexpected error: {other:?}"),
        }
    }
}

#[tokio::test]
async fn workspace_transcript_and_node_result_parse() {
    let log = FrameLog::new("workspace");
    let client = control("", &log, Arc::new(AtomicUsize::new(0)));

    let nodes = client
        .conversation_workflow_run_workspace_v4(
            &target(),
            WorkflowRunWorkspaceParams {
                session_id: "sess-1".to_string(),
                run_id: "run-1".to_string(),
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap();
    assert_eq!(nodes.nodes.len(), 1);
    let node = &nodes.nodes[0];
    assert_eq!(node.kind, agent_protocol::v4_queries::WorkspaceNodeKind::WorldRun);
    assert_eq!(node.op.as_deref(), Some("files.write"));
    assert_eq!(node.args.as_ref().map(Vec::len), Some(2));
    assert_eq!(node.summary.as_ref().map(|s| s.exit_code), Some(Some(0)));
    assert_eq!(nodes.truncated, Some(true));

    let result = client
        .conversation_workflow_run_node_result_v4(
            &target(),
            WorkflowRunNodeResultParams {
                session_id: "sess-1".to_string(),
                run_id: "run-1".to_string(),
                site_id: "files.write#1".to_string(),
                ordinal: 0,
                max_bytes: None,
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap();
    assert_eq!(
        result.status,
        agent_protocol::v4_queries::WorkspaceNodeStatus::Completed
    );
    assert_eq!(result.result.as_ref().unwrap()["lines"][0], "a");
    assert!(!result.truncated);
    assert_eq!(result.total_bytes, 24);

    let frames = log.frame_params("v4/conversation/workflowRunNodeResult");
    assert!(
        frames[0].get("maxBytes").is_none(),
        "maxBytes 缺省时不得出键"
    );
}

#[tokio::test]
async fn invalid_workspace_queries_carry_distinct_prefixes() {
    let ws_log = FrameLog::new("bad-workspace");
    let client = control("bad-v4-run-workspace", &ws_log, Arc::new(AtomicUsize::new(0)));
    let error = client
        .conversation_workflow_run_workspace_v4(
            &target(),
            WorkflowRunWorkspaceParams {
                session_id: "sess-1".to_string(),
                run_id: "run-1".to_string(),
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => assert!(
            message.starts_with("invalid v4/conversation/workflowRunWorkspace result"),
            "{message}"
        ),
        other => panic!("unexpected error: {other:?}"),
    }

    let node_log = FrameLog::new("bad-node");
    let client = control("bad-v4-node-result", &node_log, Arc::new(AtomicUsize::new(0)));
    let error = client
        .conversation_workflow_run_node_result_v4(
            &target(),
            WorkflowRunNodeResultParams {
                session_id: "sess-1".to_string(),
                run_id: "run-1".to_string(),
                site_id: "files.write#1".to_string(),
                ordinal: 0,
                max_bytes: Some(1024),
            },
            RuntimePolicy::StartIfNeeded,
        )
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidResult(message) => assert!(
            message.starts_with("invalid v4/conversation/workflowRunNodeResult result"),
            "{message}"
        ),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn existing_only_does_not_spawn_for_queries() {
    let log = FrameLog::new("existing");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));

    let error = client
        .conversation_rows_range_v4(&target(), rows_params(None), RuntimePolicy::ExistingOnly)
        .await
        .unwrap_err();
    assert!(matches!(error, ControlError::RuntimeUnavailable { .. }));
    assert_eq!(spawns.load(Ordering::SeqCst), 0);

    client
        .conversation_rows_range_v4(&target(), rows_params(None), RuntimePolicy::StartIfNeeded)
        .await
        .unwrap();
    client
        .conversation_rows_range_v4(&target(), rows_params(None), RuntimePolicy::ExistingOnly)
        .await
        .unwrap();
    assert_eq!(spawns.load(Ordering::SeqCst), 1);
}
