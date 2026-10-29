//! 工作区文件搜索：候选映射、模糊打分与 top-K（`workspaceFileSearch.ts`）。
//!
//! 打分里的长度与下标都按 **UTF-16 码元**计算（JS `String.length`/`indexOf` 口径），
//! 否则含增补平面字符的路径排序会与 TS 不同。

use crate::types::{EntryType, WorkspaceFileEntry};

pub const WORKSPACE_FILE_SEARCH_DISPLAY_CAP: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFileSearchCandidate {
    pub id: String,
    pub name: String,
    pub path: String,
    pub relative_path: String,
    pub r#type: EntryType,
    lowercase_name: Vec<u16>,
    lowercase_relative_path: Vec<u16>,
    lowercase_path: Vec<u16>,
}

fn utf16_lower(text: &str) -> Vec<u16> {
    text.trim().to_lowercase().encode_utf16().collect()
}

pub fn map_entries_to_candidates(entries: &[WorkspaceFileEntry]) -> Vec<WorkspaceFileSearchCandidate> {
    entries
        .iter()
        .map(|entry| WorkspaceFileSearchCandidate {
            id: entry.relative_path.clone(),
            name: entry.name.clone(),
            path: entry.path.clone(),
            relative_path: entry.relative_path.clone(),
            r#type: entry.r#type,
            lowercase_name: utf16_lower(&entry.name),
            lowercase_relative_path: utf16_lower(&entry.relative_path),
            lowercase_path: utf16_lower(&entry.path),
        })
        .collect()
}

fn score_utf16(text: &[u16], query: &[u16]) -> Option<i64> {
    if text.is_empty() {
        return None;
    }
    if query.is_empty() {
        return Some(0);
    }
    if text.starts_with(query) {
        return Some((text.len() - query.len()) as i64);
    }
    if let Some(index) = find_subslice(text, query) {
        return Some(100 + index as i64);
    }

    let mut score = 200i64;
    let mut search_start = 0usize;
    for unit in query {
        let found = text[search_start..].iter().position(|item| item == unit)? + search_start;
        score += (found - search_start) as i64;
        search_start = found + 1;
    }
    Some(score + (text.len() as i64 - query.len() as i64))
}

fn find_subslice(haystack: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub fn score_workspace_file_fuzzy_match(text: &str, query: &str) -> Option<i64> {
    score_utf16(&utf16_lower(text), &utf16_lower(query))
}

fn candidate_score(candidate: &WorkspaceFileSearchCandidate, query: &[u16]) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let name = score_utf16(&candidate.lowercase_name, query);
    let relative = score_utf16(&candidate.lowercase_relative_path, query);
    let path = score_utf16(&candidate.lowercase_path, query);

    let keyword = match (relative, path) {
        (Some(left), Some(right)) => Some((left + 300).min(right + 300)),
        (Some(value), None) | (None, Some(value)) => Some(value + 300),
        (None, None) => None,
    };
    [
        name,
        relative.map(|value| value + 25),
        keyword,
    ]
    .into_iter()
    .flatten()
    .min()
}

fn default_priority(candidate: &WorkspaceFileSearchCandidate) -> u8 {
    if candidate.r#type == EntryType::Directory {
        1
    } else {
        0
    }
}

/// `(score, index)` 全序；index 在一次遍历内唯一，因此不需要 name 兜底。
fn compare_scored(left: &(i64, usize), right: &(i64, usize)) -> std::cmp::Ordering {
    left.cmp(right)
}

