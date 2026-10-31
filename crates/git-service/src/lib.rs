//! Git 服务（repo 层）：仓库解析、状态快照、分支、图、diff、暂存、提交、推送、身份。
//!
//! 真源：`packages/services/src/git/repo/gitCliRepo.ts`（clean-room：只读本仓库 TS）。
//! 命令执行与解析在 `git-cli`；行为契约与已知差异见 `docs/specs/git-service.md`。

mod repo;
mod service;
mod types;

pub use repo::GitCliRepo;
pub use service::{get_changes_for_source, GitService};
pub use types::{
    GitBranchComparison, GitBranchComparisonChange, GitBranchComparisonSnapshot,
    GitBranchMutationAction, GitBranchMutationResult, GitCommitGraphCommit, GitCommitGraphRef,
    GitCommitGraphRefKind, GitCommitGraphResult, GitCommitGraphSnapshot, GitCommitResult,
    GitDiffQuery, GitFileChange, GitIdentity, GitLocalBranch, GitLocalBranchListResult,
    GitPushResult, GitRefreshResult, GitWorkspaceRepositoryInfo, GitWorkspaceRepositoryKind,
};
