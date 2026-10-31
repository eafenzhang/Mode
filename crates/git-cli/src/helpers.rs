//! Git 命令输出解析与差异/未跟踪统计（`gitCliHelpers.ts`）。

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::command::GitCommandExecutionResult;
use crate::config::{
    normalize_git_path, GIT_UNTRACKED_STAT_CHUNK_BYTES, GIT_UNTRACKED_STAT_MAX_BYTES,
};
use crate::types::{
    GitBranchMutationIssue, GitChangeKind, GitDiffAvailability, GitDiffResult, GitHeadRefType,
    GitLineStat, GitResolvedRepository, GitStatusEntry,
};

pub fn to_result_message(result: &GitCommandExecutionResult) -> String {
    let stderr = result.stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_string();
    }
    let stdout = result.stdout.trim();
    if !stdout.is_empty() {
        return stdout.to_string();
    }
    match result.exit_code {
        Some(code) => format!("exitCode={code}"),
        None => "exitCode=null".to_string(),
    }
}

fn to_normalized_lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .split('\n')
        .map(|line| line.trim_end().to_string())
        .collect()
}

fn extract_indented_paths(lines: &[String], header_needle: &str) -> Vec<String> {
    let header_index = lines
        .iter()
        .position(|line| line.to_lowercase().contains(header_needle));
    let Some(header_index) = header_index else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for line in &lines[header_index + 1..] {
        if line.is_empty() {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            break;
        }
        let value = line.trim();
        if !value.is_empty() {
            paths.push(normalize_git_path(value));
        }
    }
    paths
}

pub fn to_invalid_branch_name_issue(detail: Option<&str>) -> GitBranchMutationIssue {
    GitBranchMutationIssue {
        code: "invalid-branch-name".to_string(),
        message: "Branch name is invalid.".to_string(),
        paths: None,
        detail: detail
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
    }
}