pub fn filter_workspace_file_search_candidates(
    candidates: &[WorkspaceFileSearchCandidate],
    query: &str,
    limit: Option<usize>,
    require_query: bool,
) -> Vec<WorkspaceFileSearchCandidate> {
    let effective_limit = limit.unwrap_or(WORKSPACE_FILE_SEARCH_DISPLAY_CAP);
    let normalized_query: Vec<u16> = query.trim().to_lowercase().encode_utf16().collect();

    if normalized_query.is_empty() {
        if require_query {
            return Vec::new();
        }
        let mut sorted: Vec<(u8, usize, &WorkspaceFileSearchCandidate)> = candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| (default_priority(candidate), index, candidate))
            .collect();
        sorted.sort_by_key(|(priority, index, _)| (*priority, *index));
        return sorted
            .into_iter()
            .take(effective_limit)
            .map(|(_, _, candidate)| candidate.clone())
            .collect();
    }

    let mut best: Vec<(i64, usize)> = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let Some(score) = candidate_score(candidate, &normalized_query) else {
            continue;
        };
        let scored = (score, index);

        if let Some(worst) = best.last() {
            if best.len() >= effective_limit && compare_scored(&scored, worst) != std::cmp::Ordering::Less {
                continue;
            }
        }

        // 二分找第一个「严格大于 scored」的位置；等于末尾时表示应当追加。
        let mut low = 0usize;
        let mut high = best.len();
        while low < high {
            let mid = (low + high) / 2;
            if compare_scored(&scored, &best[mid]) == std::cmp::Ordering::Less {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        if low == best.len() {
            if best.len() < effective_limit {
                best.push(scored);
            }
            continue;
        }
        best.insert(low, scored);
        if best.len() > effective_limit {
            best.pop();
        }
    }

    best.into_iter()
        .map(|(_, index)| candidates[index].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, rel: &str, ty: EntryType) -> WorkspaceFileEntry {
        WorkspaceFileEntry {
            name: name.into(),
            path: format!("/ws/{rel}"),
            relative_path: rel.into(),
            r#type: ty,
        }
    }

    #[test]
    fn fuzzy_scoring_matches_ts_buckets() {
        assert_eq!(score_workspace_file_fuzzy_match("src/main.ts", "src"), Some(8));
        assert_eq!(score_workspace_file_fuzzy_match("src/main.ts", "main"), Some(104));
        assert_eq!(score_workspace_file_fuzzy_match("src/main.ts", "smt"), Some(215));
        assert_eq!(score_workspace_file_fuzzy_match("src/main.ts", "zzz"), None);
        assert_eq!(score_workspace_file_fuzzy_match("", "x"), None);
        assert_eq!(score_workspace_file_fuzzy_match("x", ""), Some(0));
    }

    #[test]
    fn empty_query_orders_files_before_directories_and_keeps_input_order() {
        let entries = vec![
            entry("b.ts", "src/b.ts", EntryType::File),
            entry("src", "src", EntryType::Directory),
            entry("a.ts", "a.ts", EntryType::File),
        ];
        let candidates = map_entries_to_candidates(&entries);
        let filtered = filter_workspace_file_search_candidates(&candidates, "  ", Some(10), false);
        let paths: Vec<&str> = filtered
            .iter()
            .map(|candidate| candidate.relative_path.as_str())
            .collect();
        // 默认优先级：文件 0、目录 1，升序 → 文件在前，目录在后。
        assert_eq!(paths, vec!["src/b.ts", "a.ts", "src"]);
    }

    #[test]
    fn limit_zero_and_require_query() {
        let candidates = map_entries_to_candidates(&[entry("a.ts", "a.ts", EntryType::File)]);
        assert!(filter_workspace_file_search_candidates(&candidates, "a", Some(0), false).is_empty());
        assert!(
            filter_workspace_file_search_candidates(&candidates, "", Some(10), true).is_empty()
        );
    }

    #[test]
    fn best_matches_are_ordered_by_score() {
        let entries = vec![
            entry("main.ts", "app/main.ts", EntryType::File),
            entry("main", "main", EntryType::Directory),
            entry("main2.ts", "main2.ts", EntryType::File),
        ];
        let candidates = map_entries_to_candidates(&entries);
        let filtered = filter_workspace_file_search_candidates(&candidates, "main", Some(10), false);
        assert_eq!(filtered[0].relative_path, "main", "前缀命中分数最低");
    }
}
