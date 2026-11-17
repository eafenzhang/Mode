//! `Write` / `Edit` 结果的 `display: {"kind":"file_diff", …}` 投影。
//!
//! 观察出处（行为夹具，逐条可复现）：
//!
//! - `write-overwrite-read`：单行替换 → `oldStart/oldLines/newStart/newLines = 1/1/1/1`，
//!   `lines = ["-original", "+replaced"]`；
//! - `edit-unique`：三行文件改一行 → 一个 hunk 覆盖全文件（含上下文行）；
//! - `edit-replace-all`：两处替换合并成一个 hunk；
//! - `edit-empty-old-creates`：空文件 → 新建 → `oldLines = 0`、`lines = ["+created"]`；
//! - `write-new` / `write-relative-path`：**新建 `Write` 没有 `display`**（`Edit` 新建有）。
//!
//! 形状只按这些观察实现：`kind` / `filePath` / `additions` / `deletions` /
//! `structuredPatch[{oldStart,oldLines,newStart,newLines,lines}]` / `truncated:false`。
//! 未观察到的分支（超大 diff 的截断、二进制文件）**不猜**：调用方拿不到文本就不产出 display。

use serde_json::{json, Value};

/// 统一 diff 的上下文行数。观察样本里所有 hunk 都覆盖了整个小文件，无法直接读出这个常数；
/// 取 3 是 unified diff 的通行值，且样本结果与之相容（任何 ≥ 样本间距的取值都同形）。
const CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Op<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

/// 结果的 `display.filePath`：**工作目录原生写法 + `/` + 相对路径**（观察形态）。
///
/// 观察出处：夹具 `write-new` / `write-overwrite-read` / `edit-unique` / `edit-replace-all` /
/// `edit-empty-old-creates` 的 `display.filePath` 规范化后都是 `<cwd>/<相对路径>`；
/// 录制时的工作目录前缀是**原生反斜杠**写法（§3 的路径规则只认原生与 JSON 转义两种形态），
/// 所以这里保留 `cwd` 原样、只用 `/` 拼接相对路径。目标不在 `cwd` 下时按原生绝对路径
/// 原样返回（未观察到的场景，不做猜测性改写）。
pub fn display_path(target: &std::path::Path, cwd: &str) -> String {
    let target_posix = target.to_string_lossy().replace('\\', "/");
    let cwd_posix = cwd.replace('\\', "/");
    let cwd_trimmed = cwd_posix.trim_end_matches('/');
    match target_posix.strip_prefix(cwd_trimmed) {
        Some(rest) if rest.starts_with('/') && rest.len() > 1 => {
            format!("{}{}", cwd.trim_end_matches(['/', '\\']), rest)
        }
        _ => target.to_string_lossy().to_string(),
    }
}

