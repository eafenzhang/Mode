//! `git-checkpoint` 频道适配器：参数形状、camelCase 返回与错误映射（真实临时仓库）。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use git_checkpoint_service::GitCheckpointService;
use host_channels::git_checkpoint::{
    CREATE_CHECKPOINT, DELETE_CHECKPOINT, DIFF_CHECKPOINTS, RESTORE_BETWEEN_CHECKPOINTS,
};
use host_channels::GitCheckpointChannel;
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;

struct TempRepo {
    root: PathBuf,
}

impl TempRepo {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-git-checkpoint-channel-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        let workspace = path.join("ws");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(path.join("gitconfig-empty"), "").unwrap();
        let workspace = canonical_long_path(&workspace);
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.name", "Mode Test"],
            vec!["config", "user.email", "mode@test.local"],
        ] {
            run_git(&workspace, &args);
        }
        fs::write(workspace.join("a.md"), "line1\n").unwrap();
        run_git(&workspace, &["add", "-A"]);
        run_git(&workspace, &["commit", "-m", "initial"]);
        Self { root: path }
    }

    fn workspace(&self) -> PathBuf {
        canonical_long_path(&self.root.join("ws"))
    }

    fn data_root(&self) -> PathBuf {
        self.root.join("data")
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
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
    let repo_root = cwd.parent().unwrap_or(cwd);
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        // 隔离仓库外配置：`core.autocrlf` 等过滤器会让 blob 与 restore 后的字节不一致，
        // 导致 hash-object 校验在 Windows 上误报冲突。
        .env("GIT_CONFIG_GLOBAL", repo_root.join("gitconfig-empty"))
        .env("GIT_CONFIG_SYSTEM", repo_root.join("gitconfig-empty"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {:?} failed", args);
}

fn channel(repo: &TempRepo) -> GitCheckpointChannel {
    let provider = Arc::new(
        git_cli::GitCommandProvider::new(Arc::new(
            git_cli::GitEnvironmentProvider::with_candidates(vec!["git".to_string()]),
        ))
        .with_extra_env(vec![
            (
                "GIT_CONFIG_GLOBAL".to_string(),
                Some(
                    repo.root
                        .join("gitconfig-empty")
                        .to_string_lossy()
                        .into_owned(),
                ),
            ),
            (
                "GIT_CONFIG_SYSTEM".to_string(),
                Some(
                    repo.root
                        .join("gitconfig-empty")
                        .to_string_lossy()
                        .into_owned(),
                ),
            ),
            ("GIT_CONFIG_NOSYSTEM".to_string(), Some("1".to_string())),
        ]),
    );
    GitCheckpointChannel::new(Arc::new(GitCheckpointService::new(
        provider,
        &repo.data_root(),
    )))
}

async fn call(
    channel: &GitCheckpointChannel,
    method: &str,
    arg: Value,
) -> Result<Value, ChannelError> {
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
async fn create_diff_restore_delete_round_trip() {
    let repo = TempRepo::new();
    let workspace = repo.workspace().to_string_lossy().into_owned();
    let channel = channel(&repo);

    let first = json(
        &call(
            &channel,
            CREATE_CHECKPOINT,
            object(serde_json::json!({ "workspacePath": workspace })),
        )
        .await
        .unwrap(),
    );
    let first_id = first["checkpointId"].as_str().unwrap().to_string();
    assert_eq!(first["scope"], "workspace");
    assert_eq!(first["workspaceInRepoPath"], ".");
    assert!(first["refName"]
        .as_str()
        .unwrap()
        .starts_with("refs/mode/checkpoints/"));

    fs::write(repo.workspace().join("a.md"), "line1\nline2\n").unwrap();
    fs::write(repo.workspace().join("new.md"), "hello\n").unwrap();
    let second = json(
        &call(
            &channel,
            CREATE_CHECKPOINT,
            object(serde_json::json!({ "workspacePath": workspace })),
        )
        .await
        .unwrap(),
    );
    let second_id = second["checkpointId"].as_str().unwrap().to_string();

    let diff = json(
        &call(
            &channel,
            DIFF_CHECKPOINTS,
            object(serde_json::json!({
                "workspacePath": workspace,
                "fromCheckpointId": first_id,
                "toCheckpointId": second_id,
            })),
        )
        .await
        .unwrap(),
    );
    let files = diff["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    let new_file = files
        .iter()
        .find(|file| file["repoRelativePath"] == "new.md")
        .unwrap();
    assert_eq!(new_file["kind"], "added");
    assert_eq!(new_file["added"], 1);

    // 磁盘仍停留在第二个 checkpoint：默认恢复必须报告冲突而不是覆盖。
    let blocked = json(
        &call(
            &channel,
            RESTORE_BETWEEN_CHECKPOINTS,
            object(serde_json::json!({
                "workspacePath": workspace,
                "fromCheckpointId": first_id,
                "toCheckpointId": second_id,
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(blocked["success"], false);
    assert!(!blocked["conflicts"].as_array().unwrap().is_empty());

    fs::write(repo.workspace().join("a.md"), "line1\n").unwrap();
    fs::remove_file(repo.workspace().join("new.md")).unwrap();
    let restored = json(
        &call(
            &channel,
            RESTORE_BETWEEN_CHECKPOINTS,
            object(serde_json::json!({
                "workspacePath": workspace,
                "fromCheckpointId": first_id,
                "toCheckpointId": second_id,
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(restored["success"], true);
    assert_eq!(
        fs::read_to_string(repo.workspace().join("a.md")).unwrap(),
        "line1\nline2\n"
    );
    assert_eq!(
        fs::read_to_string(repo.workspace().join("new.md")).unwrap(),
        "hello\n"
    );

    let deleted = call(
        &channel,
        DELETE_CHECKPOINT,
        object(serde_json::json!({
            "workspacePath": workspace,
            "checkpointId": first_id,
        })),
    )
    .await
    .unwrap();
    assert!(matches!(deleted, Value::Undefined));

    let error = call(
        &channel,
        DIFF_CHECKPOINTS,
        object(serde_json::json!({
            "workspacePath": workspace,
            "fromCheckpointId": first_id,
            "toCheckpointId": second_id,
        })),
    )
    .await
    .unwrap_err();
    match error {
        ChannelError::Error { message, .. } => {
            assert!(message.starts_with("Checkpoint does not exist:"))
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}

#[tokio::test]
async fn missing_checkpoint_and_unknown_command_error() {
    let repo = TempRepo::new();
    let workspace = repo.workspace().to_string_lossy().into_owned();
    let channel = channel(&repo);

    let error = call(
        &channel,
        DIFF_CHECKPOINTS,
        object(serde_json::json!({
            "workspacePath": workspace,
            "fromCheckpointId": "missing",
            "toCheckpointId": "missing",
        })),
    )
    .await
    .unwrap_err();
    match error {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "Checkpoint does not exist: missing")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }

    let error = call(
        &channel,
        "unknown",
        object(serde_json::json!({ "workspacePath": workspace })),
    )
    .await
    .unwrap_err();
    match error {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "git-checkpoint channel: unknown command unknown")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}
