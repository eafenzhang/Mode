//! Git CLI 基础层：命令执行、环境探测与输出解析。
//!
//! 真源：`packages/services/src/git/config.ts`、`providers/*`、`repo/gitCliHelpers.ts`、
//! `repo/gitCliTypes.ts`。`git` 与 `git-checkpoint` 两个频道共用本 crate；
//! 行为契约与已知差异见 `docs/specs/git-cli.md`。

mod command;
mod config;
mod environment;
mod helpers;
mod types;

pub use command::{GitCommandExecutionOptions, GitCommandExecutionResult, GitCommandProvider};
pub use config::{
    git_binary_candidates, git_command_env, git_null_device_path, is_path_in_workspace_scope,
    normalize_git_path, normalize_workspace_in_repo_path, to_workspace_relative_git_path,
    DEFAULT_GIT_COMMAND_TIMEOUT_MS, DEFAULT_GIT_DIFF_BYTES, DEFAULT_GIT_DIFF_TIMEOUT_MS,
    DEFAULT_GIT_DISCOVERY_TIMEOUT_MS, DEFAULT_GIT_OUTPUT_BYTES, DEFAULT_GIT_PUSH_OUTPUT_BYTES,
    DEFAULT_GIT_PUSH_TIMEOUT_MS, GIT_UNTRACKED_STAT_CHUNK_BYTES, GIT_UNTRACKED_STAT_CONCURRENCY,
    GIT_UNTRACKED_STAT_MAX_BYTES,
};
pub use environment::GitEnvironmentProvider;
pub use helpers::{
    build_untracked_stats, build_untracked_text_diff_result, ensure_git_command_succeeded,
    ensure_repository_available, file_exists, infer_kind_from_numstat,
    is_missing_working_directory_result, is_not_repository_result, normalize_input_path,
    parse_git_branch_mutation_issues, parse_git_config_value, parse_numstat,
    parse_status_porcelain, to_diff_result, to_invalid_branch_name_issue, to_result_message,
    GitConfigValue, ParsedStatus,
};
pub use types::{
    create_empty_summary, GitBranchMutationIssue, GitChangeKind, GitDiffAvailability,
    GitDiffResult, GitHeadRefType, GitLineStat, GitRepositoryAutoRefreshWatchPath,
    GitRepositorySummary, GitResolvedRepository, GitStatusEntry, GitStatusSnapshot,
};
