//! `gitService.ts` 门面：repo 快照 → 频道类型映射、工作区范围裁剪与 refresh 聚合。

use std::path::Path;

use git_cli::{
    is_path_in_workspace_scope, to_workspace_relative_git_path, GitRepositorySummary,
    GitStatusEntry, GitStatusSnapshot,
};

use crate::repo::GitCliRepo;
use crate::types::{
    GitBranchComparison, GitBranchComparisonSnapshot, GitBranchMutationResult,
    GitCommitGraphResult, GitCommitResult, GitDiffQuery, GitFileChange, GitIdentity,
    GitLocalBranchListResult, GitPushResult, GitRefreshResult, GitWorkspaceRepositoryInfo,
};

pub struct GitService {
    repo: GitCliRepo,
}

impl GitService {
    pub fn new(repo: GitCliRepo) -> Self {
        Self { repo }
    }

    pub fn repo(&self) -> &GitCliRepo {
        &self.repo
    }

    pub fn get_repository_summary(
        &self,
        workspace_path: &Path,
    ) -> Result<GitRepositorySummary, String> {
        Ok(self.repo.get_status(workspace_path)?.summary)
    }

    pub fn get_workspace_repository_info(
        &self,
        workspace_path: &Path,
    ) -> Result<GitWorkspaceRepositoryInfo, String> {
        self.repo.get_workspace_repository_info(workspace_path)
    }

    pub fn get_local_branches(
        &self,
        workspace_path: &Path,
    ) -> Result<GitLocalBranchListResult, String> {
        self.repo.list_local_branches(workspace_path)
    }

    pub fn get_commit_graph(
        &self,
        workspace_path: &Path,
        max_count: Option<u64>,
        skip: Option<u64>,
    ) -> Result<GitCommitGraphResult, String> {
        let snapshot = self
            .repo
            .get_commit_graph(workspace_path, max_count, skip)?;
        Ok(GitCommitGraphResult {
            commits: snapshot.commits,
            has_more: snapshot.has_more,
        })
    }

    pub fn switch_branch(
        &self,
        workspace_path: &Path,
        target_branch_name: &str,
    ) -> Result<GitBranchMutationResult, String> {
        self.repo.switch_branch(workspace_path, target_branch_name)
    }

    pub fn create_branch_and_switch(
        &self,
        workspace_path: &Path,
        branch_name: &str,
        start_point: Option<&str>,
    ) -> Result<GitBranchMutationResult, String> {
        self.repo
            .create_branch_and_switch(workspace_path, branch_name, start_point)
    }

    pub fn get_changes(
        &self,
        workspace_path: &Path,
        source_id: &str,
    ) -> Result<Vec<GitFileChange>, String> {
        let status = self.repo.get_status(workspace_path)?;
        Ok(get_changes_for_source(&status, source_id))
    }

    pub fn get_ignored_paths(
        &self,
        workspace_path: &Path,
        paths: &[String],
    ) -> Result<Vec<String>, String> {
        self.repo.get_ignored_paths(workspace_path, paths)
    }

    pub fn get_diff(&self, params: &GitDiffQuery) -> Result<git_cli::GitDiffResult, String> {
        self.repo.get_diff(params)
    }

    pub fn get_branch_comparison(
        &self,
        workspace_path: &Path,
    ) -> Result<GitBranchComparison, String> {
        let comparison = self.repo.get_branch_comparison(workspace_path)?;
        Ok(to_branch_comparison(&comparison))
    }

    pub fn stage_paths(&self, workspace_path: &Path, paths: &[String]) -> Result<(), String> {
        self.repo.stage(workspace_path, paths)
    }

    pub fn unstage_paths(&self, workspace_path: &Path, paths: &[String]) -> Result<(), String> {
        self.repo.unstage(workspace_path, paths)
    }

    pub fn discard_paths(
        &self,
        workspace_path: &Path,
        paths: &[String],
        staged: bool,
    ) -> Result<(), String> {
        self.repo.discard(workspace_path, paths, staged)
    }

    /// 提交信息生成依赖 Agent 模型调用（W4），壳内暂时只返回与 TS 无 generator 时相同的错误。
    pub fn generate_commit_message(&self) -> Result<serde_json::Value, String> {
        Err("Commit message generation is not available.".to_string())
    }

    pub fn commit(
        &self,
        workspace_path: &Path,
        message: &str,
        paths: Option<&[String]>,
        staged_only: bool,
    ) -> Result<GitCommitResult, String> {
        let commit_hash = self
            .repo
            .commit(workspace_path, message, paths, staged_only)?;
        let status = self.repo.get_status(workspace_path)?;
        Ok(GitCommitResult {
            commit_hash,
            summary: status.summary,
        })
    }

    pub fn push(&self, workspace_path: &Path) -> Result<GitPushResult, String> {
        self.repo.push(workspace_path)
    }

    pub fn get_identity(&self, workspace_path: &Path) -> Result<GitIdentity, String> {
        self.repo.get_identity(workspace_path)
    }

