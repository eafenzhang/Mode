//! Git 领域类型（`gitCliTypes.ts` 与 `@mode/shared` 的投影）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitHeadRefType {
    Branch,
    Detached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLineStat {
    pub added: u64,
    pub removed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<GitChangeKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusEntry {
    pub path: String,
    pub original_path: Option<String>,
    pub kind: GitChangeKind,
    pub x: Option<String>,
    pub y: Option<String>,
    pub is_untracked: bool,
    pub is_conflicted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepositoryAutoRefreshWatchPath {
    pub path: String,
    pub recursive: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepositorySummary {
    pub workspace_path: String,
    pub repo_root: String,
    pub workspace_in_repo_path: String,
    pub auto_refresh_watch_paths: Vec<GitRepositoryAutoRefreshWatchPath>,
    pub branch_name: Option<String>,
    pub tracking_branch_name: Option<String>,
    pub head_ref_type: GitHeadRefType,
    pub ahead: u64,
    pub behind: u64,
    pub is_dirty: bool,
    pub is_git_available: bool,
    pub is_repository: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitResolvedRepository {
    #[serde(serialize_with = "serialize_path")]
    pub workspace_path: PathBuf,
    #[serde(serialize_with = "serialize_path")]
    pub repo_root: PathBuf,
    pub workspace_in_repo_path: String,
    pub auto_refresh_watch_paths: Vec<GitRepositoryAutoRefreshWatchPath>,
    pub is_git_available: bool,
    pub is_repository: bool,
}

fn serialize_path<S: serde::Serializer>(
    path: &std::path::Path,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusSnapshot {
    pub resolution: GitResolvedRepository,
    pub summary: GitRepositorySummary,
    pub entries: Vec<GitStatusEntry>,
    pub staged_stats: std::collections::BTreeMap<String, GitLineStat>,
    pub unstaged_stats: std::collections::BTreeMap<String, GitLineStat>,
    pub untracked_stats: std::collections::BTreeMap<String, GitLineStat>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchMutationIssue {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GitDiffAvailability {
    Patch,
    Binary,
    Truncated,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffResult {
    pub path: String,
    pub availability: GitDiffAvailability,
    pub patch: Option<String>,
    pub before_content: Option<String>,
    pub after_content: Option<String>,
    pub summary: Option<String>,
}

pub fn create_empty_summary(resolution: &GitResolvedRepository) -> GitRepositorySummary {
    GitRepositorySummary {
        workspace_path: resolution.workspace_path.to_string_lossy().into_owned(),
        repo_root: resolution.repo_root.to_string_lossy().into_owned(),
        workspace_in_repo_path: resolution.workspace_in_repo_path.clone(),
        auto_refresh_watch_paths: resolution.auto_refresh_watch_paths.clone(),
        branch_name: None,
        tracking_branch_name: None,
        head_ref_type: GitHeadRefType::Branch,
        ahead: 0,
        behind: 0,
        is_dirty: false,
        is_git_available: resolution.is_git_available,
        is_repository: resolution.is_repository,
    }
}
