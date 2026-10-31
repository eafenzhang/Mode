//! Git repo 层的共享类型（`@mode/shared/git.ts` 与 `gitCliTypes.ts` 的投影）。

use serde::Serialize;

use git_cli::{GitChangeKind, GitHeadRefType, GitRepositorySummary, GitResolvedRepository};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitWorkspaceRepositoryKind {
    NotRepository,
    MainTree,
    LinkedWorktree,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitWorkspaceRepositoryInfo {
    pub workspace_path: String,
    pub kind: GitWorkspaceRepositoryKind,
    pub is_git_available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLocalBranch {
    pub name: String,
    pub is_current: bool,
    pub upstream_name: Option<String>,
    pub commit_hash: Option<String>,
    pub commit_timestamp_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLocalBranchListResult {
    pub head_ref_type: GitHeadRefType,
    pub current_branch_name: Option<String>,
    pub branches: Vec<GitLocalBranch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GitCommitGraphRefKind {
    Branch,
    Remote,
    Tag,
    Head,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GitCommitGraphRef {
    pub name: String,
    pub kind: GitCommitGraphRefKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitGraphCommit {
    pub hash: String,
    pub parents: Vec<String>,
    pub refs: Vec<GitCommitGraphRef>,
    pub subject: String,
    pub author_name: Option<String>,
    pub authored_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitGraphSnapshot {
    pub resolution: GitResolvedRepository,
    pub commits: Vec<GitCommitGraphCommit>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchComparisonChange {
    pub path: String,
    pub original_path: Option<String>,
    pub kind: GitChangeKind,
    pub added: u64,
    pub removed: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchComparisonSnapshot {
    pub resolution: GitResolvedRepository,
    pub base_ref: Option<String>,
    pub head_ref: Option<String>,
    pub comparison_label: Option<String>,
    pub changes: Vec<GitBranchComparisonChange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitBranchMutationAction {
    Switch,
    CreateAndSwitch,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchMutationResult {
    pub ok: bool,
    pub action: GitBranchMutationAction,
    pub branch_name: Option<String>,
    pub did_change: bool,
    pub created: bool,
    pub summary: GitRepositorySummary,
    pub issues: Vec<git_cli::GitBranchMutationIssue>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPushResult {
    pub branch_name: Option<String>,
    pub tracking_branch_name: Option<String>,
    pub remote_name: Option<String>,
    pub set_upstream: bool,
    pub summary: GitRepositorySummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIdentity {
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub name_source: Option<String>,
    pub email_source: Option<String>,
    pub scope_label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GitDiffQuery {
    pub workspace_path: std::path::PathBuf,
    pub path: String,
    pub source_id: Option<String>,
    pub staged: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileChange {
    pub path: String,
    pub repo_relative_path: String,
    pub workspace_relative_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    pub kind: GitChangeKind,
    pub section: String,
    pub added: u64,
    pub removed: u64,
    pub is_staged: bool,
    pub is_untracked: bool,
    pub is_conflicted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchComparison {
    pub base_ref: Option<String>,
    pub head_ref: Option<String>,
    pub comparison_label: Option<String>,
    pub changes: Vec<GitFileChange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitGraphResult {
    pub commits: Vec<GitCommitGraphCommit>,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitResult {
    pub commit_hash: String,
    pub summary: GitRepositorySummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRefreshResult {
    pub summary: GitRepositorySummary,
    pub identity: Option<GitIdentity>,
    pub unstaged_changes: Vec<GitFileChange>,
    pub staged_changes: Vec<GitFileChange>,
    pub branch_comparison: Option<GitBranchComparison>,
}