pub fn parse_git_branch_mutation_issues(
    result: &GitCommandExecutionResult,
) -> Vec<GitBranchMutationIssue> {
    let stderr = result.stderr.trim();
    let stdout = result.stdout.trim();
    let detail = if !stderr.is_empty() {
        Some(stderr.to_string())
    } else if !stdout.is_empty() {
        Some(stdout.to_string())
    } else {
        None
    };
    let lines = to_normalized_lines(detail.as_deref().unwrap_or(""));
    let normalized = detail.as_deref().unwrap_or("").to_lowercase();

    let tracked = extract_indented_paths(
        &lines,
        "your local changes to the following files would be overwritten by",
    );
    if !tracked.is_empty() && (normalized.contains("checkout") || normalized.contains("switch")) {
        return vec![GitBranchMutationIssue {
            code: "tracked-changes-would-be-overwritten".to_string(),
            message: "Tracked changes would be overwritten by switching branches.".to_string(),
            paths: Some(tracked),
            detail,
        }];
    }

    let untracked = extract_indented_paths(
        &lines,
        "the following untracked working tree files would be overwritten by",
    );
    if !untracked.is_empty() && (normalized.contains("checkout") || normalized.contains("switch")) {
        return vec![GitBranchMutationIssue {
            code: "untracked-changes-would-be-overwritten".to_string(),
            message: "Untracked files would be overwritten by switching branches.".to_string(),
            paths: Some(untracked),
            detail,
        }];
    }

    let issue = |code: &str, message: &str| GitBranchMutationIssue {
        code: code.to_string(),
        message: message.to_string(),
        paths: None,
        detail: detail.clone(),
    };

    if normalized.contains("already exists") {
        return vec![issue("branch-already-exists", "Branch already exists.")];
    }
    if normalized.contains("invalid reference:") {
        return vec![issue(
            "target-branch-not-found",
            "Target branch was not found.",
        )];
    }
    if normalized.contains("is already used by worktree at") {
        return vec![issue(
            "branch-in-other-worktree",
            "Branch is already checked out in another worktree.",
        )];
    }
    if normalized.contains("resolve your current index first") {
        return vec![issue(
            "conflicts-present",
            "Repository still has unresolved conflicts.",
        )];
    }
    let operation_in_progress = [
        "cannot switch branch while merging",
        "cannot switch branch while rebasing",
        "cannot switch branch while cherry-picking",
        "cannot switch branch while reverting",
        "cannot switch branch while bisecting",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
        || normalized.contains("you have not concluded your merge")
        || normalized.contains("rebase in progress");
    if operation_in_progress {
        return vec![issue(
            "operation-in-progress",
            "Another Git operation is still in progress.",
        )];
    }
    vec![issue(
        "unknown",
        "Git could not complete the branch operation.",
    )]
}

pub fn ensure_git_command_succeeded(
    label: &str,
    result: &GitCommandExecutionResult,
    allowed_exit_codes: &[i32],
) -> Result<(), String> {
    if result.timed_out {
        let timeout_ms = result.timeout_ms.unwrap_or(result.duration_ms);
        let mut details = vec![format!("elapsed={}ms", result.duration_ms)];
        if let Some(elapsed) = result.timeout_elapsed_ms {
            details.push(format!("killAt={elapsed}ms"));
        }
        if let Some(cleanup) = result.timeout_close_delay_ms {
            details.push(format!("cleanup={cleanup}ms"));
        }
        if result.force_kill_attempted {
            details.push("forceKill=true".to_string());
        }
        if result.orphaned {
            details.push("orphaned=true".to_string());
        }
        return Err(format!(
            "{label} timed out after {timeout_ms}ms ({})",
            details.join(", ")
        ));
    }
    if result.output_truncated {
        return Err(format!("{label} output exceeded limit"));
    }
    if result
        .exit_code
        .map(|code| allowed_exit_codes.contains(&code))
        .unwrap_or(false)
    {
        return Ok(());
    }
    Err(format!("{label} failed: {}", to_result_message(result)))
}

pub fn is_not_repository_result(result: &GitCommandExecutionResult) -> bool {
    let stderr = result.stderr.to_lowercase();
    stderr.contains("not a git repository") || stderr.contains("outside repository")
}

pub fn is_missing_working_directory_result(result: &GitCommandExecutionResult) -> bool {
    let stderr = result.stderr.to_lowercase();
    (result.exit_code == Some(-2) && stderr.contains("enoent"))
        || stderr.contains("unable to read current working directory")
        || stderr.contains("no such file or directory")
}

fn infer_kind_from_status_code(status_code: &str) -> GitChangeKind {
    match status_code {
        "A" | "?" => GitChangeKind::Added,
        "D" => GitChangeKind::Deleted,
        "R" | "C" => GitChangeKind::Renamed,
        _ => GitChangeKind::Modified,
    }
}

fn parse_branch_ahead_behind(value: &str) -> (u64, u64) {
    let parse_number = |sign: char| -> u64 {
        value
            .split(sign)
            .nth(1)
            .map(|rest| {
                rest.chars()
                    .take_while(|character| character.is_ascii_digit())
                    .collect::<String>()
            })
            .and_then(|digits| digits.parse::<u64>().ok())
            .unwrap_or(0)
    };
    (parse_number('+'), parse_number('-'))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedStatus {
    pub branch_name: Option<String>,
    pub tracking_branch_name: Option<String>,
    pub head_ref_type: GitHeadRefType,
    pub ahead: u64,
    pub behind: u64,
    pub entries: Vec<GitStatusEntry>,
}

pub fn parse_status_porcelain(stdout: &str) -> ParsedStatus {
    let records: Vec<&str> = stdout
        .split('\0')
        .filter(|record| !record.is_empty())
        .collect();
    let mut entries = Vec::new();
    let mut branch_name = None;
    let mut tracking_branch_name = None;
    let mut head_ref_type = GitHeadRefType::Branch;
    let mut ahead = 0;
    let mut behind = 0;

    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if let Some(header) = record.strip_prefix("# ") {
            if let Some(head) = header.strip_prefix("branch.head ") {
                if head == "(detached)" {
                    branch_name = None;
                    head_ref_type = GitHeadRefType::Detached;
                } else {
                    branch_name = Some(head.to_string());
                    head_ref_type = GitHeadRefType::Branch;
                }
            } else if let Some(upstream) = header.strip_prefix("branch.upstream ") {
                tracking_branch_name = Some(upstream.to_string());
            } else if let Some(ab) = header.strip_prefix("branch.ab ") {
                let (next_ahead, next_behind) = parse_branch_ahead_behind(ab);
                ahead = next_ahead;
                behind = next_behind;
            }
            index += 1;
            continue;
        }

        if let Some(path) = record.strip_prefix("? ") {
            entries.push(GitStatusEntry {
                path: normalize_git_path(path),
                original_path: None,
                kind: GitChangeKind::Added,
                x: None,
                y: Some("?".to_string()),
                is_untracked: true,
                is_conflicted: false,
            });
            index += 1;
            continue;
        }

        if let Some(rest) = record.strip_prefix("1 ") {
            if let Some((xy, path)) = split_xy_and_path(rest, 6) {
                let status = if &xy[0..1] != "." {
                    &xy[0..1]
                } else {
                    &xy[1..2]
                };
                entries.push(GitStatusEntry {
                    path: normalize_git_path(&path),
                    original_path: None,
                    kind: infer_kind_from_status_code(status),
                    x: Some(xy[0..1].to_string()),
                    y: Some(xy[1..2].to_string()),
                    is_untracked: false,
                    is_conflicted: false,
                });
            }
            index += 1;
            continue;
        }

        if let Some(rest) = record.strip_prefix("2 ") {
            if let Some((xy, path)) = split_xy_and_path(rest, 7) {
                let original_path = records.get(index + 1).copied();
                index += 1;
                entries.push(GitStatusEntry {
                    path: normalize_git_path(&path),
                    original_path: original_path.map(normalize_git_path),
                    kind: GitChangeKind::Renamed,
                    x: Some(xy[0..1].to_string()),
                    y: Some(xy[1..2].to_string()),
                    is_untracked: false,
                    is_conflicted: false,
                });
            }
            index += 1;
            continue;
        }

        if let Some(rest) = record.strip_prefix("u ") {
            if let Some((xy, path)) = split_xy_and_path(rest, 8) {
                entries.push(GitStatusEntry {
                    path: normalize_git_path(&path),
                    original_path: None,
                    kind: GitChangeKind::Modified,
                    x: Some(xy[0..1].to_string()),
                    y: Some(xy[1..2].to_string()),
                    is_untracked: false,
                    is_conflicted: true,
                });
            }
        }
        index += 1;
    }

    ParsedStatus {
        branch_name,
        tracking_branch_name,
        head_ref_type,
        ahead,
        behind,
        entries,
    }
}

/// `2` 个字符的 XY + 恰好 `field_count` 个非空字段 + 路径（可含空格）。
fn split_xy_and_path(rest: &str, field_count: usize) -> Option<(String, String)> {
    let bytes = rest.as_bytes();
    if bytes.len() < 3 || bytes[2] != b' ' {
        return None;
    }
    let xy = rest.get(0..2)?.to_string();
    if xy.contains(' ') {
        return None;
    }
    let after = rest.get(3..)?;
    let parts: Vec<&str> = after.splitn(field_count + 1, ' ').collect();
    if parts.len() != field_count + 1 {
        return None;
    }
    if parts[..field_count].iter().any(|part| part.is_empty()) {
        return None;
    }
    Some((xy, parts[field_count].to_string()))
}

fn parse_numstat_value(value: &str) -> u64 {
    if value == "-" {
        return 0;
    }
    value.trim().parse::<u64>().unwrap_or(0)
}

pub fn infer_kind_from_numstat(stat: &GitLineStat) -> GitChangeKind {
    if let Some(kind) = stat.kind {
        return kind;
    }
    if stat.added > 0 && stat.removed == 0 {
        return GitChangeKind::Added;
    }
    if stat.removed > 0 && stat.added == 0 {
        return GitChangeKind::Deleted;
    }
    GitChangeKind::Modified
}

pub fn parse_numstat(stdout: &str) -> BTreeMap<String, GitLineStat> {
    let records: Vec<&str> = stdout.split('\0').collect();
    let mut stats = BTreeMap::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.is_empty() {
            index += 1;
            continue;
        }
        let fields: Vec<&str> = record.split('\t').collect();
        if fields.len() < 3 {
            index += 1;
            continue;
        }
        let added = parse_numstat_value(fields[0]);
        let removed = parse_numstat_value(fields[1]);
        let path_field = fields[2..].join("\t");
        if !path_field.is_empty() {
            stats.insert(
                normalize_git_path(&path_field),
                GitLineStat {
                    added,
                    removed,
                    kind: None,
                    original_path: None,
                },
            );
            index += 1;
            continue;
        }
        let original_path = records.get(index + 1).copied().unwrap_or("");
        let renamed_path = records.get(index + 2).copied().unwrap_or("");
        index += 3;
        if renamed_path.is_empty() {
            continue;
        }
        stats.insert(
            normalize_git_path(renamed_path),
            GitLineStat {
                added,
                removed,
                kind: Some(GitChangeKind::Renamed),
                original_path: Some(normalize_git_path(original_path)),
            },
        );
    }
    stats
}

