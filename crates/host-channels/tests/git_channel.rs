//! `git` 频道适配器：参数形状、camelCase 返回与错误映射（真实临时仓库）。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use git_service::{GitCliRepo, GitService};
use host_channels::git::{
    COMMIT, GENERATE_COMMIT_MESSAGE, GET_CHANGES, GET_IDENTITY, GET_REPOSITORY_SUMMARY, REFRESH,
    STAGE_PATHS,
};
use host_channels::GitChannel;
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

struct TempRepo(PathBuf);

impl TempRepo {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-git-channel-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        let path = canonical_long_path(&path);
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.name", "Mode Test"],
            vec!["config", "user.email", "mode@test.local"],
        ] {
            run_git(&path, &args);
        }
        fs::write(path.join("a.md"), "line1\n").unwrap();
        run_git(&path, &["add", "-A"]);
        run_git(&path, &["commit", "-m", "initial"]);
        Self(path)
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn canonical_long_path(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    canonical
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {:?} failed", args);
}

fn channel() -> GitChannel {
    let provider = git_cli::GitCommandProvider::new(Arc::new(
        git_cli::GitEnvironmentProvider::with_candidates(vec!["git".to_string()]),
    ));
    GitChannel::new(Arc::new(GitService::new(GitCliRepo::new(Arc::new(
        provider,
    )))))
}

async fn call(channel: &GitChannel, method: &str, arg: Value) -> Result<Value, ChannelError> {
    channel
        .call(
            "ctx".into(),
            method.into(),
            arg,
            CancellationToken::default(),
        )
        .await
}

fn object(value: serde_json::Value) -> Value {
    Value::Array(vec![Value::Json(value.to_string())])
}

fn json(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn summary_changes_and_commit_round_trip() {
    let repo = TempRepo::new();
    let workspace = repo.0.to_string_lossy().into_owned();
    fs::write(repo.0.join("a.md"), "line1\nline2\n").unwrap();
    fs::write(repo.0.join("new.md"), "hello\n").unwrap();
    let channel = channel();

    let summary = json(
        &call(
            &channel,
            GET_REPOSITORY_SUMMARY,
            object(serde_json::json!({ "workspacePath": workspace })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(summary["branchName"], "main");
    assert_eq!(summary["isDirty"], true);
    assert_eq!(summary["headRefType"], "branch");

    let changes = json(
        &call(
            &channel,
            GET_CHANGES,
            object(serde_json::json!({ "workspacePath": workspace, "sourceId": "unstaged" })),
        )
        .await
        .unwrap(),
    );
    let paths: Vec<&str> = changes
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["repoRelativePath"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"a.md"));
    assert!(paths.contains(&"new.md"));

    call(
        &channel,
        STAGE_PATHS,
        object(serde_json::json!({ "workspacePath": workspace, "paths": ["a.md", "new.md"] })),
    )
    .await
    .unwrap();
    let committed = json(
        &call(
            &channel,
            COMMIT,
            object(serde_json::json!({
                "workspacePath": workspace,
                "message": "feat: channel",
                "stagedOnly": true,
                "paths": ["a.md", "new.md"],
            })),
        )
        .await
        .unwrap(),
    );
    assert!(committed["commitHash"].as_str().unwrap().len() >= 40);
    assert_eq!(committed["summary"]["isDirty"], false);

    let refreshed = json(
        &call(
            &channel,
            REFRESH,
            object(serde_json::json!({
                "workspacePath": workspace,
                "includeIdentity": true,
                "includeBranchComparison": true,
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(refreshed["summary"]["branchName"], "main");
    assert_eq!(refreshed["identity"]["userName"], "Mode Test");
    assert_eq!(refreshed["unstagedChanges"], serde_json::json!([]));
    assert_eq!(refreshed["stagedChanges"], serde_json::json!([]));
    assert!(refreshed["branchComparison"]["baseRef"].is_null());
    assert_eq!(refreshed["branchComparison"]["headRef"], "main");
    assert_eq!(
        refreshed["branchComparison"]["changes"],
        serde_json::json!([])
    );

    let identity = json(
        &call(
            &channel,
            GET_IDENTITY,
            object(serde_json::json!({ "workspacePath": workspace })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(identity["userEmail"], "mode@test.local");
    assert_eq!(identity["scopeLabel"], "local");
}

#[tokio::test]
async fn generate_commit_message_is_unavailable_and_unknown_calls_error() {
    let channel = channel();
    let error = call(
        &channel,
        GENERATE_COMMIT_MESSAGE,
        object(serde_json::json!({ "workspacePath": "." })),
    )
    .await
    .unwrap_err();
    match error {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "Commit message generation is not available.")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let missing = call(&channel, GET_REPOSITORY_SUMMARY, Value::Array(Vec::new()))
        .await
        .unwrap_err();
    match missing {
        ChannelError::Error { message, .. } => assert_eq!(message, "missing arguments"),
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let unknown = call(&channel, "nope", object(serde_json::json!({})))
        .await
        .unwrap_err();
    match unknown {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "git channel: unknown command nope")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}