    pub fn refresh(
        &self,
        workspace_path: &Path,
        include_identity: bool,
        include_branch_comparison: bool,
    ) -> Result<GitRefreshResult, String> {
        let status = self.repo.get_status(workspace_path)?;
        let identity = if include_identity {
            Some(self.repo.get_identity(workspace_path)?)
        } else {
            None
        };
        let branch_comparison = if include_branch_comparison {
            Some(to_branch_comparison(
                &self.repo.get_branch_comparison(workspace_path)?,
            ))
        } else {
            None
        };
        Ok(GitRefreshResult {
            unstaged_changes: get_changes_for_source(&status, "unstaged"),
            staged_changes: get_changes_for_source(&status, "staged"),
            summary: status.summary,
            identity,
            branch_comparison,
        })
    }
}

fn to_absolute_path(repo_root: &Path, repo_relative_path: &str) -> String {
    let mut path = repo_root.to_path_buf();
    for segment in repo_relative_path.replace('\\', "/").split('/') {
        if !segment.is_empty() {
            path = path.join(segment);
        }
    }
    path.to_string_lossy().into_owned()
}

fn matches_workspace_scope(
    workspace_in_repo_path: &str,
    path: &str,
    original_path: Option<&str>,
) -> bool {
    is_path_in_workspace_scope(path, workspace_in_repo_path)
        || original_path
            .map(|original| is_path_in_workspace_scope(original, workspace_in_repo_path))
            .unwrap_or(false)
}

fn build_file_change(
    snapshot: &GitStatusSnapshot,
    entry: &GitStatusEntry,
    section: &str,
    added: u64,
    removed: u64,
) -> GitFileChange {
    GitFileChange {
        path: to_absolute_path(&snapshot.resolution.repo_root, &entry.path),
        repo_relative_path: entry.path.clone(),
        workspace_relative_path: to_workspace_relative_git_path(
            &entry.path,
            &snapshot.summary.workspace_in_repo_path,
        ),
        x: entry.x.clone(),
        y: entry.y.clone(),
        kind: entry.kind,
        section: section.to_string(),
        added,
        removed,
        is_staged: section == "staged",
        is_untracked: section == "untracked",
        is_conflicted: section == "conflicted",
    }
}

fn should_include_untracked_change(entry: &GitStatusEntry, added: u64, removed: u64) -> bool {
    entry.path.ends_with('/') || added > 0 || removed > 0
}

fn to_change_for_source(
    snapshot: &GitStatusSnapshot,
    entry: &GitStatusEntry,
    source_id: &str,
) -> Option<GitFileChange> {
    if !matches_workspace_scope(
        &snapshot.summary.workspace_in_repo_path,
        &entry.path,
        entry.original_path.as_deref(),
    ) {
        return None;
    }
    if source_id == "staged" {
        let x = entry.x.as_deref().unwrap_or("");
        if entry.is_untracked || entry.is_conflicted || x.is_empty() || x == "." {
            return None;
        }
        let stat = snapshot.staged_stats.get(&entry.path);
        return Some(build_file_change(
            snapshot,
            entry,
            "staged",
            stat.map(|stat| stat.added).unwrap_or(0),
            stat.map(|stat| stat.removed).unwrap_or(0),
        ));
    }

    if entry.is_conflicted {
        return Some(build_file_change(snapshot, entry, "conflicted", 0, 0));
    }
    if entry.is_untracked {
        let stat = snapshot.untracked_stats.get(&entry.path);
        let added = stat.map(|stat| stat.added).unwrap_or(0);
        let removed = stat.map(|stat| stat.removed).unwrap_or(0);
        if !should_include_untracked_change(entry, added, removed) {
            return None;
        }
        return Some(build_file_change(
            snapshot,
            entry,
            "untracked",
            added,
            removed,
        ));
    }
    let y = entry.y.as_deref().unwrap_or("");
    if y.is_empty() || y == "." {
        return None;
    }
    let stat = snapshot.unstaged_stats.get(&entry.path);
    Some(build_file_change(
        snapshot,
        entry,
        "unstaged",
        stat.map(|stat| stat.added).unwrap_or(0),
        stat.map(|stat| stat.removed).unwrap_or(0),
    ))
}

pub fn get_changes_for_source(snapshot: &GitStatusSnapshot, source_id: &str) -> Vec<GitFileChange> {
    snapshot
        .entries
        .iter()
        .filter_map(|entry| to_change_for_source(snapshot, entry, source_id))
        .collect()
}

fn to_branch_comparison(snapshot: &GitBranchComparisonSnapshot) -> GitBranchComparison {
    let changes = snapshot
        .changes
        .iter()
        .filter(|change| {
            matches_workspace_scope(
                &snapshot.resolution.workspace_in_repo_path,
                &change.path,
                change.original_path.as_deref(),
            )
        })
        .map(|change| GitFileChange {
            path: to_absolute_path(&snapshot.resolution.repo_root, &change.path),
            repo_relative_path: change.path.clone(),
            workspace_relative_path: to_workspace_relative_git_path(
                &change.path,
                &snapshot.resolution.workspace_in_repo_path,
            ),
            x: None,
            y: None,
            kind: change.kind,
            section: "branch".to_string(),
            added: change.added,
            removed: change.removed,
            is_staged: false,
            is_untracked: false,
            is_conflicted: false,
        })
        .collect();
    GitBranchComparison {
        base_ref: snapshot.base_ref.clone(),
        head_ref: snapshot.head_ref.clone(),
        comparison_label: snapshot.comparison_label.clone(),
        changes,
    }
}