fn count_untracked_file_lines(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::metadata(path) else {
        return 0;
    };
    if !metadata.is_file() || metadata.len() > GIT_UNTRACKED_STAT_MAX_BYTES {
        return 0;
    }
    let Ok(mut file) = File::open(path) else {
        return 0;
    };
    let mut buffer = vec![0u8; GIT_UNTRACKED_STAT_CHUNK_BYTES];
    let mut total_bytes: u64 = 0;
    let mut newlines: u64 = 0;
    let mut last_byte: u8 = 10;
    loop {
        let remaining = GIT_UNTRACKED_STAT_MAX_BYTES + 1 - total_bytes;
        let length = (buffer.len() as u64).min(remaining) as usize;
        let Ok(read) = file.read(&mut buffer[..length]) else {
            return 0;
        };
        if read == 0 {
            return newlines + u64::from(last_byte != 10);
        }
        total_bytes += read as u64;
        if total_bytes > GIT_UNTRACKED_STAT_MAX_BYTES {
            return 0;
        }
        for byte in &buffer[..read] {
            if *byte == 0 {
                return 0;
            }
            if *byte == 10 {
                newlines += 1;
            }
        }
        last_byte = buffer[read - 1];
    }
}

pub fn build_untracked_stats(
    repo_root: &Path,
    entries: &[GitStatusEntry],
) -> BTreeMap<String, GitLineStat> {
    let mut stats = BTreeMap::new();
    for entry in entries.iter().filter(|entry| entry.is_untracked) {
        let absolute = entry
            .path
            .split('/')
            .fold(repo_root.to_path_buf(), |path, segment| path.join(segment));
        stats.insert(
            entry.path.clone(),
            GitLineStat {
                added: count_untracked_file_lines(&absolute),
                removed: 0,
                kind: None,
                original_path: None,
            },
        );
    }
    stats
}

