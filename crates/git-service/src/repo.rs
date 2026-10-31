//! Git repo 层（`gitCliRepo.ts`）：仓库解析、状态快照、分支、图、diff、暂存、提交、推送、身份。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use git_cli::{
    build_untracked_stats, build_untracked_text_diff_result, create_empty_summary,
    ensure_git_command_succeeded, ensure_repository_available, file_exists,
    infer_kind_from_numstat, is_missing_working_directory_result, is_not_repository_result,
    normalize_git_path, normalize_input_path, normalize_workspace_in_repo_path,
    parse_git_branch_mutation_issues, parse_git_config_value, parse_numstat,
    parse_status_porcelain, to_diff_result, to_invalid_branch_name_issue,
    GitCommandExecutionOptions, GitCommandExecutionResult, GitCommandProvider, GitDiffAvailability,
    GitDiffResult, GitHeadRefType, GitResolvedRepository, GitStatusSnapshot,
    DEFAULT_GIT_COMMAND_TIMEOUT_MS, DEFAULT_GIT_DIFF_BYTES, DEFAULT_GIT_DIFF_TIMEOUT_MS,
    DEFAULT_GIT_OUTPUT_BYTES, DEFAULT_GIT_PUSH_OUTPUT_BYTES, DEFAULT_GIT_PUSH_TIMEOUT_MS,
};

use crate::types::{
    GitBranchComparisonChange, GitBranchComparisonSnapshot, GitBranchMutationAction,
    GitBranchMutationResult, GitCommitGraphCommit, GitCommitGraphRef, GitCommitGraphRefKind,
    GitCommitGraphSnapshot, GitDiffQuery, GitIdentity, GitLocalBranch, GitLocalBranchListResult,
    GitPushResult, GitWorkspaceRepositoryInfo, GitWorkspaceRepositoryKind,
};

const GIT_OPERATION_MARKERS: [&str; 7] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "REBASE_HEAD",
    "rebase-merge",
    "rebase-apply",
    "BISECT_LOG",
];
const DEFAULT_GIT_GRAPH_MAX_COUNT: u64 = 100;
const MAX_GIT_GRAPH_MAX_COUNT: u64 = 200;

pub struct GitCliRepo {
    provider: Arc<GitCommandProvider>,
    collapsed_untracked_repo_roots: Mutex<BTreeSet<PathBuf>>,
}

impl GitCliRepo {
    pub fn new(provider: Arc<GitCommandProvider>) -> Self {
        Self {
            provider,
            collapsed_untracked_repo_roots: Mutex::new(BTreeSet::new()),
        }
    }

    pub fn provider(&self) -> &Arc<GitCommandProvider> {
        &self.provider
    }

    /// TS 侧只清 in-flight 请求缓存；同步实现没有缓存，保留方法以对齐接口。
    pub fn invalidate(&self, _workspace_path: &Path) {}

    fn run(
        &self,
        cwd: &Path,
        args: Vec<String>,
        timeout_ms: Option<u64>,
        max_output_bytes: Option<usize>,
        env: Vec<(String, Option<String>)>,
    ) -> Result<GitCommandExecutionResult, String> {
        let mut options = GitCommandExecutionOptions::new(cwd.to_path_buf(), args);
        options.timeout_ms = timeout_ms;
        options.max_output_bytes = max_output_bytes;
        options.env = env;
        self.provider.run(&options)
    }

    fn run_default(
        &self,
        cwd: &Path,
        args: Vec<String>,
    ) -> Result<GitCommandExecutionResult, String> {
        self.run(
            cwd,
            args,
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            None,
            Vec::new(),
        )
    }

    pub fn resolve_repository(
        &self,
        workspace_path: &Path,
    ) -> Result<GitResolvedRepository, String> {
        if self.provider.resolve_git_binary().is_none() {
            return Ok(unavailable_repository(workspace_path, false));
        }
        let result = self.run(
            workspace_path,
            [
                "rev-parse",
                "--show-toplevel",
                "--show-prefix",
                "--absolute-git-dir",
                "--git-common-dir",
            ]
            .iter()
            .map(|arg| arg.to_string())
            .collect(),
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            None,
            Vec::new(),
        )?;
        if result.exit_code != Some(0) {
            if is_missing_working_directory_result(&result) || is_not_repository_result(&result) {
                return Ok(unavailable_repository(workspace_path, true));
            }
            ensure_git_command_succeeded("git rev-parse", &result, &[0])?;
        }
        let lines: Vec<String> = result
            .stdout
            .replace("\r\n", "\n")
            .split('\n')
            .map(str::to_string)
            .collect();
        let Some(repo_root) = lines
            .first()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
        else {
            return Err("Failed to resolve Git repository root".to_string());
        };
        let repo_root = PathBuf::from(repo_root);
        let workspace_in_repo_path =
            normalize_workspace_in_repo_path(lines.get(1).map(String::as_str).unwrap_or(""));
        let absolute_git_dir = lines.get(2).map(|line| line.trim()).unwrap_or("");
        let git_common_dir = lines.get(3).map(|line| line.trim()).unwrap_or("");
        let auto_refresh_watch_paths =
            build_watch_paths(workspace_path, absolute_git_dir, git_common_dir);
        Ok(GitResolvedRepository {
            workspace_path: workspace_path.to_path_buf(),
            repo_root,
            workspace_in_repo_path,
            auto_refresh_watch_paths,
            is_git_available: true,
            is_repository: true,
        })
    }

    pub fn get_workspace_repository_info(
        &self,
        workspace_path: &Path,
    ) -> Result<GitWorkspaceRepositoryInfo, String> {
        let resolution = self.resolve_repository(workspace_path)?;
        let workspace_text = workspace_path.to_string_lossy().into_owned();
        if !resolution.is_git_available || !resolution.is_repository {
            return Ok(GitWorkspaceRepositoryInfo {
                workspace_path: workspace_text,
                kind: GitWorkspaceRepositoryKind::NotRepository,
                is_git_available: resolution.is_git_available,
            });
        }
        let git_entry = resolution.repo_root.join(".git");
        if let Ok(metadata) = std::fs::metadata(&git_entry) {
            if metadata.is_dir() {
                return Ok(main_tree_info(workspace_text));
            }
            if metadata.is_file() {
                if let Ok(content) = std::fs::read_to_string(&git_entry) {
                    let first_line = content
                        .replace("\r\n", "\n")
                        .split('\n')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if let Some(rest) = first_line.strip_prefix("gitdir:") {
                        let raw = rest.trim();
                        let resolved = if raw.is_empty() {
                            PathBuf::new()
                        } else if Path::new(raw).is_absolute() {
                            PathBuf::from(raw)
                        } else {
                            plugin_discovery_path(&resolution.repo_root, raw)
                        };
                        let normalized = resolved.to_string_lossy().replace('\\', "/");
                        if normalized.contains("/.git/worktrees/") {
                            return Ok(GitWorkspaceRepositoryInfo {
                                workspace_path: workspace_text,
                                kind: GitWorkspaceRepositoryKind::LinkedWorktree,
                                is_git_available: true,
                            });
                        }
                    }
                }
            }
        }
        Ok(main_tree_info(workspace_text))
    }

