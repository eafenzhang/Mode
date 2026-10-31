//! `git` 频道适配器：18 个方法。
//!
//! 参数形状是 `ProxyChannel` 的位置参数数组（第一个元素为对象）；Git 命令是阻塞调用，
//! 统一丢进 `spawn_blocking`。`generateCommitMessage` 依赖 Agent 模型（W4），
//! 暂时返回与 TS 无 generator 时一致的错误。
//!
//! 行为契约见 `docs/specs/git-service.md`。

use std::path::PathBuf;
use std::sync::Arc;

use git_service::{GitDiffQuery, GitService};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::Deserialize;

use crate::params::first_arg_object;

pub const GET_REPOSITORY_SUMMARY: &str = "getRepositorySummary";
pub const GET_WORKSPACE_REPOSITORY_INFO: &str = "getWorkspaceRepositoryInfo";
pub const GET_LOCAL_BRANCHES: &str = "getLocalBranches";
pub const GET_COMMIT_GRAPH: &str = "getCommitGraph";
pub const SWITCH_BRANCH: &str = "switchBranch";
pub const CREATE_BRANCH_AND_SWITCH: &str = "createBranchAndSwitch";
pub const GET_CHANGES: &str = "getChanges";
pub const GET_IGNORED_PATHS: &str = "getIgnoredPaths";
pub const GET_DIFF: &str = "getDiff";
pub const GET_BRANCH_COMPARISON: &str = "getBranchComparison";
pub const STAGE_PATHS: &str = "stagePaths";
pub const UNSTAGE_PATHS: &str = "unstagePaths";
pub const DISCARD_PATHS: &str = "discardPaths";
pub const GENERATE_COMMIT_MESSAGE: &str = "generateCommitMessage";
pub const COMMIT: &str = "commit";
pub const PUSH: &str = "push";
pub const GET_IDENTITY: &str = "getIdentity";
pub const REFRESH: &str = "refresh";

pub struct GitChannel {
    service: Arc<GitService>,
}

