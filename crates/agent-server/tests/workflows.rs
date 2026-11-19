//! C4 T3：Workflows 文件读写与 run 读面的集成测试。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §4 与 `fixtures/c4/workflows-*.json`。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use agent_protocol::workflow::{
    SavedWorkflowArgType, SavedWorkflowMeta, SavedWorkflowScope, WorkflowRunStatus,
    WorkflowsDeleteResult, WorkflowsGetResult, WorkflowsMoveResult, WorkflowsUpdateMetaResult,
};
use agent_server::{parse_workflow, render_workflow, WorkflowDirs};
use std::collections::BTreeMap;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-agent-server-workflows-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const SAVED: &str = "/* mode-workflow\ndescription: probe workflow saved by tool\nwhenToUse: use when the C4 probe runs the saved workflow\n*/\nreturn { ok: true };";

fn dirs(root: &TempRoot) -> WorkflowDirs {
    WorkflowDirs::new(
        root.path().join("ws").to_str().expect("ws"),
        &root.path().join("data"),
    )
}

fn write_project(root: &TempRoot, name: &str, body: &str) -> PathBuf {
    let dir = dirs(root).project;
    fs::create_dir_all(&dir).expect("create workflows dir");
    let path = dir.join(format!("{name}.dwf.ts"));
    fs::write(&path, body).expect("write workflow");
    path
}

#[test]
fn parse_and_render_round_trip_saved_bytes() {
    let parsed = parse_workflow(SAVED).expect("parse saved workflow");
    assert_eq!(parsed.meta.description, "probe workflow saved by tool");
    assert_eq!(
        parsed.meta.when_to_use.as_deref(),
        Some("use when the C4 probe runs the saved workflow")
    );
    assert_eq!(parsed.script, "return { ok: true };");
    let rendered = render_workflow(&parsed.meta, &parsed.script);
    assert_eq!(rendered, SAVED);

    let with_args = SavedWorkflowMeta {
        description: "probe workflow saved by tool (meta updated)".to_string(),
        when_to_use: Some("updated whenToUse".to_string()),
        args: Some(BTreeMap::from([(
            "target".to_string(),
            agent_protocol::workflow::SavedWorkflowArgDeclaration {
                arg_type: SavedWorkflowArgType::String,
                description: Some("probe arg".to_string()),
                required: Some(false),
                default: Some(Some(serde_json::json!("x"))),
            },
        )])),
    };
    let rendered = render_workflow(&with_args, "return { ok: true };");
    let reparsed = parse_workflow(&rendered).expect("reparse");
    assert_eq!(reparsed.meta, with_args);
}

#[test]
fn invalid_files_are_reported_and_wrong_extension_is_ignored() {
    let root = TempRoot::new("invalid");
    write_project(&root, "bad-frontmatter", "---\ndescription: nope\n---\n");
    write_project(
        &root,
        "missing-description",
        "/* mode-workflow\nwhenToUse: x\n*/\n",
    );
    let dir = dirs(&root).project;
    fs::write(dir.join("wrong-ext.ts"), "return {};").expect("write wrong ext");

    let listing = agent_server::WorkflowDirs::new(
        root.path().join("ws").to_str().expect("ws"),
        &root.path().join("data"),
    );
    let result = list(&listing);
    assert!(result.workflows.is_empty());
    assert_eq!(result.invalid.len(), 2);
    let reasons: Vec<&str> = result
        .invalid
        .iter()
        .map(|entry| entry.reason.as_str())
        .collect();
    assert!(reasons.iter().any(|reason| reason.starts_with(
        "missing_frontmatter: file does not start with the `/* mode-workflow` metadata block"
    )));
    assert!(reasons.contains(&"parse_error: metadata block is missing `description`"));

    let bad = match agent_server::workflows::get(&listing, "bad-frontmatter", None) {
        WorkflowsGetResult::Failure(failure) => failure,
        other => panic!("expected parse failure, got {other:?}"),
    };
    assert_eq!(
        bad.reason,
        agent_protocol::workflow::SavedWorkflowFailureReason::ParseError
    );
    assert_eq!(
        bad.detail.as_deref(),
        Some("file does not start with the `/* mode-workflow` metadata block")
    );
}

fn list(dirs: &WorkflowDirs) -> agent_protocol::workflow::WorkflowsListResult {
    agent_server::workflows::list(dirs, None)
}