/// `before` → `after` 的行级 diff；无变化时返回 `None`。
///
/// 行切分：按 `\n` 切，末尾换行不产生空行（`"a\n"` 与 `"a"` 都是一行）。这与夹具里
/// `lines` 不含空串的观察一致。
pub fn file_diff_display(file_path: &str, before: &str, after: &str) -> Option<String> {
    if before == after {
        return None;
    }
    // `filePath` 按调用方给的**观测形态**原样落盘：oracle 录到的是「工作目录的原生写法 +
    // 正斜杠 + 相对路径」（夹具里规范化后是 `<cwd>/target.txt`，§3 的路径替换发生在原生
    // 前缀上）。这里不再做反斜杠→正斜杠的全局改写——那会把工作目录也折成正斜杠，
    // 路径规则就换不掉它（C1 Task 11 回放发现）。用 [`display_path`] 构造调用入参。
    let file_path = file_path.to_string();
    let old = split_lines(before);
    let new = split_lines(after);
    let ops = diff_ops(&old, &new);
    let hunks = hunk_ranges(&ops);

    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut structured = Vec::with_capacity(hunks.len());
    for (start, end) in hunks {
        // hunk 起点在旧/新文件里的 1-based 行号：数一遍 hunk 之前的 Equal/Delete（或 Equal/Insert）。
        let old_start = 1 + ops[..start]
            .iter()
            .filter(|op| matches!(op, Op::Equal(_) | Op::Delete(_)))
            .count();
        let new_start = 1 + ops[..start]
            .iter()
            .filter(|op| matches!(op, Op::Equal(_) | Op::Insert(_)))
            .count();

        let mut old_lines = 0usize;
        let mut new_lines = 0usize;
        let mut lines = Vec::new();
        for op in &ops[start..end] {
            match op {
                Op::Equal(text) => {
                    old_lines += 1;
                    new_lines += 1;
                    lines.push(format!(" {text}"));
                }
                Op::Delete(text) => {
                    old_lines += 1;
                    deletions += 1;
                    lines.push(format!("-{text}"));
                }
                Op::Insert(text) => {
                    new_lines += 1;
                    additions += 1;
                    lines.push(format!("+{text}"));
                }
            }
        }
        structured.push(json!({
            "oldStart": old_start,
            "oldLines": old_lines,
            "newStart": new_start,
            "newLines": new_lines,
            "lines": lines,
        }));
    }

    Some(
        json!({
            "kind": "file_diff",
            "filePath": file_path,
            "additions": additions,
            "deletions": deletions,
            "structuredPatch": structured,
            "truncated": false,
        })
        .to_string(),
    )
}

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// LCS 行 diff。平局时**先删后增**（观察：替换行是 `-old` 紧接 `+new`）。
fn diff_ops<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Op<'a>> {
    let n = old.len();
    let m = new.len();
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old[i] == new[j] {
            ops.push(Op::Equal(old[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            ops.push(Op::Delete(old[i]));
            i += 1;
        } else {
            ops.push(Op::Insert(new[j]));
            j += 1;
        }
    }
    while i < n {
        ops.push(Op::Delete(old[i]));
        i += 1;
    }
    while j < m {
        ops.push(Op::Insert(new[j]));
        j += 1;
    }
    ops
}

/// 把变更点括进 `CONTEXT_LINES` 行上下文；相邻 hunk 的间隔 ≤ 2×context 时合并。
fn hunk_ranges(ops: &[Op<'_>]) -> Vec<(usize, usize)> {
    let mut hunks: Vec<(usize, usize, usize)> = Vec::new(); // (start, end, last_change)
    for (index, op) in ops.iter().enumerate() {
        if matches!(op, Op::Equal(_)) {
            continue;
        }
        if let Some((_, end, last_change)) = hunks.last_mut() {
            if index - *last_change - 1 <= 2 * CONTEXT_LINES {
                *end = (index + CONTEXT_LINES + 1).min(ops.len());
                *last_change = index;
                continue;
            }
        }
        hunks.push((
            index.saturating_sub(CONTEXT_LINES),
            (index + CONTEXT_LINES + 1).min(ops.len()),
            index,
        ));
    }
    hunks
        .into_iter()
        .map(|(start, end, _)| (start, end))
        .collect()
}

/// 供测试与调用方判断：这次显示是否有内容可呈。
pub fn has_changes(display: &Value) -> bool {
    display
        .get("additions")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + display
            .get("deletions")
            .and_then(Value::as_u64)
            .unwrap_or(0)
        > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(before: &str, after: &str) -> Value {
        let json = file_diff_display("/ws/file.txt", before, after).expect("有变化");
        serde_json::from_str(&json).expect("合法 JSON")
    }

    #[test]
    fn single_line_replacement_matches_the_recorded_shape() {
        let value = display("original\n", "replaced\n");
        assert_eq!(
            value,
            json!({
                "kind": "file_diff",
                "filePath": "/ws/file.txt",
                "additions": 1,
                "deletions": 1,
                "structuredPatch": [{
                    "oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 1,
                    "lines": ["-original", "+replaced"],
                }],
                "truncated": false,
            })
        );
    }

    #[test]
    fn one_changed_line_keeps_context_lines() {
        let value = display("alpha\nBeta\ngamma\n", "alpha\nBETA\ngamma\n");
        assert_eq!(
            value["structuredPatch"][0],
            json!({
                "oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 3,
                "lines": [" alpha", "-Beta", "+BETA", " gamma"],
            })
        );
    }

    #[test]
    fn replace_all_merges_into_one_hunk() {
        let value = display("x\nfoo\ny\nfoo\n", "x\nbar\ny\nbar\n");
        assert_eq!(
            value["structuredPatch"][0],
            json!({
                "oldStart": 1, "oldLines": 4, "newStart": 1, "newLines": 4,
                "lines": [" x", "-foo", "+bar", " y", "-foo", "+bar"],
            })
        );
    }

    #[test]
    fn creating_from_an_empty_file_has_zero_old_lines() {
        let value = display("", "created\n");
        assert_eq!(
            value,
            json!({
                "kind": "file_diff",
                "filePath": "/ws/file.txt",
                "additions": 1,
                "deletions": 0,
                "structuredPatch": [{
                    "oldStart": 1, "oldLines": 0, "newStart": 1, "newLines": 1,
                    "lines": ["+created"],
                }],
                "truncated": false,
            })
        );
    }

    #[test]
    fn no_change_has_no_display() {
        assert_eq!(file_diff_display("/ws/file.txt", "same\n", "same\n"), None);
    }

    #[test]
    fn distant_changes_split_into_two_hunks() {
        let before = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\n";
        let after = "A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nN\n";
        let value = display(before, after);
        assert_eq!(value["structuredPatch"].as_array().unwrap().len(), 2);
        assert_eq!(value["additions"], 2);
        assert_eq!(value["deletions"], 2);
    }

    #[test]
    fn has_changes_counts_additions_and_deletions() {
        assert!(has_changes(&display("a\n", "b\n")));
        assert!(!has_changes(&json!({
            "kind": "file_diff", "additions": 0, "deletions": 0,
        })));
    }
}
