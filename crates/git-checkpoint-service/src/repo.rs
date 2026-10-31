//! Checkpoint repo 层（`gitCheckpointRepo.ts`）：隐藏 commit、diff、restore、删除引用。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use git_cli::{
    ensure_git_command_succeeded, normalize_git_path, to_workspace_relative_git_path,
    GitChangeKind, GitCommandExecutionOptions, GitCommandExecutionResult, GitCommandProvider,
    GitResolvedRepository, DEFAULT_GIT_COMMAND_TIMEOUT_MS,
};
use git_service::GitCliRepo;

use crate::types::{
    GitCheckpointConflict, GitCheckpointDiff, GitCheckpointFileDiff, GitCheckpointMeta,
    GitCheckpointRestoreResult,
};

pub struct GitCheckpointRepo {
    provider: Arc<GitCommandProvider>,
    git_repo: GitCliRepo,
    index_root_dir: PathBuf,
}

impl GitCheckpointRepo {
    pub fn new(
        provider: Arc<GitCommandProvider>,
        git_repo: GitCliRepo,
        index_root_dir: PathBuf,
    ) -> Self {
        Self {
            provider,
            git_repo,
            index_root_dir,
        }
    }

    fn run(
        &self,
        cwd: &Path,
        args: Vec<String>,
        env: Vec<(String, Option<String>)>,
    ) -> Result<GitCommandExecutionResult, String> {
        let mut options = GitCommandExecutionOptions::new(cwd.to_path_buf(), args);
        options.timeout_ms = Some(DEFAULT_GIT_COMMAND_TIMEOUT_MS);
        options.env = env;
        self.provider.run(&options)
    }

    fn ensure_repository(&self, workspace_path: &Path) -> Result<GitResolvedRepository, String> {
        let resolution = self.git_repo.resolve_repository(workspace_path)?;
        if !resolution.is_git_available {
            return Err("Git binary is not available in the current environment.".to_string());
        }
        if !resolution.is_repository {
            return Err("Workspace is not inside a Git repository.".to_string());
        }
        Ok(resolution)
    }

