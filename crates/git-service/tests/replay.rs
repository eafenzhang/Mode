//! fixture 回放对拍（`docs/specs/git-service.md`）：真实 git 仓库端到端。
//!
//! `pnpm git-service:fixtures` 固定作者/提交时间与全局配置，记录 repo 层的六个用例；
//! 本测试在 Rust 侧重放同一套仓库操作并逐项深比较（40 位 hash 归一为 `<hash>`）。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};

use git_cli::{GitCommandProvider, GitEnvironmentProvider};
use git_service::{GitCliRepo, GitDiffQuery, GitService};

const FIXTURE: &str = include_str!("../fixtures/git-service.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct FixtureCase {
    name: String,
    setup: Setup,
    ops: Vec<Op>,
    expected: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Setup {
    files: Vec<FileSpec>,
    #[serde(default)]
    remote: bool,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    detach: bool,
    #[serde(default)]
    extra_files: Option<Vec<FileSpec>>,
    #[serde(default)]
    add_paths: Option<Vec<String>>,
    #[serde(default)]
    skip_repo: bool,
}

#[derive(Deserialize)]
struct FileSpec {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Op {
    kind: String,
    paths: Option<Vec<String>>,
    path: Option<String>,
    source_id: Option<String>,
    staged: Option<bool>,
    branch: Option<String>,
    start_point: Option<String>,
    message: Option<String>,
    max_count: Option<u64>,
    skip: Option<u64>,
    staged_only: Option<bool>,
    include_identity: Option<bool>,
    include_branch_comparison: Option<bool>,
}

struct Roots {
    native: PathBuf,
    native_text: String,
    forward: String,
    escaped: String,
}

impl Roots {
    fn create(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let base = std::env::temp_dir().join(format!(
            "zc-git-service-replay-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create sandbox");
        let native = canonical_long_path(&base);
        let native_text = native.to_string_lossy().into_owned();
        let forward = native_text.replace('\\', "/");
        let escaped = native_text.replace('\\', "\\\\");
        Self {
            native,
            native_text,
            forward,
            escaped,
        }
    }

    fn normalize_text(&self, text: &str) -> String {
        let mut replaced = text.replace(&self.escaped, "<root>");
        replaced = replaced.replace(&self.native_text, "<root>");
        replaced = replaced.replace(&self.forward, "<root>");
        if cfg!(windows) {
            replaced = replaced.replace("\\\\", "/");
            replaced = replaced.replace('\\', "/");
        }
        replaced
    }
}

fn canonical_long_path(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    canonical
}

fn is_hash(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn normalize_value(roots: &Roots, key: Option<&str>, value: &Value) -> Value {
    match value {
        Value::String(text) => {
            if key == Some("patch") {
                return value.clone();
            }
            let replaced = roots.normalize_text(text);
            if is_hash(&replaced) {
                Value::String("<hash>".to_string())
            } else {
                Value::String(replaced)
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_value(roots, None, item))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (child_key, child) in map {
                out.insert(
                    child_key.clone(),
                    normalize_value(roots, Some(child_key), child),
                );
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn git_env(roots: &Roots) -> Vec<(String, String)> {
    let config = roots.native.join("gitconfig-empty");
    let config_text = config.to_string_lossy().into_owned();
    vec![
        ("GIT_AUTHOR_NAME".into(), "Mode Test".into()),
        ("GIT_AUTHOR_EMAIL".into(), "mode@test.local".into()),
        ("GIT_COMMITTER_NAME".into(), "Mode Test".into()),
        ("GIT_COMMITTER_EMAIL".into(), "mode@test.local".into()),
        ("GIT_AUTHOR_DATE".into(), "2024-01-01T00:00:00+0000".into()),
        (
            "GIT_COMMITTER_DATE".into(),
            "2024-01-01T00:00:00+0000".into(),
        ),
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
        ("GIT_CONFIG_GLOBAL".into(), config_text.clone()),
        ("GIT_CONFIG_SYSTEM".into(), config_text),
    ]
}

fn run_git(cwd: &Path, args: &[&str], env: &[(String, String)]) -> Result<(), String> {
    let mut command = Command::new("git");
    command.args(args).current_dir(cwd);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_CONFIG",
    ] {
        command.env_remove(name);
    }
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn materialize(root: &Path, files: &[FileSpec]) {
    for file in files {
        let target = root.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("materialize parent");
        }
        std::fs::write(&target, &file.content).expect("materialize file");
    }
}

fn setup_repo(roots: &Roots, setup: &Setup) -> Result<PathBuf, String> {
    let workspace = roots.native.join("ws");
    std::fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    std::fs::write(roots.native.join("gitconfig-empty"), "").map_err(|error| error.to_string())?;
    if setup.skip_repo {
        materialize(&workspace, &setup.files);
        return Ok(workspace);
    }
    let env: Vec<(String, String)> = git_env(roots);
    run_git(&workspace, &["init", "-b", "main"], &env)?;
    run_git(&workspace, &["config", "user.name", "Mode Test"], &env)?;
    run_git(
        &workspace,
        &["config", "user.email", "mode@test.local"],
        &env,
    )?;
    materialize(&workspace, &setup.files);
    run_git(&workspace, &["add", "-A"], &env)?;
    run_git(&workspace, &["commit", "-m", "initial"], &env)?;
    if setup.remote {
        let remote = roots.native.join("remote.git");
        run_git(
            &roots.native,
            &["init", "--bare", remote.to_string_lossy().as_ref()],
            &env,
        )?;
        run_git(
            &workspace,
            &["remote", "add", "origin", remote.to_string_lossy().as_ref()],
            &env,
        )?;
        run_git(&workspace, &["push", "-u", "origin", "main"], &env)?;
    }
    if let Some(branch) = &setup.branch {
        run_git(&workspace, &["branch", branch], &env)?;
    }
    if let Some(files) = &setup.extra_files {
        materialize(&workspace, files);
    }
    if let Some(paths) = &setup.add_paths {
        if !paths.is_empty() {
            let mut args: Vec<&str> = vec!["add", "--"];
            args.extend(paths.iter().map(String::as_str));
            run_git(&workspace, &args, &env)?;
        }
    }
    if setup.detach {
        run_git(&workspace, &["checkout", "--detach"], &env)?;
    }
    Ok(workspace)
}

fn run_op(service: &GitService, workspace: &Path, op: &Op) -> Result<Value, String> {
    let repo = service.repo();
    match op.kind.as_str() {
        "status" => {
            Ok(serde_json::to_value(repo.get_status(workspace)?).expect("status serializes"))
        }
        "ignored" => Ok(serde_json::to_value(
            repo.get_ignored_paths(workspace, &op.paths.clone().unwrap_or_default())?,
        )
        .expect("ignored serializes")),
        "branches" => {
            Ok(serde_json::to_value(repo.list_local_branches(workspace)?)
                .expect("branches serialize"))
        }
        "graph" => {
            Ok(
                serde_json::to_value(repo.get_commit_graph(workspace, op.max_count, op.skip)?)
                    .expect("graph serializes"),
            )
        }
        "identity" => {
            Ok(serde_json::to_value(repo.get_identity(workspace)?).expect("identity serializes"))
        }
        "diff" => Ok(serde_json::to_value(repo.get_diff(&GitDiffQuery {
            workspace_path: workspace.to_path_buf(),
            path: op.path.clone().unwrap_or_default(),
            source_id: op.source_id.clone(),
            staged: op.staged,
        })?)
        .expect("diff serializes")),
        "switch" => Ok(serde_json::to_value(
            repo.switch_branch(workspace, &op.branch.clone().unwrap_or_default())?,
        )
        .expect("switch serializes")),
        "createBranch" => Ok(serde_json::to_value(repo.create_branch_and_switch(
            workspace,
            &op.branch.clone().unwrap_or_default(),
            op.start_point.as_deref(),
        )?)
        .expect("create branch serializes")),
        "stage" => {
            repo.stage(workspace, &op.paths.clone().unwrap_or_default())?;
            Ok(json!({ "ok": true }))
        }
        "unstage" => {
            repo.unstage(workspace, &op.paths.clone().unwrap_or_default())?;
            Ok(json!({ "ok": true }))
        }
        "discard" => {
            repo.discard(
                workspace,
                &op.paths.clone().unwrap_or_default(),
                op.staged.unwrap_or(false),
            )?;
            Ok(json!({ "ok": true }))
        }
        "commit" => {
            let hash = repo.commit(
                workspace,
                op.message.as_deref().unwrap_or(""),
                op.paths.as_deref(),
                op.staged_only.unwrap_or(false),
            )?;
            Ok(json!({ "commitHash": hash }))
        }
        "push" => Ok(serde_json::to_value(repo.push(workspace)?).expect("push serializes")),
        "repoInfo" => Ok(
            serde_json::to_value(repo.get_workspace_repository_info(workspace)?)
                .expect("repo info serializes"),
        ),
        "branchComparison" => Ok(serde_json::to_value(repo.get_branch_comparison(workspace)?)
            .expect("branch comparison serializes")),
        "changes" => Ok(serde_json::to_value(
            service.get_changes(workspace, op.source_id.as_deref().unwrap_or("unstaged"))?,
        )
        .expect("changes serialize")),
        "summary" => Ok(
            serde_json::to_value(service.get_repository_summary(workspace)?)
                .expect("summary serializes"),
        ),
        "refresh" => Ok(serde_json::to_value(service.refresh(
            workspace,
            op.include_identity.unwrap_or(false),
            op.include_branch_comparison.unwrap_or(false),
        )?)
        .expect("refresh serializes")),
        "commitService" => Ok(serde_json::to_value(service.commit(
            workspace,
            op.message.as_deref().unwrap_or(""),
            op.paths.as_deref(),
            op.staged_only.unwrap_or(false),
        )?)
        .expect("commit result serializes")),
        "generate" => match service.generate_commit_message() {
            Ok(value) => Ok(value),
            Err(message) => Ok(json!({ "error": message })),
        },
        other => panic!("unknown op: {other}"),
    }
}

#[test]
fn replay_ts_fixtures() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
    let mut failures = Vec::new();
    for case in &fixture.cases {
        let roots = Roots::create(&case.name);
        let workspace = match setup_repo(&roots, &case.setup) {
            Ok(workspace) => workspace,
            Err(error) => {
                failures.push(format!("[case {}] setup failed: {error}", case.name));
                let _ = std::fs::remove_dir_all(&roots.native);
                continue;
            }
        };
        let provider =
            GitCommandProvider::new(Arc::new(GitEnvironmentProvider::with_candidates(vec![
                "git".to_string(),
            ])))
            .with_extra_env(
                git_env(&roots)
                    .into_iter()
                    .map(|(name, value)| (name, Some(value)))
                    .collect(),
            );
        let service = GitService::new(GitCliRepo::new(Arc::new(provider)));

        if case.expected.len() != case.ops.len() {
            failures.push(format!("[case {}] ops/expected 数量不一致", case.name));
            let _ = std::fs::remove_dir_all(&roots.native);
            continue;
        }
        for (index, op) in case.ops.iter().enumerate() {
            let actual = match run_op(&service, &workspace, op) {
                Ok(value) => value,
                Err(message) => json!({ "error": message }),
            };
            let actual = normalize_value(&roots, None, &actual);
            let expected = normalize_value(&roots, None, &case.expected[index]);
            if actual != expected {
                failures.push(format!(
                    "[case {}] op[{index}] {} 结果不一致\n--- expected ---\n{}\n--- actual ---\n{}",
                    case.name,
                    op.kind,
                    serde_json::to_string_pretty(&expected).unwrap_or_default(),
                    serde_json::to_string_pretty(&actual).unwrap_or_default(),
                ));
            }
        }
        let _ = std::fs::remove_dir_all(&roots.native);
    }
    assert!(
        failures.is_empty(),
        "fixture 回放存在差异：\n{}",
        failures.join("\n\n")
    );
}
