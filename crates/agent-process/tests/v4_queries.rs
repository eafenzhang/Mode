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
    PlansParams, RowsRangeParams, WorkflowRunEventsParams, WorkflowRunsQueryParams,
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