#[test]
fn list_get_update_meta_delete_and_move() {
    let root = TempRoot::new("lifecycle");
    write_project(&root, "probe-wf", SAVED);
    let dirs = dirs(&root);

    let listing = list(&dirs);
    assert_eq!(listing.workflows.len(), 1);
    let entry = &listing.workflows[0];
    assert_eq!(entry.name, "probe-wf");
    assert_eq!(entry.scope, SavedWorkflowScope::Project);
    assert_eq!(entry.description, "probe workflow saved by tool");

    let got = match agent_server::workflows::get(&dirs, "probe-wf", None) {
        WorkflowsGetResult::Ok(ok) => ok,
        other => panic!("expected get ok, got {other:?}"),
    };
    assert_eq!(got.script, "return { ok: true };");
    assert_eq!(got.scope, SavedWorkflowScope::Project);

    let meta = SavedWorkflowMeta {
        description: "probe workflow saved by tool (meta updated)".to_string(),
        when_to_use: Some("updated whenToUse".to_string()),
        args: None,
    };
    let updated = agent_server::workflows::update_meta(&dirs, "probe-wf", &meta, None);
    assert!(matches!(updated, WorkflowsUpdateMetaResult::Ok(_)));
    let got = match agent_server::workflows::get(&dirs, "probe-wf", None) {
        WorkflowsGetResult::Ok(ok) => ok,
        other => panic!("expected get ok, got {other:?}"),
    };
    assert_eq!(got.meta, meta);
    assert_eq!(got.script, "return { ok: true };");

    // project → global → project。
    let moved = agent_server::workflows::move_workflow(&dirs, "probe-wf");
    assert!(matches!(moved, WorkflowsMoveResult::Ok(ref ok) if ok.to.contains("data")));
    assert!(!dirs.project.join("probe-wf.dwf.ts").exists());
    assert!(dirs.global.join("probe-wf.dwf.ts").is_file());
    let listing = list(&dirs);
    assert_eq!(listing.workflows[0].scope, SavedWorkflowScope::Global);
    let moved_back = agent_server::workflows::move_workflow(&dirs, "probe-wf");
    assert!(matches!(moved_back, WorkflowsMoveResult::Ok(ref ok) if ok.to.contains("ws")));

    let deleted = agent_server::workflows::delete(&dirs, "probe-wf", None);
    assert!(matches!(deleted, WorkflowsDeleteResult::Ok(_)));
    let missing = agent_server::workflows::delete(&dirs, "probe-wf", None);
    match missing {
        WorkflowsDeleteResult::Failure(failure) => {
            assert_eq!(
                failure.reason,
                agent_protocol::workflow::SavedWorkflowFailureReason::NotFound
            );
        }
        other => panic!("expected not_found, got {other:?}"),
    }

    let invalid = agent_server::workflows::get(&dirs, "bad/name", None);
    match invalid {
        WorkflowsGetResult::Failure(failure) => {
            assert_eq!(
                failure.reason,
                agent_protocol::workflow::SavedWorkflowFailureReason::InvalidName
            );
            assert_eq!(
                failure.detail.as_deref(),
                Some("workflow names may only contain letters, digits, '.', '-' and '_', and must be 1-64 characters")
            );
        }
        other => panic!("expected invalid_name, got {other:?}"),
    }
}

fn seed_runs_database(data_root: &Path, workspace: &str) {
    let db_dir = data_root.join("cli").join("db");
    fs::create_dir_all(&db_dir).expect("db dir");
    let connection = rusqlite::Connection::open(db_dir.join("db.sqlite")).expect("open db");
    connection
        .execute_batch(
            "create table dwf_run (
                id text primary key,
                parent_session_id text,
                cwd text,
                name text,
                script_text text,
                script_hash text,
                args_json text,
                tool_call_id text,
                resumed_from text,
                caps_max_concurrency integer,
                spent_tokens integer,
                status text,
                result_json text,
                failure_json text,
                time_created integer,
                time_updated integer
            );",
        )
        .expect("create dwf_run");
    for (index, name) in ["probe-run", "other-run"].iter().enumerate() {
        connection
            .execute(
                "insert into dwf_run (id,parent_session_id,cwd,name,script_text,script_hash,args_json,tool_call_id,resumed_from,caps_max_concurrency,spent_tokens,status,result_json,failure_json,time_created,time_updated)
                 values (?1,?2,?3,?4,'return { ok: true };','hash',null,'tool',null,10,0,'completed','{\"ok\":true}',null,?5,?5)",
                rusqlite::params![
                    format!("dwfrun-{index}"),
                    "sess_1",
                    workspace,
                    name,
                    1_700_000_000_000i64 + index as i64
                ],
            )
            .expect("insert run");
    }
}

#[test]
fn runs_reads_dwf_run_with_limit_and_name_filter() {
    let root = TempRoot::new("runs");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&workspace).expect("workspace");
    let workspace_text = workspace.to_str().expect("ws").to_string();
    seed_runs_database(&root.path().join("data"), &workspace_text);

    let all = agent_server::workflows::runs(&root.path().join("data"), &workspace_text, None, 10);
    assert_eq!(all.runs.len(), 2);
    assert_eq!(all.runs[0].run_id, "dwfrun-1");
    assert_eq!(all.runs[0].status, WorkflowRunStatus::Completed);
    assert_eq!(all.runs[0].name.as_deref(), Some("other-run"));
    assert!(all.truncated.is_none());

    let filtered = agent_server::workflows::runs(
        &root.path().join("data"),
        &workspace_text,
        Some("probe-run"),
        10,
    );
    assert_eq!(filtered.runs.len(), 1);
    assert_eq!(filtered.runs[0].name.as_deref(), Some("probe-run"));

    let limited =
        agent_server::workflows::runs(&root.path().join("data"), &workspace_text, None, 1);
    assert_eq!(limited.runs.len(), 1);
    assert!(limited.truncated.is_some());
}