    pub fn create_checkpoint(
        &self,
        workspace_path: &Path,
        checkpoint_id: &str,
    ) -> Result<GitCheckpointMeta, String> {
        let resolution = self.ensure_repository(workspace_path)?;
        let workspace_text = workspace_path.to_string_lossy().into_owned();
        let ref_name = get_checkpoint_ref_name(&workspace_text, checkpoint_id);
        std::fs::create_dir_all(&self.index_root_dir).map_err(|error| error.to_string())?;
        let temp_dir = self.index_root_dir.join(format!(
            "index-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&temp_dir).map_err(|error| error.to_string())?;
        let temp_index = temp_dir.join("index");
        let env = checkpoint_env(&temp_index);
        let pathspec = workspace_pathspec(&resolution.workspace_in_repo_path);

        let outcome = (|| -> Result<GitCheckpointMeta, String> {
            let mut primed = false;
            if let Ok(index_path_result) = self.run(
                &resolution.repo_root,
                vec!["rev-parse".into(), "--git-path".into(), "index".into()],
                Vec::new(),
            ) {
                if index_path_result.exit_code == Some(0) {
                    let index_rel = index_path_result.stdout.trim();
                    if !index_rel.is_empty() {
                        let user_index = resolve_relative(&resolution.repo_root, index_rel);
                        if std::fs::copy(&user_index, &temp_index).is_ok() {
                            primed = true;
                        }
                    }
                }
            }
            if !primed {
                let read_tree = self.run(
                    &resolution.repo_root,
                    vec!["read-tree".into(), "HEAD".into()],
                    env.clone(),
                )?;
                if read_tree.exit_code == Some(0) {
                    primed = true;
                }
            }
            let _ = primed;

            let add_result = self.run(
                &resolution.repo_root,
                vec!["add".into(), "-A".into(), "--".into(), pathspec.clone()],
                env.clone(),
            )?;
            ensure_git_command_succeeded("git add checkpoint", &add_result, &[0])?;

            let tree_result = self.run(
                &resolution.repo_root,
                vec!["write-tree".into()],
                env.clone(),
            )?;
            ensure_git_command_succeeded("git write-tree checkpoint", &tree_result, &[0])?;
            let tree_oid = tree_result.stdout.trim().to_string();

            let commit_result = self.run(
                &resolution.repo_root,
                vec![
                    "commit-tree".into(),
                    tree_oid,
                    "-m".into(),
                    format!("mode checkpoint {checkpoint_id}"),
                ],
                env,
            )?;
            ensure_git_command_succeeded("git commit-tree checkpoint", &commit_result, &[0])?;
            let commit_oid = commit_result.stdout.trim().to_string();

            let update_ref = self.run(
                &resolution.repo_root,
                vec!["update-ref".into(), ref_name.clone(), commit_oid.clone()],
                Vec::new(),
            )?;
            ensure_git_command_succeeded("git update-ref checkpoint", &update_ref, &[0])?;

            Ok(GitCheckpointMeta {
                checkpoint_id: checkpoint_id.to_string(),
                workspace_path: workspace_text,
                repo_root: resolution.repo_root.to_string_lossy().into_owned(),
                workspace_in_repo_path: resolution.workspace_in_repo_path.clone(),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .unwrap_or(0),
                ref_name,
                commit_oid,
                scope: "workspace".to_string(),
            })
        })();
        let _ = std::fs::remove_dir_all(&temp_dir);
        outcome
    }

    pub fn diff_checkpoints(
        &self,
        workspace_path: &Path,
        from: &GitCheckpointMeta,
        to: &GitCheckpointMeta,
    ) -> Result<GitCheckpointDiff, String> {
        self.compute_diff(workspace_path, from, to)
    }

    fn compute_diff(
        &self,
        workspace_path: &Path,
        from: &GitCheckpointMeta,
        to: &GitCheckpointMeta,
    ) -> Result<GitCheckpointDiff, String> {
        let resolution = self.ensure_repository(workspace_path)?;
        let pathspec = workspace_pathspec(&resolution.workspace_in_repo_path);
        let name_status = self.run(
            &resolution.repo_root,
            vec![
                "diff".into(),
                "--name-status".into(),
                "--find-renames".into(),
                "-z".into(),
                from.commit_oid.clone(),
                to.commit_oid.clone(),
                "--".into(),
                pathspec.clone(),
            ],
            Vec::new(),
        )?;
        let numstat = self.run(
            &resolution.repo_root,
            vec![
                "diff".into(),
                "--numstat".into(),
                "--find-renames".into(),
                "-z".into(),
                from.commit_oid.clone(),
                to.commit_oid.clone(),
                "--".into(),
                pathspec,
            ],
            Vec::new(),
        )?;
        ensure_git_command_succeeded("git diff --name-status checkpoint", &name_status, &[0])?;
        ensure_git_command_succeeded("git diff --numstat checkpoint", &numstat, &[0])?;
        Ok(merge_diff(
            &resolution,
            &from.checkpoint_id,
            &to.checkpoint_id,
            parse_name_status(&name_status.stdout),
            &parse_numstat(&numstat.stdout),
        ))
    }

    pub fn restore_between_checkpoints(
        &self,
        workspace_path: &Path,
        from: &GitCheckpointMeta,
        to: &GitCheckpointMeta,
        force: bool,
    ) -> Result<GitCheckpointRestoreResult, String> {
        let resolution = self.ensure_repository(workspace_path)?;
        let diff = self.compute_diff(workspace_path, from, to)?;
        let affected: Vec<String> = build_affected_repo_paths(&diff.files)
            .into_iter()
            .map(|path| normalize_affected_repo_path(&resolution.repo_root, &path))
            .collect();
        if affected.is_empty() {
            return Ok(GitCheckpointRestoreResult {
                success: true,
                conflicts: None,
                restored_paths: Some(Vec::new()),
            });
        }

        let conflicts = self.collect_workspace_conflicts(&resolution, from, &affected)?;
        if !conflicts.is_empty() && !force {
            return Ok(GitCheckpointRestoreResult {
                success: false,
                conflicts: Some(conflicts),
                restored_paths: None,
            });
        }

        let restore_paths: Vec<String> = diff
            .files
            .iter()
            .filter(|file| file.kind != GitChangeKind::Deleted)
            .map(|file| file.repo_relative_path.clone())
            .collect();
        if !restore_paths.is_empty() {
            let mut args = vec![
                "restore".to_string(),
                format!("--source={}", to.commit_oid),
                "--worktree".to_string(),
                "--".to_string(),
            ];
            args.extend(restore_paths);
            let restore_result = self.run(&resolution.repo_root, args, Vec::new())?;
            ensure_git_command_succeeded("git restore checkpoint", &restore_result, &[0])?;
        }

        let mut delete_absolute: BTreeMap<String, ()> = BTreeMap::new();
        for file in &diff.files {
            if file.kind == GitChangeKind::Deleted {
                delete_absolute.insert(file.path.clone(), ());
                continue;
            }
            if file.kind == GitChangeKind::Renamed {
                if let Some(original) = &file.original_path {
                    delete_absolute.insert(original.clone(), ());
                }
            }
        }
        for path in delete_absolute.keys() {
            let _ = std::fs::remove_dir_all(path);
            let _ = std::fs::remove_file(path);
        }

        let verify = self.collect_workspace_conflicts(&resolution, to, &affected)?;
        if !verify.is_empty() {
            return Err("Checkpoint restore verification failed.".to_string());
        }
        Ok(GitCheckpointRestoreResult {
            success: true,
            conflicts: None,
            restored_paths: Some(diff.files.iter().map(|file| file.path.clone()).collect()),
        })
    }

    fn collect_workspace_conflicts(
        &self,
        resolution: &GitResolvedRepository,
        from: &GitCheckpointMeta,
        affected_repo_paths: &[String],
    ) -> Result<Vec<GitCheckpointConflict>, String> {
        if affected_repo_paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut args = vec![
            "ls-tree".to_string(),
            "-r".to_string(),
            "-z".to_string(),
            from.commit_oid.clone(),
            "--".to_string(),
        ];
        args.extend(affected_repo_paths.iter().cloned());
        let tree_result = self.run(&resolution.repo_root, args, Vec::new())?;
        ensure_git_command_succeeded("git ls-tree checkpoint paths", &tree_result, &[0])?;
        let tree_entries = parse_ls_tree(&tree_result.stdout);

        let mut conflicts: Vec<GitCheckpointConflict> = Vec::new();
        for repo_relative_path in affected_repo_paths {
            let absolute = to_absolute_path(&resolution.repo_root, repo_relative_path);
            let workspace_relative = to_workspace_relative_git_path(
                repo_relative_path,
                &resolution.workspace_in_repo_path,
            );
            let Some(expected) = tree_entries.get(repo_relative_path) else {
                if !path_exists(&absolute) {
                    continue;
                }
                push_conflict(
                    &mut conflicts,
                    repo_relative_path,
                    &absolute,
                    workspace_relative,
                    "unexpected-file-in-worktree",
                );
                continue;
            };
            let Ok(metadata) = std::fs::symlink_metadata(&absolute) else {
                push_conflict(
                    &mut conflicts,
                    repo_relative_path,
                    &absolute,
                    workspace_relative,
                    "missing-in-worktree",
                );
                continue;
            };
            let expects_symlink = expected.mode == "120000";
            if metadata.is_dir() || (expects_symlink && !metadata.file_type().is_symlink()) {
                push_conflict(
                    &mut conflicts,
                    repo_relative_path,
                    &absolute,
                    workspace_relative,
                    "type-mismatch",
                );
                continue;
            }
            let hash_result = self.run(
                &resolution.repo_root,
                vec![
                    "hash-object".into(),
                    "--no-filters".into(),
                    absolute.to_string_lossy().into_owned(),
                ],
                Vec::new(),
            )?;
            ensure_git_command_succeeded("git hash-object checkpoint verify", &hash_result, &[0])?;
            if hash_result.stdout.trim() == expected.object_id {
                continue;
            }
            push_conflict(
                &mut conflicts,
                repo_relative_path,
                &absolute,
                workspace_relative,
                "content-mismatch",
            );
        }
        Ok(conflicts)
    }

    pub fn delete_checkpoint(
        &self,
        workspace_path: &Path,
        checkpoint: &GitCheckpointMeta,
    ) -> Result<(), String> {
        let resolution = self.ensure_repository(workspace_path)?;
        let result = self.run(
            &resolution.repo_root,
            vec![
                "update-ref".into(),
                "-d".into(),
                checkpoint.ref_name.clone(),
            ],
            Vec::new(),
        )?;
        ensure_git_command_succeeded("git update-ref -d checkpoint", &result, &[0, 1])
    }
}

fn push_conflict(
    conflicts: &mut Vec<GitCheckpointConflict>,
    repo_relative_path: &str,
    absolute: &Path,
    workspace_relative: String,
    reason: &str,
) {
    let conflict = GitCheckpointConflict {
        path: absolute.to_string_lossy().into_owned(),
        repo_relative_path: repo_relative_path.to_string(),
        workspace_relative_path: workspace_relative,
        reason: reason.to_string(),
    };
    if let Some(existing) = conflicts
        .iter_mut()
        .find(|existing| existing.repo_relative_path == repo_relative_path)
    {
        *existing = conflict;
        return;
    }
    conflicts.push(conflict);
}

fn workspace_pathspec(workspace_in_repo_path: &str) -> String {
    if workspace_in_repo_path == "." {
        ".".to_string()
    } else {
        workspace_in_repo_path.to_string()
    }
}

fn get_checkpoint_ref_name(workspace_path: &str, checkpoint_id: &str) -> String {
    let hash = workspace_identity::workspace_hash_for(workspace_path, None);
    format!("refs/mode/checkpoints/{hash}/{checkpoint_id}")
}

fn checkpoint_env(temp_index: &Path) -> Vec<(String, Option<String>)> {
    vec![
        (
            "GIT_INDEX_FILE".to_string(),
            Some(temp_index.to_string_lossy().into_owned()),
        ),
        (
            "GIT_AUTHOR_NAME".to_string(),
            Some("Mode Checkpoint".to_string()),
        ),
        (
            "GIT_AUTHOR_EMAIL".to_string(),
            Some("checkpoint@mode.local".to_string()),
        ),
        (
            "GIT_COMMITTER_NAME".to_string(),
            Some("Mode Checkpoint".to_string()),
        ),
        (
            "GIT_COMMITTER_EMAIL".to_string(),
            Some("checkpoint@mode.local".to_string()),
        ),
    ]
}

fn resolve_relative(base: &Path, relative: &str) -> PathBuf {
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

fn to_absolute_path(repo_root: &Path, repo_relative_path: &str) -> PathBuf {
    normalize_git_path(repo_relative_path)
        .split('/')
        .filter(|segment| !segment.is_empty())
        .fold(repo_root.to_path_buf(), |path, segment| path.join(segment))
}

fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

struct NameStatusEntry {
    kind: GitChangeKind,
    path: String,
    original_path: Option<String>,
}

fn parse_name_status(stdout: &str) -> Vec<NameStatusEntry> {
    let records: Vec<&str> = stdout
        .split('\0')
        .filter(|record| !record.is_empty())
        .collect();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let status = records[index];
        let kind = map_name_status_kind(status);
        if kind == GitChangeKind::Renamed {
            let original = records.get(index + 1).copied().unwrap_or("");
            let path = records.get(index + 2).copied().unwrap_or("");
            index += 3;
            if original.is_empty() || path.is_empty() {
                continue;
            }
            entries.push(NameStatusEntry {
                kind,
                original_path: Some(normalize_git_path(original)),
                path: normalize_git_path(path),
            });
            continue;
        }
        let path = records.get(index + 1).copied().unwrap_or("");
        index += 2;
        if path.is_empty() {
            continue;
        }
        entries.push(NameStatusEntry {
            kind,
            original_path: None,
            path: normalize_git_path(path),
        });
    }
    entries
}

fn map_name_status_kind(status: &str) -> GitChangeKind {
    match status.chars().next().unwrap_or('M') {
        'A' => GitChangeKind::Added,
        'D' => GitChangeKind::Deleted,
        'R' | 'C' => GitChangeKind::Renamed,
        _ => GitChangeKind::Modified,
    }
}

fn parse_numstat(stdout: &str) -> BTreeMap<String, (u64, u64)> {
    let records: Vec<&str> = stdout
        .split('\0')
        .filter(|record| !record.is_empty())
        .collect();
    let mut stats = BTreeMap::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        let fields: Vec<&str> = record.split('\t').collect();
        if fields.len() < 3 {
            index += 1;
            continue;
        }
        let added = parse_count(fields[0]);
        let removed = parse_count(fields[1]);
        let path_field = fields[2..].join("\t");
        if !path_field.is_empty() {
            stats.insert(normalize_git_path(&path_field), (added, removed));
            index += 1;
            continue;
        }
        let original = records.get(index + 1).copied().unwrap_or("");
        let renamed = records.get(index + 2).copied().unwrap_or("");
        index += 3;
        if renamed.is_empty() {
            continue;
        }
        stats.insert(normalize_git_path(renamed), (added, removed));
        if !original.is_empty() {
            stats.insert(normalize_git_path(original), (added, removed));
        }
    }
    stats
}

fn parse_count(value: &str) -> u64 {
    if value == "-" {
        0
    } else {
        value.trim().parse::<u64>().unwrap_or(0)
    }
}

struct TreeEntry {
    mode: String,
    object_id: String,
}

fn parse_ls_tree(stdout: &str) -> BTreeMap<String, TreeEntry> {
    let mut entries = BTreeMap::new();
    for record in stdout.split('\0').filter(|record| !record.is_empty()) {
        let Some(tab_index) = record.find('\t') else {
            continue;
        };
        let header: Vec<&str> = record[..tab_index].split(' ').collect();
        if header.len() < 3 {
            continue;
        }
        let path = normalize_git_path(&record[tab_index + 1..]);
        entries.insert(
            path,
            TreeEntry {
                mode: header[0].to_string(),
                object_id: header[2].to_string(),
            },
        );
    }
    entries
}

fn merge_diff(
    resolution: &GitResolvedRepository,
    from_checkpoint_id: &str,
    to_checkpoint_id: &str,
    name_status: Vec<NameStatusEntry>,
    numstat: &BTreeMap<String, (u64, u64)>,
) -> GitCheckpointDiff {
    let files = name_status
        .into_iter()
        .map(|entry| {
            let (added, removed) = numstat
                .get(&entry.path)
                .or_else(|| {
                    entry
                        .original_path
                        .as_ref()
                        .and_then(|original| numstat.get(original))
                })
                .copied()
                .unwrap_or((0, 0));
            GitCheckpointFileDiff {
                path: to_absolute_path(&resolution.repo_root, &entry.path)
                    .to_string_lossy()
                    .into_owned(),
                repo_relative_path: entry.path.clone(),
                workspace_relative_path: to_workspace_relative_git_path(
                    &entry.path,
                    &resolution.workspace_in_repo_path,
                ),
                original_path: entry.original_path.map(|original| {
                    to_absolute_path(&resolution.repo_root, &original)
                        .to_string_lossy()
                        .into_owned()
                }),
                kind: entry.kind,
                added,
                removed,
            }
        })
        .collect();
    GitCheckpointDiff {
        from_checkpoint_id: from_checkpoint_id.to_string(),
        to_checkpoint_id: to_checkpoint_id.to_string(),
        files,
    }
}

fn build_affected_repo_paths(files: &[GitCheckpointFileDiff]) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    for file in files {
        if !values.contains(&file.repo_relative_path) {
            values.push(file.repo_relative_path.clone());
        }
        if let Some(original) = &file.original_path {
            if !values.contains(original) {
                values.push(original.clone());
            }
        }
    }
    values
}

fn normalize_affected_repo_path(repo_root: &Path, path: &str) -> String {
    let unified_path = path.replace('\\', "/");
    if !Path::new(path).is_absolute() {
        return normalize_git_path(path);
    }
    let root = repo_root.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/');
    if cfg!(windows) {
        let lowered_path = unified_path.to_lowercase();
        let lowered_root = root.to_lowercase();
        if let Some(rest) = lowered_path
            .strip_prefix(&format!("{lowered_root}/"))
            .map(|offset| unified_path[unified_path.len() - offset.len()..].to_string())
        {
            return normalize_git_path(&rest);
        }
    } else if let Some(rest) = unified_path.strip_prefix(&format!("{root}/")) {
        return normalize_git_path(rest);
    }
    normalize_git_path(path)
}
