//! 工作流只读面：list / get / runs 的参数、联合结果与 limit 校验。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_process::{
    AgentControlClient, AgentProcessSpec, AgentRuntimeRegistry, AgentWorkspaceTarget, ControlError,
    SpecFactory,
};
use agent_protocol::workflow::{
    SavedWorkflowFailureReason, SavedWorkflowMeta, SavedWorkflowScope, WorkflowRunStatus,
    WorkflowsDeleteResult, WorkflowsGetResult, WorkflowsMoveResult, WorkflowsUpdateMetaResult,
};
use serde_json::Value;

const FIXTURE: &str = env!("CARGO_BIN_EXE_agent-process-fixture");

struct FrameLog(PathBuf);

impl FrameLog {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "mode-workflows-{label}-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn frame_params(&self, method: &str) -> Option<Value> {
        let marker = format!("frame\t{method}\t");
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                if let Some(line) = text.lines().find(|line| line.starts_with(&marker)) {
                    return line.splitn(3, '\t').nth(2)?.parse().ok();
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
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

#[tokio::test]
async fn lists_saved_workflows_with_scope() {
    let log = FrameLog::new("list");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let result = client
        .list_saved_workflows(
            &AgentWorkspaceTarget::new("/ws/demo"),
            Some(SavedWorkflowScope::Global),
        )
        .await
        .unwrap();
    assert_eq!(result.workflows.len(), 1);
    assert_eq!(result.workflows[0].name, "demo");
    assert_eq!(result.dir, "/ws/demo/.mode/workflows");

    let params = log.frame_params("workflows/list").expect("必须记录到请求帧");
    assert_eq!(params["workspace"]["workspacePath"], "/ws/demo");
    assert_eq!(params["scope"], "global");
}

#[tokio::test]
async fn gets_workflow_ok_and_failure_union() {
    let log = FrameLog::new("get");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);
    let result = client
        .get_saved_workflow(&AgentWorkspaceTarget::new("/ws/demo"), "demo", None)
        .await
        .unwrap();
    match result {
        WorkflowsGetResult::Ok(ok) => {
            assert_eq!(ok.name, "demo");
            assert_eq!(ok.script, "print(1)");
            assert_eq!(ok.meta.description, "演示工作流");
        }
        other => panic!("unexpected result: {other:?}"),
    }
    let params = log.frame_params("workflows/get").expect("必须记录到请求帧");
    assert_eq!(params["name"], "demo");
    assert!(params.get("scope").is_none(), "scope 缺省时不能出现键");

    let log = FrameLog::new("get-failure");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("workflow-get-failure", &log, spawns);
    let result = client
        .get_saved_workflow(&AgentWorkspaceTarget::new("/ws/demo"), "missing", None)
        .await
        .unwrap();
    match result {
        WorkflowsGetResult::Failure(failure) => {
            assert_eq!(failure.reason, SavedWorkflowFailureReason::NotFound);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[tokio::test]
async fn lists_runs_with_artifacts_and_rejects_bad_limit() {
    let log = FrameLog::new("runs");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);

    let result = client
        .list_saved_workflow_runs(
            &AgentWorkspaceTarget::new("/ws/demo"),
            Some("demo"),
            10,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result.runs.len(), 1);
    assert_eq!(result.runs[0].status, WorkflowRunStatus::Completed);
    assert_eq!(result.runs[0].artifacts.as_ref().unwrap()[0].id, "art-1");
    assert!(result.truncated.is_some());

    let params = log.frame_params("workflows/runs").expect("必须记录到请求帧");
    assert_eq!(params["name"], "demo");
    assert_eq!(params["limit"], 10);

    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, Arc::clone(&spawns));
    let error = client
        .list_saved_workflow_runs(&AgentWorkspaceTarget::new("/ws/demo"), None, 0, None)
        .await
        .unwrap_err();
    match error {
        ControlError::InvalidParams(message) => {
            assert!(message.contains("limit must be in 1..=50"), "{message}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert_eq!(spawns.load(Ordering::SeqCst), 0, "参数非法不得拉起 Agent");
}

#[tokio::test]
async fn updates_deletes_and_moves_workflow() {
    let log = FrameLog::new("write");
    let spawns = Arc::new(AtomicUsize::new(0));
    let client = control("", &log, spawns);
    let target = AgentWorkspaceTarget::new("/ws/demo");

    let meta = SavedWorkflowMeta {
        description: "新描述".to_string(),
        when_to_use: None,
        args: None,
    };
    let updated = client
        .update_saved_workflow_meta(&target, "demo", meta, Some(SavedWorkflowScope::Project))
        .await
        .unwrap();
    match updated {
        WorkflowsUpdateMetaResult::Ok(ok) => assert_eq!(ok.path, "/ws/demo/.mode/workflows/demo.md"),
        other => panic!("unexpected result: {other:?}"),
    }
    let params = log.frame_params("workflows/updateMeta").expect("必须记录到请求帧");
    assert_eq!(params["name"], "demo");
    assert_eq!(params["meta"]["description"], "新描述");
    assert_eq!(params["scope"], "project");

    let deleted = client
        .delete_saved_workflow(&target, "demo", None)
        .await
        .unwrap();
    match deleted {
        WorkflowsDeleteResult::Ok(ok) => assert_eq!(ok.path, "/ws/demo/.mode/workflows/demo.md"),
        other => panic!("unexpected result: {other:?}"),
    }
    let params = log.frame_params("workflows/delete").expect("必须记录到请求帧");
    assert!(params.get("scope").is_none());

    let moved = client.move_saved_workflow(&target, "demo").await.unwrap();
    match moved {
        WorkflowsMoveResult::Ok(ok) => {
            assert_eq!(ok.from, "/home/u/.mode/workflows/demo.md");
            assert_eq!(ok.to, "/ws/demo/.mode/workflows/demo.md");
        }
        other => panic!("unexpected result: {other:?}"),
    }
    let params = log.frame_params("workflows/move").expect("必须记录到请求帧");
    assert_eq!(params["workspace"]["workspacePath"], "/ws/demo");
}

#[tokio::test]
async fn malformed_workflow_results_are_rejected() {
    let cases = [
        ("bad-workflows-list", "invalid workflows/list result"),
        ("bad-workflow-get", "invalid workflows/get result"),
        ("bad-workflow-runs", "invalid workflows/runs result"),
        ("bad-workflow-update-meta", "invalid workflows/updateMeta result"),
        ("bad-workflow-delete", "invalid workflows/delete result"),
        ("bad-workflow-move", "invalid workflows/move result"),
    ];
    for (mode, prefix) in cases {
        let log = FrameLog::new(mode);
        let spawns = Arc::new(AtomicUsize::new(0));
        let client = control(mode, &log, spawns);
        let target = AgentWorkspaceTarget::new("/ws/demo");
        let error = match mode {
            "bad-workflows-list" => client.list_saved_workflows(&target, None).await.unwrap_err(),
            "bad-workflow-get" => {
                client.get_saved_workflow(&target, "demo", None).await.unwrap_err()
            }
            "bad-workflow-runs" => {
                client
                    .list_saved_workflow_runs(&target, None, 5, None)
                    .await
                    .unwrap_err()
            }
            "bad-workflow-update-meta" => {
                client
                    .update_saved_workflow_meta(
                        &target,
                        "demo",
                        SavedWorkflowMeta {
                            description: "d".to_string(),
                            when_to_use: None,
                            args: None,
                        },
                        None,
                    )
                    .await
                    .unwrap_err()
            }
            "bad-workflow-delete" => {
                client.delete_saved_workflow(&target, "demo", None).await.unwrap_err()
            }
            _ => client.move_saved_workflow(&target, "demo").await.unwrap_err(),
        };
        match error {
            ControlError::InvalidResult(message) => {
                assert!(message.contains(prefix), "{mode}: {message}");
            }
            other => panic!("{mode}: unexpected error: {other:?}"),
        }
    }
}
