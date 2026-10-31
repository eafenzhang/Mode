//! Checkpoint 领域类型（`@mode/shared/git.ts` 的投影）。

use serde::Serialize;

use git_cli::GitChangeKind;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCheckpointMeta {
    pub checkpoint_id: String,
    pub workspace_path: String,
    pub repo_root: String,
    pub workspace_in_repo_path: String,
    pub created_at: u64,
    pub ref_name: String,
    pub commit_oid: String,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCheckpointFileDiff {
    pub path: String,
    pub repo_relative_path: String,
    pub workspace_relative_path: String,
    pub original_path: Option<String>,
    pub kind: GitChangeKind,
    pub added: u64,
    pub removed: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCheckpointDiff {
    pub from_checkpoint_id: String,
    pub to_checkpoint_id: String,
    pub files: Vec<GitCheckpointFileDiff>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCheckpointConflict {
    pub path: String,
    pub repo_relative_path: String,
    pub workspace_relative_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCheckpointRestoreResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflicts: Option<Vec<GitCheckpointConflict>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_paths: Option<Vec<String>>,
}