pub fn file_exists(path: &Path) -> bool {
    path.exists()
}

fn split_untracked_text(content: &str) -> (Vec<String>, bool) {
    let normalized = content.replace("\r\n", "\n");
    if normalized.is_empty() {
        return (Vec::new(), false);
    }
    let has_trailing_newline = normalized.ends_with('\n');
    let mut lines: Vec<String> = normalized.split('\n').map(str::to_string).collect();
    if has_trailing_newline {
        lines.pop();
    }
    (lines, has_trailing_newline)
}

pub fn build_untracked_text_diff_result(
    absolute_path: &Path,
    repo_relative_path: &str,
    max_preview_bytes: usize,
) -> Option<GitDiffResult> {
    let content = std::fs::read(absolute_path).ok()?;
    let path_text = absolute_path.to_string_lossy().into_owned();
    if content.contains(&0) {
        return Some(GitDiffResult {
            path: path_text,
            availability: GitDiffAvailability::Binary,
            patch: None,
            before_content: None,
            after_content: None,
            summary: Some("Binary diff is not previewable.".to_string()),
        });
    }
    if content.len() > max_preview_bytes {
        return Some(GitDiffResult {
            path: path_text,
            availability: GitDiffAvailability::Truncated,
            patch: None,
            before_content: None,
            after_content: None,
            summary: Some("Git diff output exceeded the preview limit.".to_string()),
        });
    }
    let text = String::from_utf8_lossy(&content).into_owned();
    let normalized_path = normalize_git_path(repo_relative_path);
    let (lines, has_trailing_newline) = split_untracked_text(&text);
    let mut patch_lines = vec![
        "--- /dev/null".to_string(),
        format!("+++ b/{normalized_path}"),
    ];
    if !lines.is_empty() {
        patch_lines.push(format!("@@ -0,0 +1,{} @@", lines.len()));
        patch_lines.extend(lines.iter().map(|line| format!("+{line}")));
        if !has_trailing_newline {
            patch_lines.push("\\ No newline at end of file".to_string());
        }
    }
    Some(GitDiffResult {
        path: path_text,
        availability: GitDiffAvailability::Patch,
        patch: Some(format!("{}\n", patch_lines.join("\n"))),
        before_content: Some(String::new()),
        after_content: Some(text),
        summary: None,
    })
}