    pub fn get_status(&self, workspace_path: &Path) -> Result<GitStatusSnapshot, String> {
        let resolution = self.resolve_repository(workspace_path)?;
        if !resolution.is_git_available || !resolution.is_repository {
            return Ok(GitStatusSnapshot {
                summary: create_empty_summary(&resolution),
                resolution,
                entries: Vec::new(),
                staged_stats: Default::default(),
                unstaged_stats: Default::default(),
                untracked_stats: Default::default(),
            });
        }
        let status_result = self.run_git_status(&resolution)?;
        let staged_stats_result = self.run(
            &resolution.repo_root,
            vec![
                "diff".into(),
                "--cached".into(),
                "--numstat".into(),
                "-z".into(),
                "--find-renames".into(),
                "--".into(),
            ],
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        let unstaged_stats_result = self.run(
            &resolution.repo_root,
            vec![
                "diff".into(),
                "--numstat".into(),
                "-z".into(),
                "--find-renames".into(),
                "--".into(),
            ],
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        ensure_git_command_succeeded("git status", &status_result, &[0])?;
        ensure_git_command_succeeded("git diff --cached --numstat", &staged_stats_result, &[0])?;
        ensure_git_command_succeeded("git diff --numstat", &unstaged_stats_result, &[0])?;

        let parsed = parse_status_porcelain(&status_result.stdout);
        let untracked_stats = build_untracked_stats(&resolution.repo_root, &parsed.entries);
        let summary = git_cli::GitRepositorySummary {
            workspace_path: resolution.workspace_path.to_string_lossy().into_owned(),
            repo_root: resolution.repo_root.to_string_lossy().into_owned(),
            workspace_in_repo_path: resolution.workspace_in_repo_path.clone(),
            auto_refresh_watch_paths: resolution.auto_refresh_watch_paths.clone(),
            branch_name: parsed.branch_name.clone(),
            tracking_branch_name: parsed.tracking_branch_name.clone(),
            head_ref_type: parsed.head_ref_type,
            ahead: parsed.ahead,
            behind: parsed.behind,
            is_dirty: !parsed.entries.is_empty(),
            is_git_available: true,
            is_repository: true,
        };
        Ok(GitStatusSnapshot {
            resolution,
            summary,
            entries: parsed.entries,
            staged_stats: parse_numstat(&staged_stats_result.stdout),
            unstaged_stats: parse_numstat(&unstaged_stats_result.stdout),
            untracked_stats,
        })
    }

    pub fn get_ignored_paths(
        &self,
        workspace_path: &Path,
        paths: &[String],
    ) -> Result<Vec<String>, String> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let resolution = self.resolve_repository(workspace_path)?;
        if !resolution.is_git_available || !resolution.is_repository {
            return Ok(Vec::new());
        }
        let mut pairs: Vec<(String, String)> = Vec::new();
        for path in paths {
            let absolute = if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                path.split('/')
                    .fold(resolution.workspace_path.clone(), |base, segment| {
                        base.join(segment)
                    })
            };
            let Ok(relative) = normalize_input_path(&resolution, path) else {
                continue;
            };
            pairs.push((absolute.to_string_lossy().into_owned(), relative));
        }
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        let mut args = vec!["check-ignore".to_string(), "--".to_string()];
        args.extend(pairs.iter().map(|(_, relative)| relative.clone()));
        let result = self.run(
            &resolution.repo_root,
            args,
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        if result.exit_code == Some(1) {
            return Ok(Vec::new());
        }
        ensure_git_command_succeeded("git check-ignore", &result, &[0])?;
        let ignored: BTreeSet<String> = result
            .stdout
            .replace("\r\n", "\n")
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(normalize_git_path)
            .collect();
        Ok(pairs
            .into_iter()
            .filter(|(_, relative)| ignored.contains(relative))
            .map(|(absolute, _)| absolute)
            .collect())
    }

    pub fn list_local_branches(
        &self,
        workspace_path: &Path,
    ) -> Result<GitLocalBranchListResult, String> {
        let status = self.get_status(workspace_path)?;
        if !status.resolution.is_git_available || !status.resolution.is_repository {
            return Ok(GitLocalBranchListResult {
                head_ref_type: status.summary.head_ref_type,
                current_branch_name: status.summary.branch_name.clone(),
                branches: Vec::new(),
            });
        }
        let result = self.run(
            &status.resolution.repo_root,
            vec![
                "for-each-ref".into(),
                "refs/heads".into(),
                "--format=%(refname:short)%00%(upstream:short)%00%(objectname)%00%(committerdate:unix)"
                    .into(),
            ],
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        ensure_git_command_succeeded("git for-each-ref refs/heads", &result, &[0])?;
        let current = if status.summary.head_ref_type == GitHeadRefType::Branch {
            status.summary.branch_name.clone()
        } else {
            None
        };
        Ok(GitLocalBranchListResult {
            head_ref_type: status.summary.head_ref_type,
            current_branch_name: status.summary.branch_name.clone(),
            branches: parse_branch_ref_records(&result.stdout, current.as_deref()),
        })
    }

    pub fn get_commit_graph(
        &self,
        workspace_path: &Path,
        max_count: Option<u64>,
        skip: Option<u64>,
    ) -> Result<GitCommitGraphSnapshot, String> {
        let resolution = self.resolve_repository(workspace_path)?;
        if !resolution.is_git_available || !resolution.is_repository {
            return Ok(GitCommitGraphSnapshot {
                resolution,
                commits: Vec::new(),
                has_more: false,
            });
        }
        let normalized_max = normalize_graph_max_count(max_count);
        let normalized_skip = skip.unwrap_or(0);
        let result = self.run(
            &resolution.repo_root,
            vec![
                "log".into(),
                "HEAD".into(),
                "--branches".into(),
                "--tags".into(),
                "--remotes".into(),
                "--date-order".into(),
                "--topo-order".into(),
                format!("--skip={normalized_skip}"),
                format!("--max-count={}", normalized_max + 1),
                "--format=%H%x00%P%x00%an%x00%at%x00%s%x00%D%x1e".into(),
            ],
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        if result.exit_code != Some(0) {
            let stderr = result.stderr.to_lowercase();
            if stderr.contains("does not have any commits yet")
                || stderr.contains("your current branch")
                || stderr.contains("bad default revision")
                || stderr.contains("ambiguous argument 'head'")
            {
                return Ok(GitCommitGraphSnapshot {
                    resolution,
                    commits: Vec::new(),
                    has_more: false,
                });
            }
            ensure_git_command_succeeded("git log visible refs", &result, &[0])?;
        }
        let parsed = parse_graph_records(&result.stdout);
        let has_more = parsed.len() as u64 > normalized_max;
        let commits = parsed.into_iter().take(normalized_max as usize).collect();
        Ok(GitCommitGraphSnapshot {
            resolution,
            commits,
            has_more,
        })
    }

    pub fn switch_branch(
        &self,
        workspace_path: &Path,
        target_branch_name: &str,
    ) -> Result<GitBranchMutationResult, String> {
        let status = self.get_status(workspace_path)?;
        let resolution = status.resolution.clone();
        ensure_repository_available(&resolution, "switch branches")?;
        let normalized = target_branch_name.trim();
        if normalized.is_empty() {
            return Ok(branch_failure(
                GitBranchMutationAction::Switch,
                None,
                &status.summary,
                vec![to_invalid_branch_name_issue(None)],
            ));
        }
        if status.summary.head_ref_type == GitHeadRefType::Branch
            && status.summary.branch_name.as_deref() == Some(normalized)
        {
            return Ok(branch_success(
                GitBranchMutationAction::Switch,
                normalized,
                false,
                false,
                &status.summary,
            ));
        }
        if status.entries.iter().any(|entry| entry.is_conflicted) {
            return Ok(branch_failure(
                GitBranchMutationAction::Switch,
                Some(normalized.to_string()),
                &status.summary,
                vec![git_cli::GitBranchMutationIssue {
                    code: "conflicts-present".into(),
                    message: "Repository still has unresolved conflicts.".into(),
                    paths: None,
                    detail: None,
                }],
            ));
        }
        if self.has_operation_in_progress(&resolution)? {
            return Ok(branch_failure(
                GitBranchMutationAction::Switch,
                Some(normalized.to_string()),
                &status.summary,
                vec![git_cli::GitBranchMutationIssue {
                    code: "operation-in-progress".into(),
                    message: "Another Git operation is still in progress.".into(),
                    paths: None,
                    detail: None,
                }],
            ));
        }
        if let Some(issue) = self.validate_branch_name(&resolution, normalized)? {
            return Ok(branch_failure(
                GitBranchMutationAction::Switch,
                Some(normalized.to_string()),
                &status.summary,
                vec![issue],
            ));
        }
        let result = self.run(
            &resolution.repo_root,
            vec!["switch".into(), "--no-guess".into(), normalized.into()],
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        if result.exit_code != Some(0) {
            return Ok(branch_failure(
                GitBranchMutationAction::Switch,
                Some(normalized.to_string()),
                &status.summary,
                parse_git_branch_mutation_issues(&result),
            ));
        }
        self.invalidate(workspace_path);
        let next_status = self.get_status(workspace_path)?;
        Ok(branch_success(
            GitBranchMutationAction::Switch,
            normalized,
            true,
            false,
            &next_status.summary,
        ))
    }

    pub fn create_branch_and_switch(
        &self,
        workspace_path: &Path,
        branch_name: &str,
        start_point: Option<&str>,
    ) -> Result<GitBranchMutationResult, String> {
        let status = self.get_status(workspace_path)?;
        let resolution = status.resolution.clone();
        ensure_repository_available(&resolution, "create and switch branches")?;
        let normalized = branch_name.trim();
        if normalized.is_empty() {
            return Ok(branch_failure(
                GitBranchMutationAction::CreateAndSwitch,
                None,
                &status.summary,
                vec![to_invalid_branch_name_issue(None)],
            ));
        }
        if status.entries.iter().any(|entry| entry.is_conflicted) {
            return Ok(branch_failure(
                GitBranchMutationAction::CreateAndSwitch,
                Some(normalized.to_string()),
                &status.summary,
                vec![git_cli::GitBranchMutationIssue {
                    code: "conflicts-present".into(),
                    message: "Repository still has unresolved conflicts.".into(),
                    paths: None,
                    detail: None,
                }],
            ));
        }
        if self.has_operation_in_progress(&resolution)? {
            return Ok(branch_failure(
                GitBranchMutationAction::CreateAndSwitch,
                Some(normalized.to_string()),
                &status.summary,
                vec![git_cli::GitBranchMutationIssue {
                    code: "operation-in-progress".into(),
                    message: "Another Git operation is still in progress.".into(),
                    paths: None,
                    detail: None,
                }],
            ));
        }
        if let Some(issue) = self.validate_branch_name(&resolution, normalized)? {
            return Ok(branch_failure(
                GitBranchMutationAction::CreateAndSwitch,
                Some(normalized.to_string()),
                &status.summary,
                vec![issue],
            ));
        }
        let mut args = vec![
            "switch".to_string(),
            "--no-guess".to_string(),
            "-c".to_string(),
            normalized.to_string(),
        ];
        if let Some(start) = start_point.map(str::trim).filter(|value| !value.is_empty()) {
            args.push("--".to_string());
            args.push(start.to_string());
        }
        let result = self.run(
            &resolution.repo_root,
            args,
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        if result.exit_code != Some(0) {
            return Ok(branch_failure(
                GitBranchMutationAction::CreateAndSwitch,
                Some(normalized.to_string()),
                &status.summary,
                parse_git_branch_mutation_issues(&result),
            ));
        }
        self.invalidate(workspace_path);
        let next_status = self.get_status(workspace_path)?;
        Ok(branch_success(
            GitBranchMutationAction::CreateAndSwitch,
            normalized,
            true,
            true,
            &next_status.summary,
        ))
    }

    pub fn get_diff(&self, params: &GitDiffQuery) -> Result<GitDiffResult, String> {
        let resolution = self.resolve_repository(&params.workspace_path)?;
        let absolute_path = if Path::new(&params.path).is_absolute() {
            PathBuf::from(&params.path)
        } else {
            params
                .path
                .split('/')
                .fold(params.workspace_path.clone(), |base, segment| {
                    base.join(segment)
                })
        };
        let absolute_text = absolute_path.to_string_lossy().into_owned();
        if !resolution.is_git_available {
            return Ok(unavailable_diff(
                absolute_text,
                "Git binary is not available in the current environment.",
            ));
        }
        if !resolution.is_repository {
            return Ok(unavailable_diff(
                absolute_text,
                "Workspace is not inside a Git repository.",
            ));
        }
        let repo_relative = normalize_input_path(&resolution, &params.path)?;
        if params.source_id.as_deref() == Some("branch") {
            let status = self.get_status(&params.workspace_path)?;
            let Some(tracking) = status.summary.tracking_branch_name.clone() else {
                return Ok(unavailable_diff(
                    absolute_text,
                    "Current branch does not have an upstream branch.",
                ));
            };
            let branch_result = self.run(
                &resolution.repo_root,
                vec![
                    "diff".into(),
                    "--no-ext-diff".into(),
                    "--no-color".into(),
                    "--binary".into(),
                    "--find-renames".into(),
                    format!("{tracking}...HEAD"),
                    "--".into(),
                    repo_relative.clone(),
                ],
                Some(DEFAULT_GIT_DIFF_TIMEOUT_MS),
                Some(DEFAULT_GIT_DIFF_BYTES),
                Vec::new(),
            )?;
            let parsed = to_diff_result(
                &absolute_text,
                &branch_result,
                None,
                Some("No branch comparison diff is available for this file."),
                None,
            );
            let contents = self.read_branch_diff_contents(&resolution, &repo_relative, &tracking);
            return Ok(with_diff_contents(parsed, contents));
        }

        let staged = params
            .staged
            .unwrap_or(params.source_id.as_deref() == Some("staged"));
        let mut args = vec!["diff".to_string()];
        if staged {
            args.push("--cached".to_string());
        }
        args.extend([
            "--no-ext-diff".to_string(),
            "--no-color".to_string(),
            "--binary".to_string(),
            "--".to_string(),
            repo_relative.clone(),
        ]);
        let diff_result = self.run(
            &resolution.repo_root,
            args,
            Some(DEFAULT_GIT_DIFF_TIMEOUT_MS),
            Some(DEFAULT_GIT_DIFF_BYTES),
            Vec::new(),
        )?;
        let parsed = to_diff_result(
            &absolute_text,
            &diff_result,
            None,
            Some("No Git diff is available for this file."),
            None,
        );
        if parsed.availability != GitDiffAvailability::Unavailable || staged {
            let contents = if staged {
                self.read_staged_diff_contents(&resolution, &repo_relative)
            } else {
                self.read_unstaged_diff_contents(&resolution, &repo_relative, &absolute_path)
            };
            return Ok(with_diff_contents(parsed, contents));
        }

        if !file_exists(&absolute_path) {
            return Ok(parsed);
        }
        if let Some(untracked) =
            build_untracked_text_diff_result(&absolute_path, &repo_relative, DEFAULT_GIT_DIFF_BYTES)
        {
            return Ok(untracked);
        }
        let no_index_result = self.run(
            &resolution.repo_root,
            vec![
                "diff".into(),
                "--no-index".into(),
                "--no-ext-diff".into(),
                "--no-color".into(),
                "--binary".into(),
                git_cli::git_null_device_path().to_string(),
                absolute_text.clone(),
            ],
            Some(DEFAULT_GIT_DIFF_TIMEOUT_MS),
            Some(DEFAULT_GIT_DIFF_BYTES),
            Vec::new(),
        )?;
        Ok(to_diff_result(
            &absolute_text,
            &no_index_result,
            Some(&[0, 1]),
            Some("No previewable diff is available for this file."),
            None,
        ))
    }

    pub fn get_branch_comparison(
        &self,
        workspace_path: &Path,
    ) -> Result<GitBranchComparisonSnapshot, String> {
        let status = self.get_status(workspace_path)?;
        let Some(tracking) = status.summary.tracking_branch_name.clone() else {
            return Ok(empty_branch_comparison(&status));
        };
        if !status.resolution.is_git_available || !status.resolution.is_repository {
            return Ok(empty_branch_comparison(&status));
        }
        let result = self.run(
            &status.resolution.repo_root,
            vec![
                "diff".into(),
                "--numstat".into(),
                "-z".into(),
                "--find-renames".into(),
                format!("{tracking}...HEAD"),
                "--".into(),
            ],
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        ensure_git_command_succeeded("git diff --numstat upstream...HEAD", &result, &[0])?;
        let changes = parse_numstat(&result.stdout)
            .into_iter()
            .map(|(path, stat)| GitBranchComparisonChange {
                path,
                original_path: stat.original_path.clone(),
                kind: infer_kind_from_numstat(&stat),
                added: stat.added,
                removed: stat.removed,
            })
            .collect();
        let head_ref = status
            .summary
            .branch_name
            .clone()
            .unwrap_or_else(|| "HEAD".to_string());
        let comparison_label = match &status.summary.branch_name {
            Some(branch) => Some(format!("{branch} -> {tracking}")),
            None => Some(format!("HEAD -> {tracking}")),
        };
        Ok(GitBranchComparisonSnapshot {
            resolution: status.resolution,
            base_ref: Some(tracking),
            head_ref: Some(head_ref),
            comparison_label,
            changes,
        })
    }

    pub fn stage(&self, workspace_path: &Path, paths: &[String]) -> Result<(), String> {
        let resolution = self.resolve_repository(workspace_path)?;
        ensure_repository_available(&resolution, "stage paths")?;
        let repo_paths = normalize_repo_paths(&resolution, paths)?;
        if repo_paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["add".to_string(), "--".to_string()];
        args.extend(repo_paths);
        let result = self.run_default(&resolution.repo_root, args)?;
        ensure_git_command_succeeded("git add", &result, &[0])?;
        self.invalidate(workspace_path);
        Ok(())
    }

    pub fn unstage(&self, workspace_path: &Path, paths: &[String]) -> Result<(), String> {
        let resolution = self.resolve_repository(workspace_path)?;
        ensure_repository_available(&resolution, "unstage paths")?;
        let repo_paths = normalize_repo_paths(&resolution, paths)?;
        if repo_paths.is_empty() {
            return Ok(());
        }
        let mut args = vec_restore_staged(&repo_paths);
        args.insert(0, "restore".to_string());
        let result = self.run_default(&resolution.repo_root, args)?;
        ensure_git_command_succeeded("git restore --staged", &result, &[0])?;
        self.invalidate(workspace_path);
        Ok(())
    }

    pub fn discard(
        &self,
        workspace_path: &Path,
        paths: &[String],
        staged: bool,
    ) -> Result<(), String> {
        let resolution = self.resolve_repository(workspace_path)?;
        ensure_repository_available(&resolution, "discard paths")?;
        let repo_paths = normalize_repo_paths(&resolution, paths)?;
        if repo_paths.is_empty() {
            return Ok(());
        }
        let mut args = if staged {
            vec![
                "restore".to_string(),
                "--source=HEAD".to_string(),
                "--staged".to_string(),
                "--worktree".to_string(),
                "--".to_string(),
            ]
        } else {
            vec![
                "restore".to_string(),
                "--worktree".to_string(),
                "--".to_string(),
            ]
        };
        args.extend(repo_paths);
        let result = self.run_default(&resolution.repo_root, args)?;
        ensure_git_command_succeeded("git restore", &result, &[0])?;
        self.invalidate(workspace_path);
        Ok(())
    }

    pub fn commit(
        &self,
        workspace_path: &Path,
        message: &str,
        paths: Option<&[String]>,
        staged_only: bool,
    ) -> Result<String, String> {
        let resolution = self.resolve_repository(workspace_path)?;
        ensure_repository_available(&resolution, "commit changes")?;
        let trimmed_message = message.trim();
        if trimmed_message.is_empty() {
            return Err("Commit message cannot be empty".to_string());
        }
        let repo_paths = match paths {
            Some(paths) if !paths.is_empty() => normalize_repo_paths(&resolution, paths)?,
            _ => Vec::new(),
        };
        if staged_only && !repo_paths.is_empty() {
            return self.commit_selected_staged(
                workspace_path,
                &resolution,
                trimmed_message,
                &repo_paths,
            );
        }

        let mut args = vec![
            "commit".to_string(),
            "-m".to_string(),
            trimmed_message.to_string(),
        ];
        if !repo_paths.is_empty() {
            args.push("--".to_string());
            args.extend(repo_paths);
        }
        let commit_result = self.run(
            &resolution.repo_root,
            args,
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        ensure_git_command_succeeded("git commit", &commit_result, &[0])?;
        self.invalidate(workspace_path);
        let hash_result = self.run_default(
            &resolution.repo_root,
            vec!["rev-parse".into(), "HEAD".into()],
        )?;
        ensure_git_command_succeeded("git rev-parse HEAD", &hash_result, &[0])?;
        Ok(hash_result.stdout.trim().to_string())
    }

    fn commit_selected_staged(
        &self,
        workspace_path: &Path,
        resolution: &GitResolvedRepository,
        message: &str,
        repo_paths: &[String],
    ) -> Result<String, String> {
        let mut scoped_args = vec![
            "status".to_string(),
            "--porcelain=v2".to_string(),
            "-z".to_string(),
            "--".to_string(),
        ];
        scoped_args.extend(repo_paths.iter().cloned());
        let scoped_status = self.run(
            &resolution.repo_root,
            scoped_args,
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        ensure_git_command_succeeded("git status selected paths", &scoped_status, &[0])?;

        let mut cleanup: Vec<String> = repo_paths.to_vec();
        for entry in parse_status_porcelain(&scoped_status.stdout).entries {
            if repo_paths.contains(&entry.path) {
                if let Some(original) = entry.original_path {
                    if !cleanup.contains(&original) {
                        cleanup.push(original);
                    }
                }
            }
        }
        cleanup.sort();
        cleanup.dedup();

        let mut ls_args = vec![
            "ls-files".to_string(),
            "--stage".to_string(),
            "-z".to_string(),
            "--".to_string(),
        ];
        ls_args.extend(repo_paths.iter().cloned());
        let staged_entries_result = self.run(
            &resolution.repo_root,
            ls_args,
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        ensure_git_command_succeeded(
            "git ls-files selected staged entries",
            &staged_entries_result,
            &[0],
        )?;
        let staged_entries = parse_index_entries(&staged_entries_result.stdout)?;
        if staged_entries.iter().any(|entry| entry.stage != "0") {
            return Err(
                "Cannot commit selected staged paths while index conflicts exist.".to_string(),
            );
        }

        let head_result = self.run_default(
            &resolution.repo_root,
            vec!["rev-parse".into(), "--verify".into(), "HEAD".into()],
        )?;
        let parent_hash = if head_result.exit_code == Some(0) {
            Some(head_result.stdout.trim().to_string())
        } else {
            None
        };
        let temp_dir = std::env::temp_dir().join(format!(
            "mode-git-index-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&temp_dir).map_err(|error| error.to_string())?;
        let temp_index = temp_dir.join("index");
        let env = vec![(
            "GIT_INDEX_FILE".to_string(),
            Some(temp_index.to_string_lossy().into_owned()),
        )];

        let outcome = (|| -> Result<String, String> {
            let read_tree_args = match &parent_hash {
                Some(hash) => vec!["read-tree".to_string(), hash.clone()],
                None => vec!["read-tree".to_string(), "--empty".to_string()],
            };
            let read_tree = self.run(
                &resolution.repo_root,
                read_tree_args,
                Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
                None,
                env.clone(),
            )?;
            ensure_git_command_succeeded("git read-tree selected commit base", &read_tree, &[0])?;

            if !cleanup.is_empty() {
                let mut remove_args = vec![
                    "update-index".to_string(),
                    "--force-remove".to_string(),
                    "--".to_string(),
                ];
                remove_args.extend(cleanup.iter().cloned());
                let remove = self.run(
                    &resolution.repo_root,
                    remove_args,
                    Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
                    None,
                    env.clone(),
                )?;
                ensure_git_command_succeeded(
                    "git update-index remove selected paths",
                    &remove,
                    &[0],
                )?;
            }

            for entry in &staged_entries {
                let add = self.run(
                    &resolution.repo_root,
                    vec![
                        "update-index".to_string(),
                        "--add".to_string(),
                        "--cacheinfo".to_string(),
                        entry.mode.clone(),
                        entry.object_hash.clone(),
                        entry.path.clone(),
                    ],
                    Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
                    None,
                    env.clone(),
                )?;
                ensure_git_command_succeeded("git update-index add selected paths", &add, &[0])?;
            }

            let scoped_commit = self.run(
                &resolution.repo_root,
                vec!["commit".into(), "-m".into(), message.to_string()],
                Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
                Some(DEFAULT_GIT_OUTPUT_BYTES),
                env.clone(),
            )?;
            ensure_git_command_succeeded("git commit selected staged paths", &scoped_commit, &[0])?;

            let hash_result = self.run_default(
                &resolution.repo_root,
                vec!["rev-parse".into(), "HEAD".into()],
            )?;
            ensure_git_command_succeeded("git rev-parse selected commit HEAD", &hash_result, &[0])?;
            let commit_hash = hash_result.stdout.trim().to_string();

            let mut reset_args = vec![
                "reset".to_string(),
                "--quiet".to_string(),
                "HEAD".to_string(),
                "--".to_string(),
            ];
            reset_args.extend(cleanup.iter().cloned());
            let reset = self.run_default(&resolution.repo_root, reset_args)?;
            ensure_git_command_succeeded("git reset selected committed paths", &reset, &[0])?;
            self.invalidate(workspace_path);
            Ok(commit_hash)
        })();
        let _ = std::fs::remove_dir_all(&temp_dir);
        outcome
    }

    pub fn push(&self, workspace_path: &Path) -> Result<GitPushResult, String> {
        let status = self.get_status(workspace_path)?;
        let resolution = status.resolution.clone();
        ensure_repository_available(&resolution, "push changes")?;
        let branch_name = status
            .summary
            .branch_name
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();
        if status.summary.head_ref_type != GitHeadRefType::Branch || branch_name.is_empty() {
            return Err("Cannot push while HEAD is detached.".to_string());
        }
        let has_tracking = status.summary.tracking_branch_name.is_some();
        let remote_name = if has_tracking {
            parse_tracking_remote_name(status.summary.tracking_branch_name.as_deref())
        } else {
            Some(self.resolve_push_remote(&status)?)
        };
        let args = if has_tracking {
            vec!["push".to_string()]
        } else {
            vec![
                "push".to_string(),
                "--set-upstream".to_string(),
                remote_name.clone().unwrap_or_else(|| "origin".to_string()),
                branch_name.clone(),
            ]
        };
        let push_result = self.run(
            &resolution.repo_root,
            args,
            Some(DEFAULT_GIT_PUSH_TIMEOUT_MS),
            Some(DEFAULT_GIT_PUSH_OUTPUT_BYTES),
            Vec::new(),
        )?;
        ensure_git_command_succeeded("git push", &push_result, &[0])?;
        self.invalidate(workspace_path);
        let next_status = self.get_status(workspace_path)?;
        Ok(GitPushResult {
            branch_name: Some(branch_name),
            tracking_branch_name: next_status.summary.tracking_branch_name.clone(),
            remote_name: remote_name.or_else(|| {
                parse_tracking_remote_name(next_status.summary.tracking_branch_name.as_deref())
            }),
            set_upstream: !has_tracking,
            summary: next_status.summary,
        })
    }

    pub fn get_identity(&self, workspace_path: &Path) -> Result<GitIdentity, String> {
        let resolution = self.resolve_repository(workspace_path)?;
        if !resolution.is_git_available || !resolution.is_repository {
            return Ok(GitIdentity {
                user_name: None,
                user_email: None,
                name_source: None,
                email_source: None,
                scope_label: None,
            });
        }
        let name_result = self.run_default(
            &resolution.repo_root,
            vec![
                "config".into(),
                "--show-scope".into(),
                "--show-origin".into(),
                "--get".into(),
                "user.name".into(),
            ],
        )?;
        let email_result = self.run_default(
            &resolution.repo_root,
            vec![
                "config".into(),
                "--show-scope".into(),
                "--show-origin".into(),
                "--get".into(),
                "user.email".into(),
            ],
        )?;
        let name = parse_git_config_value(&name_result)?;
        let email = parse_git_config_value(&email_result)?;
        Ok(GitIdentity {
            scope_label: name.scope.clone().or(email.scope.clone()),
            user_name: name.value,
            user_email: email.value,
            name_source: name.source,
            email_source: email.source,
        })
    }

    fn execute_git_status(
        &self,
        resolution: &GitResolvedRepository,
        untracked_mode: &str,
    ) -> Result<GitCommandExecutionResult, String> {
        self.run(
            &resolution.repo_root,
            vec![
                "status".into(),
                "--porcelain=v2".into(),
                "--branch".into(),
                format!("--untracked-files={untracked_mode}"),
                "-z".into(),
            ],
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )
    }

    fn run_git_status(
        &self,
        resolution: &GitResolvedRepository,
    ) -> Result<GitCommandExecutionResult, String> {
        let collapsed = self
            .collapsed_untracked_repo_roots
            .lock()
            .unwrap()
            .contains(&resolution.repo_root);
        let result =
            self.execute_git_status(resolution, if collapsed { "normal" } else { "all" })?;
        if collapsed || !result.output_truncated {
            return Ok(result);
        }
        self.collapsed_untracked_repo_roots
            .lock()
            .unwrap()
            .insert(resolution.repo_root.clone());
        self.execute_git_status(resolution, "normal")
    }

    fn validate_branch_name(
        &self,
        resolution: &GitResolvedRepository,
        branch_name: &str,
    ) -> Result<Option<git_cli::GitBranchMutationIssue>, String> {
        let result = self.run(
            &resolution.repo_root,
            vec![
                "check-ref-format".into(),
                "--branch".into(),
                branch_name.to_string(),
            ],
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            None,
            Vec::new(),
        )?;
        if result.timed_out || result.output_truncated {
            ensure_git_command_succeeded("git check-ref-format --branch", &result, &[0])?;
        }
        if result.exit_code == Some(0) {
            return Ok(None);
        }
        Ok(Some(to_invalid_branch_name_issue(Some(&result.stderr))))
    }

    fn has_operation_in_progress(
        &self,
        resolution: &GitResolvedRepository,
    ) -> Result<bool, String> {
        let mut args = vec!["rev-parse".to_string()];
        for marker in GIT_OPERATION_MARKERS {
            args.push("--git-path".to_string());
            args.push(marker.to_string());
        }
        let result = self.run_default(&resolution.repo_root, args)?;
        ensure_git_command_succeeded("git rev-parse --git-path", &result, &[0])?;
        let paths: Vec<PathBuf> = result
            .stdout
            .replace("\r\n", "\n")
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                let path = PathBuf::from(line);
                if path.is_absolute() {
                    path
                } else {
                    plugin_discovery_path(&resolution.repo_root, line)
                }
            })
            .collect();
        Ok(paths.iter().any(|path| file_exists(path)))
    }

    fn read_optional_git_config(
        &self,
        resolution: &GitResolvedRepository,
        key: &str,
    ) -> Result<Option<String>, String> {
        let result = self.run(
            &resolution.repo_root,
            vec!["config".into(), "--get".into(), key.to_string()],
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            None,
            Vec::new(),
        )?;
        if result.timed_out || result.output_truncated {
            ensure_git_command_succeeded(&format!("git config --get {key}"), &result, &[0])?;
        }
        if result.exit_code == Some(0) {
            let value = result.stdout.trim().to_string();
            return Ok(if value.is_empty() { None } else { Some(value) });
        }
        if result.exit_code == Some(1) {
            return Ok(None);
        }
        ensure_git_command_succeeded(&format!("git config --get {key}"), &result, &[0])?;
        Ok(None)
    }

    fn list_remotes(&self, resolution: &GitResolvedRepository) -> Result<Vec<String>, String> {
        let result = self.run(
            &resolution.repo_root,
            vec!["remote".into()],
            Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS),
            Some(DEFAULT_GIT_OUTPUT_BYTES),
            Vec::new(),
        )?;
        ensure_git_command_succeeded("git remote", &result, &[0])?;
        Ok(parse_remote_list(&result.stdout))
    }

    fn resolve_push_remote(&self, status: &GitStatusSnapshot) -> Result<String, String> {
        let resolution = status.resolution.clone();
        ensure_repository_available(&resolution, "push changes")?;
        let branch_name = status
            .summary
            .branch_name
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();
        if branch_name.is_empty() {
            return Err("Cannot resolve a Git push remote without a current branch.".to_string());
        }
        if let Some(remote) =
            self.read_optional_git_config(&resolution, &format!("branch.{branch_name}.remote"))?
        {
            return Ok(remote);
        }
        if let Some(remote) = self.read_optional_git_config(&resolution, "remote.pushDefault")? {
            return Ok(remote);
        }
        let remotes = self.list_remotes(&resolution)?;
        if remotes.iter().any(|remote| remote == "origin") {
            return Ok("origin".to_string());
        }
        if remotes.len() == 1 {
            return Ok(remotes[0].clone());
        }
        if remotes.is_empty() {
            return Err("No Git remote is configured for the current repository.".to_string());
        }
        Err("Multiple Git remotes are configured. Configure branch.<name>.remote or remote.pushDefault first.".to_string())
    }

    fn read_branch_diff_contents(
        &self,
        resolution: &GitResolvedRepository,
        repo_relative: &str,
        tracking_branch: &str,
    ) -> Option<DiffContents> {
        let merge_base_result = self
            .run_default(
                &resolution.repo_root,
                vec![
                    "merge-base".into(),
                    tracking_branch.to_string(),
                    "HEAD".into(),
                ],
            )
            .ok()?;
        let merge_base = if merge_base_result.exit_code == Some(0) {
            merge_base_result.stdout.trim().to_string()
        } else {
            String::new()
        };
        let before = self.read_git_blob_preview(
            resolution,
            if merge_base.is_empty() {
                tracking_branch
            } else {
                &merge_base
            },
            repo_relative,
        );
        let after = self.read_git_blob_preview(resolution, "HEAD", repo_relative);
        complete_contents(before, after)
    }

    fn read_staged_diff_contents(
        &self,
        resolution: &GitResolvedRepository,
        repo_relative: &str,
    ) -> Option<DiffContents> {
        let before = self.read_git_blob_preview(resolution, "HEAD", repo_relative);
        let after = self.read_git_blob_preview(resolution, "", repo_relative);
        complete_contents(before, after)
    }

    fn read_unstaged_diff_contents(
        &self,
        resolution: &GitResolvedRepository,
        repo_relative: &str,
        absolute_path: &Path,
    ) -> Option<DiffContents> {
        let before = self.read_git_blob_preview(resolution, "", repo_relative);
        let after = read_working_tree_preview(absolute_path);
        complete_contents(before, after)
    }

    fn read_git_blob_preview(
        &self,
        resolution: &GitResolvedRepository,
        reference: &str,
        repo_relative: &str,
    ) -> Option<String> {
        let result = self
            .run(
                &resolution.repo_root,
                vec!["show".to_string(), format!("{reference}:{repo_relative}")],
                Some(DEFAULT_GIT_DIFF_TIMEOUT_MS),
                Some(DEFAULT_GIT_DIFF_BYTES),
                Vec::new(),
            )
            .ok()?;
        if result.timed_out
            || result.output_truncated
            || result.exit_code != Some(0)
            || result.stdout.contains('\0')
        {
            return None;
        }
        Some(result.stdout)
    }
}

struct DiffContents {
    before: String,
    after: String,
}

fn complete_contents(before: Option<String>, after: Option<String>) -> Option<DiffContents> {
    Some(DiffContents {
        before: before?,
        after: after?,
    })
}

fn read_working_tree_preview(absolute_path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(absolute_path).ok()?;
    if !metadata.is_file() || metadata.len() as usize > DEFAULT_GIT_DIFF_BYTES {
        return None;
    }
    let bytes = std::fs::read(absolute_path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn with_diff_contents(diff: GitDiffResult, contents: Option<DiffContents>) -> GitDiffResult {
    if diff.availability != GitDiffAvailability::Patch {
        return diff;
    }
    let Some(contents) = contents else {
        return diff;
    };
    GitDiffResult {
        before_content: Some(contents.before),
        after_content: Some(contents.after),
        ..diff
    }
}

fn unavailable_diff(path: String, summary: &str) -> GitDiffResult {
    GitDiffResult {
        path,
        availability: GitDiffAvailability::Unavailable,
        patch: None,
        before_content: None,
        after_content: None,
        summary: Some(summary.to_string()),
    }
}

fn unavailable_repository(workspace_path: &Path, git_available: bool) -> GitResolvedRepository {
    GitResolvedRepository {
        workspace_path: workspace_path.to_path_buf(),
        repo_root: workspace_path.to_path_buf(),
        workspace_in_repo_path: ".".to_string(),
        auto_refresh_watch_paths: Vec::new(),
        is_git_available: git_available,
        is_repository: false,
    }
}

fn main_tree_info(workspace_path: String) -> GitWorkspaceRepositoryInfo {
    GitWorkspaceRepositoryInfo {
        workspace_path,
        kind: GitWorkspaceRepositoryKind::MainTree,
        is_git_available: true,
    }
}

fn empty_branch_comparison(status: &GitStatusSnapshot) -> GitBranchComparisonSnapshot {
    GitBranchComparisonSnapshot {
        resolution: status.resolution.clone(),
        base_ref: status.summary.tracking_branch_name.clone(),
        head_ref: Some(
            status
                .summary
                .branch_name
                .clone()
                .unwrap_or_else(|| "HEAD".to_string()),
        ),
        comparison_label: None,
        changes: Vec::new(),
    }
}

fn branch_failure(
    action: GitBranchMutationAction,
    branch_name: Option<String>,
    summary: &GitRepositorySummaryAlias,
    issues: Vec<git_cli::GitBranchMutationIssue>,
) -> GitBranchMutationResult {
    GitBranchMutationResult {
        ok: false,
        action,
        branch_name,
        did_change: false,
        created: false,
        summary: summary.clone(),
        issues,
    }
}

type GitRepositorySummaryAlias = git_cli::GitRepositorySummary;

fn branch_success(
    action: GitBranchMutationAction,
    branch_name: &str,
    did_change: bool,
    created: bool,
    summary: &GitRepositorySummaryAlias,
) -> GitBranchMutationResult {
    GitBranchMutationResult {
        ok: true,
        action,
        branch_name: Some(branch_name.to_string()),
        did_change,
        created,
        summary: summary.clone(),
        issues: Vec::new(),
    }
}

fn normalize_repo_paths(
    resolution: &GitResolvedRepository,
    paths: &[String],
) -> Result<Vec<String>, String> {
    let mut result: Vec<String> = Vec::new();
    for path in paths {
        let normalized = normalize_input_path(resolution, path)?;
        if !result.contains(&normalized) {
            result.push(normalized);
        }
    }
    Ok(result)
}

fn vec_restore_staged(repo_paths: &[String]) -> Vec<String> {
    let mut args = vec!["--staged".to_string(), "--".to_string()];
    args.extend(repo_paths.iter().cloned());
    args
}

fn normalize_graph_max_count(max_count: Option<u64>) -> u64 {
    match max_count {
        Some(value) => value.clamp(1, MAX_GIT_GRAPH_MAX_COUNT),
        None => DEFAULT_GIT_GRAPH_MAX_COUNT,
    }
}

fn parse_tracking_remote_name(tracking_branch_name: Option<&str>) -> Option<String> {
    let remote = tracking_branch_name?.split('/').next().unwrap_or("").trim();
    if remote.is_empty() {
        None
    } else {
        Some(remote.to_string())
    }
}

fn parse_remote_list(stdout: &str) -> Vec<String> {
    stdout
        .replace("\r\n", "\n")
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_branch_ref_records(
    stdout: &str,
    current_branch_name: Option<&str>,
) -> Vec<GitLocalBranch> {
    let mut branches: Vec<GitLocalBranch> = stdout
        .replace("\r\n", "\n")
        .split('\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\0').collect();
            let name = fields.first().copied().unwrap_or("").to_string();
            if name.is_empty() {
                return None;
            }
            let upstream = fields.get(1).copied().unwrap_or("");
            let hash = fields.get(2).copied().unwrap_or("");
            let timestamp = fields.get(3).copied().unwrap_or("");
            Some(GitLocalBranch {
                is_current: Some(name.as_str()) == current_branch_name,
                name,
                upstream_name: if upstream.is_empty() {
                    None
                } else {
                    Some(upstream.to_string())
                },
                commit_hash: if hash.is_empty() {
                    None
                } else {
                    Some(hash.to_string())
                },
                commit_timestamp_ms: timestamp
                    .trim()
                    .parse::<i64>()
                    .ok()
                    .map(|seconds| seconds * 1000),
            })
        })
        .collect();
    branches.sort_by(|left, right| {
        if left.is_current != right.is_current {
            return if left.is_current {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        let left_ts = left.commit_timestamp_ms.unwrap_or(i64::MIN);
        let right_ts = right.commit_timestamp_ms.unwrap_or(i64::MIN);
        if left_ts != right_ts {
            return right_ts.cmp(&left_ts);
        }
        plugin_discovery::js::locale_compare(&left.name, &right.name)
    });
    branches
}

fn add_graph_ref(refs: &mut Vec<GitCommitGraphRef>, candidate: GitCommitGraphRef) {
    if refs
        .iter()
        .any(|existing| existing.kind == candidate.kind && existing.name == candidate.name)
    {
        return;
    }
    refs.push(candidate);
}

fn parse_graph_decoration_ref(raw: &str) -> Option<GitCommitGraphRef> {
    let reference = raw.trim();
    if reference.is_empty() {
        return None;
    }
    if reference == "HEAD" {
        return Some(GitCommitGraphRef {
            name: "HEAD".to_string(),
            kind: GitCommitGraphRefKind::Head,
        });
    }
    if let Some(rest) = reference.strip_prefix("tag: ") {
        let tag_ref = rest.trim();
        let name = tag_ref.strip_prefix("refs/tags/").unwrap_or(tag_ref);
        return (!name.is_empty()).then(|| GitCommitGraphRef {
            name: name.to_string(),
            kind: GitCommitGraphRefKind::Tag,
        });
    }
    for (prefix, kind) in [
        ("refs/heads/", GitCommitGraphRefKind::Branch),
        ("refs/remotes/", GitCommitGraphRefKind::Remote),
        ("refs/tags/", GitCommitGraphRefKind::Tag),
    ] {
        if let Some(name) = reference.strip_prefix(prefix) {
            return (!name.is_empty()).then(|| GitCommitGraphRef {
                name: name.to_string(),
                kind,
            });
        }
    }
    Some(GitCommitGraphRef {
        name: reference.to_string(),
        kind: if reference.contains('/') {
            GitCommitGraphRefKind::Remote
        } else {
            GitCommitGraphRefKind::Branch
        },
    })
}

fn parse_graph_refs(raw_decorations: &str) -> Vec<GitCommitGraphRef> {
    let mut refs = Vec::new();
    for raw in raw_decorations.split(',') {
        let decoration = raw.trim();
        if decoration.is_empty() {
            continue;
        }
        if let Some(rest) = decoration.strip_prefix("HEAD -> ") {
            add_graph_ref(
                &mut refs,
                GitCommitGraphRef {
                    name: "HEAD".to_string(),
                    kind: GitCommitGraphRefKind::Head,
                },
            );
            if let Some(pointed) = parse_graph_decoration_ref(rest) {
                add_graph_ref(&mut refs, pointed);
            }
            continue;
        }
        if let Some(parsed) = parse_graph_decoration_ref(decoration) {
            add_graph_ref(&mut refs, parsed);
        }
    }
    refs
}

fn parse_graph_records(stdout: &str) -> Vec<GitCommitGraphCommit> {
    stdout
        .split('\u{1e}')
        .map(str::trim)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.split('\u{0}').collect();
            let hash = fields.first().copied().unwrap_or("");
            if hash.is_empty() {
                return None;
            }
            Some(GitCommitGraphCommit {
                hash: hash.to_string(),
                parents: fields
                    .get(1)
                    .copied()
                    .unwrap_or("")
                    .split(' ')
                    .filter(|parent| !parent.is_empty())
                    .map(str::to_string)
                    .collect(),
                refs: parse_graph_refs(fields.get(5).copied().unwrap_or("")),
                subject: fields.get(4).copied().unwrap_or("").to_string(),
                author_name: fields
                    .get(2)
                    .copied()
                    .filter(|name| !name.is_empty())
                    .map(str::to_string),
                authored_at_ms: fields
                    .get(3)
                    .copied()
                    .unwrap_or("")
                    .trim()
                    .parse::<i64>()
                    .ok()
                    .map(|seconds| seconds * 1000),
            })
        })
        .collect()
}

struct GitIndexEntry {
    mode: String,
    object_hash: String,
    stage: String,
    path: String,
}

fn parse_index_entries(stdout: &str) -> Result<Vec<GitIndexEntry>, String> {
    let mut entries = Vec::new();
    for record in stdout.split('\0').filter(|record| !record.is_empty()) {
        let Some(tab_index) = record.find('\t') else {
            return Err("Failed to parse staged Git index entry.".to_string());
        };
        let header = record[..tab_index].trim();
        let fields: Vec<&str> = header.split_whitespace().collect();
        if fields.len() < 3 {
            return Err("Failed to parse staged Git index entry.".to_string());
        }
        let path = normalize_git_path(&record[tab_index + 1..]);
        if path.is_empty() {
            return Err("Failed to parse staged Git index entry.".to_string());
        }
        entries.push(GitIndexEntry {
            mode: fields[0].to_string(),
            object_hash: fields[1].to_string(),
            stage: fields[2].to_string(),
            path,
        });
    }
    Ok(entries)
}

/// `resolve(base, relative)`（Node 语义）。
fn plugin_discovery_path(base: &Path, relative: &str) -> PathBuf {
    let joined = if Path::new(relative).is_absolute() {
        PathBuf::from(relative)
    } else {
        base.join(relative)
    };
    normalize_lexical(&joined)
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(parts.last(), Some(std::path::Component::Normal(_))) {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    parts.into_iter().collect()
}

fn normalize_watch_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed == "/" {
        return trimmed.to_string();
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return trimmed.to_string();
    }
    if bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return trimmed.to_string();
    }
    trimmed.trim_end_matches(['\\', '/']).to_string()
}

fn add_watch_path(
    paths: &mut Vec<git_cli::GitRepositoryAutoRefreshWatchPath>,
    path: &str,
    recursive: bool,
) {
    let normalized = normalize_watch_path(path);
    if normalized.is_empty() || paths.iter().any(|entry| entry.path == normalized) {
        return;
    }
    paths.push(git_cli::GitRepositoryAutoRefreshWatchPath {
        path: normalized,
        recursive,
    });
}

fn build_watch_paths(
    workspace_path: &Path,
    absolute_git_dir: &str,
    git_common_dir: &str,
) -> Vec<git_cli::GitRepositoryAutoRefreshWatchPath> {
    let mut paths = Vec::new();
    add_watch_path(&mut paths, absolute_git_dir, true);
    let resolved_common = if git_common_dir.is_empty() {
        absolute_git_dir.to_string()
    } else if Path::new(git_common_dir).is_absolute() {
        git_common_dir.to_string()
    } else {
        plugin_discovery_path(workspace_path, git_common_dir)
            .to_string_lossy()
            .into_owned()
    };
    add_watch_path(&mut paths, &resolved_common, true);
    paths
}