impl GitChannel {
    pub fn new(service: Arc<GitService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for GitChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || run(&service, &command, &arg))
                .await
                .map_err(|error| ChannelError::error(format!("git channel task failed: {error}")))?
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceArgs {
    workspace_path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitGraphArgs {
    workspace_path: PathBuf,
    #[serde(default)]
    max_count: Option<u64>,
    #[serde(default)]
    skip: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchBranchArgs {
    workspace_path: PathBuf,
    target_branch_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBranchArgs {
    workspace_path: PathBuf,
    branch_name: String,
    #[serde(default)]
    start_point: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangesArgs {
    workspace_path: PathBuf,
    source_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PathsArgs {
    workspace_path: PathBuf,
    paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffArgs {
    workspace_path: PathBuf,
    path: String,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    staged: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscardArgs {
    workspace_path: PathBuf,
    paths: Vec<String>,
    #[serde(default)]
    staged: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommitArgs {
    workspace_path: PathBuf,
    message: String,
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    staged_only: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshArgs {
    workspace_path: PathBuf,
    #[serde(default)]
    include_identity: Option<bool>,
    #[serde(default)]
    include_branch_comparison: Option<bool>,
}

fn run(service: &GitService, command: &str, arg: &Value) -> Result<Value, ChannelError> {
    match command {
        GET_REPOSITORY_SUMMARY => {
            let args: WorkspaceArgs = parse(arg)?;
            Ok(to_value(
                &service
                    .get_repository_summary(&args.workspace_path)
                    .chan()?,
            ))
        }
        GET_WORKSPACE_REPOSITORY_INFO => {
            let args: WorkspaceArgs = parse(arg)?;
            Ok(to_value(
                &service
                    .get_workspace_repository_info(&args.workspace_path)
                    .chan()?,
            ))
        }
        GET_LOCAL_BRANCHES => {
            let args: WorkspaceArgs = parse(arg)?;
            Ok(to_value(
                &service.get_local_branches(&args.workspace_path).chan()?,
            ))
        }
        GET_COMMIT_GRAPH => {
            let args: CommitGraphArgs = parse(arg)?;
            Ok(to_value(
                &service
                    .get_commit_graph(&args.workspace_path, args.max_count, args.skip)
                    .chan()?,
            ))
        }
        SWITCH_BRANCH => {
            let args: SwitchBranchArgs = parse(arg)?;
            Ok(to_value(
                &service
                    .switch_branch(&args.workspace_path, &args.target_branch_name)
                    .chan()?,
            ))
        }
        CREATE_BRANCH_AND_SWITCH => {
            let args: CreateBranchArgs = parse(arg)?;
            Ok(to_value(
                &service
                    .create_branch_and_switch(
                        &args.workspace_path,
                        &args.branch_name,
                        args.start_point.as_deref(),
                    )
                    .chan()?,
            ))
        }
        GET_CHANGES => {
            let args: ChangesArgs = parse(arg)?;
            Ok(to_value(
                &service
                    .get_changes(&args.workspace_path, &args.source_id)
                    .chan()?,
            ))
        }
        GET_IGNORED_PATHS => {
            let args: PathsArgs = parse(arg)?;
            Ok(to_value(
                &service
                    .get_ignored_paths(&args.workspace_path, &args.paths)
                    .chan()?,
            ))
        }
        GET_DIFF => {
            let args: DiffArgs = parse(arg)?;
            Ok(to_value(
                &service
                    .get_diff(&GitDiffQuery {
                        workspace_path: args.workspace_path,
                        path: args.path,
                        source_id: args.source_id,
                        staged: args.staged,
                    })
                    .chan()?,
            ))
        }
        GET_BRANCH_COMPARISON => {
            let args: WorkspaceArgs = parse(arg)?;
            Ok(to_value(
                &service.get_branch_comparison(&args.workspace_path).chan()?,
            ))
        }
        STAGE_PATHS => {
            let args: PathsArgs = parse(arg)?;
            service
                .stage_paths(&args.workspace_path, &args.paths)
                .chan()?;
            Ok(Value::Undefined)
        }
        UNSTAGE_PATHS => {
            let args: PathsArgs = parse(arg)?;
            service
                .unstage_paths(&args.workspace_path, &args.paths)
                .chan()?;
            Ok(Value::Undefined)
        }
        DISCARD_PATHS => {
            let args: DiscardArgs = parse(arg)?;
            service
                .discard_paths(
                    &args.workspace_path,
                    &args.paths,
                    args.staged.unwrap_or(false),
                )
                .chan()?;
            Ok(Value::Undefined)
        }
        GENERATE_COMMIT_MESSAGE => match service.generate_commit_message() {
            Ok(value) => Ok(to_value(&value)),
            Err(message) => Err(ChannelError::error(message)),
        },
        COMMIT => {
            let args: CommitArgs = parse(arg)?;
            Ok(to_value(
                &service
                    .commit(
                        &args.workspace_path,
                        &args.message,
                        args.paths.as_deref(),
                        args.staged_only.unwrap_or(false),
                    )
                    .chan()?,
            ))
        }
        PUSH => {
            let args: WorkspaceArgs = parse(arg)?;
            Ok(to_value(&service.push(&args.workspace_path).chan()?))
        }
        GET_IDENTITY => {
            let args: WorkspaceArgs = parse(arg)?;
            Ok(to_value(
                &service.get_identity(&args.workspace_path).chan()?,
            ))
        }
        REFRESH => {
            let args: RefreshArgs = parse(arg)?;
            Ok(to_value(
                &service
                    .refresh(
                        &args.workspace_path,
                        args.include_identity.unwrap_or(false),
                        args.include_branch_comparison.unwrap_or(false),
                    )
                    .chan()?,
            ))
        }
        other => Err(ChannelError::error(format!(
            "git channel: unknown command {other}"
        ))),
    }
}

fn parse<T: serde::de::DeserializeOwned>(arg: &Value) -> Result<T, ChannelError> {
    let params = first_arg_object(arg)?;
    serde_json::from_value(serde_json::Value::Object(params))
        .map_err(|error| ChannelError::error(format!("invalid arguments: {error}")))
}

trait IntoChannel<T> {
    fn chan(self) -> Result<T, ChannelError>;
}

impl<T> IntoChannel<T> for Result<T, String> {
    fn chan(self) -> Result<T, ChannelError> {
        self.map_err(ChannelError::error)
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    match serde_json::to_string(value) {
        Ok(text) => Value::Json(text),
        Err(error) => Value::Json(format!("{{\"serializeError\":\"{error}\"}}")),
    }
}

/// 供壳装配：默认环境探测的 Git 服务。
pub fn default_service() -> Arc<GitService> {
    Arc::new(GitService::new(git_service::GitCliRepo::new(Arc::new(
        git_cli::GitCommandProvider::new(Arc::new(git_cli::GitEnvironmentProvider::new())),
    ))))
}