fn is_binary_diff(stdout: &str) -> bool {
    stdout.contains("GIT binary patch") || stdout.contains("Binary files ")
}

pub fn to_diff_result(
    path: &str,
    result: &GitCommandExecutionResult,
    allowed_exit_codes: Option<&[i32]>,
    empty_summary: Option<&str>,
    binary_summary: Option<&str>,
) -> GitDiffResult {
    let unavailable = |summary: String| GitDiffResult {
        path: path.to_string(),
        availability: GitDiffAvailability::Unavailable,
        patch: None,
        before_content: None,
        after_content: None,
        summary: Some(summary),
    };
    if result.timed_out {
        return unavailable("Git diff command timed out.".to_string());
    }
    if result.output_truncated {
        return GitDiffResult {
            path: path.to_string(),
            availability: GitDiffAvailability::Truncated,
            patch: None,
            before_content: None,
            after_content: None,
            summary: Some("Git diff output exceeded the preview limit.".to_string()),
        };
    }
    let allowed = allowed_exit_codes.unwrap_or(&[0]);
    if !result
        .exit_code
        .map(|code| allowed.contains(&code))
        .unwrap_or(false)
    {
        return unavailable(to_result_message(result));
    }
    if result.stdout.trim().is_empty() {
        return unavailable(
            empty_summary
                .unwrap_or("No diff output available.")
                .to_string(),
        );
    }
    if is_binary_diff(&result.stdout) {
        return GitDiffResult {
            path: path.to_string(),
            availability: GitDiffAvailability::Binary,
            patch: None,
            before_content: None,
            after_content: None,
            summary: Some(
                binary_summary
                    .unwrap_or("Binary diff is not previewable.")
                    .to_string(),
            ),
        };
    }
    GitDiffResult {
        path: path.to_string(),
        availability: GitDiffAvailability::Patch,
        patch: Some(result.stdout.clone()),
        before_content: None,
        after_content: None,
        summary: None,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitConfigValue {
    pub scope: Option<String>,
    pub source: Option<String>,
    pub value: Option<String>,
}

pub fn parse_git_config_value(
    result: &GitCommandExecutionResult,
) -> Result<GitConfigValue, String> {
    if result.exit_code == Some(1) {
        return Ok(GitConfigValue {
            scope: None,
            source: None,
            value: None,
        });
    }
    ensure_git_command_succeeded("git config", result, &[0])?;
    let line = result
        .stdout
        .strip_suffix("\r\n")
        .or_else(|| result.stdout.strip_suffix('\n'))
        .unwrap_or(&result.stdout)
        .to_string();
    let parts: Vec<&str> = line.split('\t').collect();
    if parts.len() < 3 {
        return Ok(GitConfigValue {
            scope: None,
            source: None,
            value: if line.is_empty() { None } else { Some(line) },
        });
    }
    let value = parts[2..].join("\t");
    Ok(GitConfigValue {
        scope: Some(parts[0].to_string()),
        source: Some(parts[1].to_string()),
        value: if value.is_empty() { None } else { Some(value) },
    })
}

pub fn normalize_input_path(
    resolution: &GitResolvedRepository,
    path: &str,
) -> Result<String, String> {
    let raw_absolute = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        path.split('/')
            .fold(resolution.workspace_path.clone(), |base, segment| {
                base.join(segment)
            })
    };
    let absolute = plugin_discovery::js::realpath(&raw_absolute).unwrap_or(raw_absolute);
    let repo_relative = relative_to(&resolution.repo_root, &absolute);
    if repo_relative.is_empty()
        || repo_relative == "."
        || repo_relative == ".."
        || repo_relative.starts_with("../")
    {
        return Err(format!("Path is outside repository scope: {path}"));
    }
    Ok(normalize_git_path(&repo_relative))
}

/// `path.relative(root, candidate)` 的近似（Windows 上按小写比较组件）。
fn relative_to(root: &Path, candidate: &Path) -> String {
    let root_components: Vec<Component> = root.components().collect();
    let candidate_components: Vec<Component> = candidate.components().collect();
    let comparable = |component: &Component| -> String {
        let text = component.as_os_str().to_string_lossy().into_owned();
        if cfg!(windows) {
            text.to_lowercase()
        } else {
            text
        }
    };
    let mut common = 0;
    while common < root_components.len()
        && common < candidate_components.len()
        && comparable(&root_components[common]) == comparable(&candidate_components[common])
    {
        common += 1;
    }
    if common < root_components.len() {
        let ups = root_components.len() - common;
        let mut parts = vec!["..".to_string(); ups];
        parts.extend(
            candidate_components[common..]
                .iter()
                .map(|component| component.as_os_str().to_string_lossy().into_owned()),
        );
        return parts.join("/");
    }
    candidate_components[common..]
        .iter()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn ensure_repository_available(
    resolution: &GitResolvedRepository,
    label: &str,
) -> Result<(), String> {
    if !resolution.is_git_available {
        return Err(format!("Cannot {label}: Git binary is not available"));
    }
    if !resolution.is_repository {
        return Err(format!(
            "Cannot {label}: workspace is not inside a Git repository"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_of(stdout: &str, stderr: &str, exit: Option<i32>) -> GitCommandExecutionResult {
        GitCommandExecutionResult {
            binary_path: "git".to_string(),
            cwd: ".".to_string(),
            args: Vec::new(),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            exit_code: exit,
            signal: None,
            duration_ms: 1,
            timed_out: false,
            timeout_ms: Some(15_000),
            timeout_elapsed_ms: None,
            timeout_close_delay_ms: None,
            force_kill_attempted: false,
            orphaned: false,
            output_truncated: false,
        }
    }

    #[test]
    fn status_porcelain_parses_headers_and_entries() {
        let stdout =
            "# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0? new.md\0\
1 .M N... 100644 100644 100644 a b mod.md\0\
2 R. N... 100644 100644 100644 a b R100 renamed.md\0original.md\0\
u UU N... 100644 100644 100644 100644 a b c conflict.md\0";
        let parsed = parse_status_porcelain(stdout);
        assert_eq!(parsed.branch_name.as_deref(), Some("main"));
        assert_eq!(parsed.tracking_branch_name.as_deref(), Some("origin/main"));
        assert_eq!(parsed.ahead, 2);
        assert_eq!(parsed.behind, 1);
        assert_eq!(parsed.entries.len(), 4);
        assert!(parsed.entries[0].is_untracked);
        assert_eq!(parsed.entries[1].kind, GitChangeKind::Modified);
        assert_eq!(parsed.entries[2].kind, GitChangeKind::Renamed);
        assert_eq!(
            parsed.entries[2].original_path.as_deref(),
            Some("original.md")
        );
        assert!(parsed.entries[3].is_conflicted);
    }

    #[test]
    fn numstat_handles_renames_and_dashes() {
        let stats =
            parse_numstat("3\t0\tadded.md\u{0}-\t-\tbin.dat\u{0}3\t0\t\u{0}old.md\u{0}new.md\u{0}");
        assert_eq!(stats["added.md"].added, 3);
        assert_eq!(stats["bin.dat"].removed, 0);
        assert_eq!(stats["new.md"].kind, Some(GitChangeKind::Renamed));
        assert_eq!(stats["new.md"].original_path.as_deref(), Some("old.md"));
    }

    #[test]
    fn diff_result_classifies_states() {
        let truncated = to_diff_result(
            "a.md",
            &GitCommandExecutionResult {
                output_truncated: true,
                ..result_of("", "", Some(0))
            },
            None,
            None,
            None,
        );
        assert_eq!(truncated.availability, GitDiffAvailability::Truncated);
        let binary = to_diff_result(
            "a.bin",
            &result_of("Binary files a and b differ\n", "", Some(0)),
            None,
            None,
            None,
        );
        assert_eq!(binary.availability, GitDiffAvailability::Binary);
        let patch = to_diff_result(
            "a.md",
            &result_of("diff --git a b\n", "", Some(0)),
            None,
            None,
            None,
        );
        assert_eq!(patch.availability, GitDiffAvailability::Patch);
        let empty = to_diff_result("a.md", &result_of("  ", "", Some(0)), None, None, None);
        assert_eq!(empty.summary.as_deref(), Some("No diff output available."));
    }

    #[test]
    fn config_value_parses_and_handles_missing() {
        let value =
            parse_git_config_value(&result_of("global\tfile:x\tA\tB\n", "", Some(0))).unwrap();
        assert_eq!(value.scope.as_deref(), Some("global"));
        assert_eq!(value.source.as_deref(), Some("file:x"));
        assert_eq!(value.value.as_deref(), Some("A\tB"));
        let missing = parse_git_config_value(&result_of("", "", Some(1))).unwrap();
        assert!(missing.scope.is_none());
        assert!(missing.value.is_none());
    }

    #[test]
    fn branch_mutation_issues_are_normalized() {
        let tracked = parse_git_branch_mutation_issues(&result_of(
            "",
            "error: Your local changes to the following files would be overwritten by checkout:\n\tpkg/a.md\nPlease commit your changes.",
            Some(1),
        ));
        assert_eq!(tracked[0].code, "tracked-changes-would-be-overwritten");
        assert_eq!(
            tracked[0].paths.as_deref(),
            Some(&["pkg/a.md".to_string()][..])
        );

        let exists = parse_git_branch_mutation_issues(&result_of(
            "",
            "fatal: a branch named 'x' already exists",
            Some(128),
        ));
        assert_eq!(exists[0].code, "branch-already-exists");
        let progress = parse_git_branch_mutation_issues(&result_of(
            "",
            "fatal: cannot switch branch while rebasing",
            Some(128),
        ));
        assert_eq!(progress[0].code, "operation-in-progress");
        let unknown = parse_git_branch_mutation_issues(&result_of("", "fatal: others", Some(128)));
        assert_eq!(unknown[0].code, "unknown");
    }
}
