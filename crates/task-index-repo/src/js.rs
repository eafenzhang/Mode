//! ECMAScript 兼容辅助：空白集合、trim、UTF-16 切片、搜索 snippet。
//!
//! 任务索引的列表/搜索语义直接依赖 JS 字符串行为：`String.prototype.trim`、
//! `\s+` 归一化、`slice` 的 UTF-16 码元口径，以及 `Array.prototype.sort` 前的
//! 去空白比较。这里显式复刻，避免 Rust 同名 API 的近似。

/// ECMAScript `WhiteSpace` + `LineTerminator`（与 `tasks-db::js_compat` 同集合）。
pub(crate) fn is_js_whitespace(value: char) -> bool {
    matches!(
        value,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

pub(crate) fn js_trim(value: &str) -> &str {
    value.trim_matches(is_js_whitespace)
}

/// `String.prototype.toLocaleLowerCase()` 的默认（无 locale）行为。
///
/// Rust 的 `to_lowercase` 使用 Unicode 默认大小写映射，与 JS 默认一致；
/// 宿主 locale 相关的特例（如土耳其语）不在复刻范围。
pub(crate) fn js_lowercase(value: &str) -> String {
    value.to_lowercase()
}

pub(crate) fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// JS `String.prototype.slice(0, max)`（按 UTF-16 码元截断）。
pub(crate) fn truncate_utf16(value: &str, max: usize) -> String {
    let units: Vec<u16> = value.encode_utf16().collect();
    if units.len() <= max {
        return value.to_owned();
    }
    String::from_utf16_lossy(&units[..max])
}

/// JS `String.prototype.slice(start, end)`（按 UTF-16 码元）。
pub(crate) fn slice_utf16(value: &str, start: usize, end: usize) -> String {
    let units: Vec<u16> = value.encode_utf16().collect();
    let start = start.min(units.len());
    let end = end.min(units.len()).max(start);
    String::from_utf16_lossy(&units[start..end])
}

/// `String.prototype.indexOf(needle, fromIndex)`，单位是 UTF-16 码元。
fn index_of_utf16(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(haystack.len()));
    }
    if from > haystack.len() || needle.len() > haystack.len() - from {
        return None;
    }
    (from..=haystack.len() - needle.len()).find(|&index| &haystack[index..index + needle.len()] == needle)
}

const TASK_SEARCH_SNIPPET_MAX_CHARS: usize = 140;
const TASK_SEARCH_SNIPPET_LIMIT: usize = 4;
const TASK_SEARCH_SNIPPET_PREFIX_RADIUS: usize = 20;
const TASK_SEARCH_SNIPPET_SUFFIX_RADIUS: usize = 72;

/// `String(searchableText).replace(/\s+/g, " ").trim().slice(0, 140)`。
pub(crate) fn normalize_search_snippet_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut pending_space = false;
    for value in text.chars() {
        if is_js_whitespace(value) {
            pending_space = !normalized.is_empty();
            continue;
        }
        if pending_space {
            normalized.push(' ');
            pending_space = false;
        }
        normalized.push(value);
    }
    truncate_utf16(js_trim(&normalized), TASK_SEARCH_SNIPPET_MAX_CHARS)
}

/// 以命中点为中心截窗，去重相近窗口，最多 4 条；全未命中时回退整段摘要。
pub(crate) fn build_search_snippets(searchable_text: &str, search: Option<&str>) -> Vec<String> {
    let Some(search) = search else {
        return Vec::new();
    };
    if search.is_empty() || js_trim(searchable_text).is_empty() {
        return Vec::new();
    }

    let normalized_search = js_lowercase(search);
    let normalized_text = js_lowercase(searchable_text);
    let search_units: Vec<u16> = normalized_search.encode_utf16().collect();
    let text_units: Vec<u16> = normalized_text.encode_utf16().collect();
    let original_len = utf16_len(searchable_text);

    let mut snippets: Vec<String> = Vec::new();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut search_start = 0usize;

    while snippets.len() < TASK_SEARCH_SNIPPET_LIMIT && search_start < text_units.len() {
        let Some(match_index) = index_of_utf16(&text_units, &search_units, search_start) else {
            break;
        };

        let start = match_index.saturating_sub(TASK_SEARCH_SNIPPET_PREFIX_RADIUS);
        let end = (match_index + search_units.len() + TASK_SEARCH_SNIPPET_SUFFIX_RADIUS).min(original_len);
        let prefix = if start > 0 { "..." } else { "" };
        let suffix = if end < original_len { "..." } else { "" };
        let raw = format!("{prefix}{}{suffix}", slice_utf16(searchable_text, start, end));
        let snippet = normalize_search_snippet_text(&raw);
        let overlaps = ranges
            .iter()
            .any(|&(existing_start, existing_end)| existing_end.min(end) > existing_start.max(start));
        if !snippet.is_empty() && !overlaps {
            snippets.push(snippet);
            ranges.push((start, end));
        }
        search_start = match_index + search_units.len();
    }

    if snippets.is_empty() {
        let fallback = normalize_search_snippet_text(searchable_text);
        return if fallback.is_empty() {
            Vec::new()
        } else {
            vec![fallback]
        };
    }
    snippets
}

/// `resolveWorkspaceKey`：identity 去空白后非空则取 identity，否则取路径。
pub(crate) fn resolve_workspace_key(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    match workspace_identity.map(js_trim) {
        Some(identity) if !identity.is_empty() => identity.to_owned(),
        _ => workspace_path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_prefix_and_suffix_follow_js_windows() {
        let text = "a".repeat(30) + "NEEDLE" + &"b".repeat(100);
        let snippets = build_search_snippets(&text, Some("needle"));
        assert_eq!(snippets.len(), 1);
        assert!(snippets[0].starts_with("..."));
        assert!(snippets[0].ends_with("..."));
        assert!(snippets[0].contains("NEEDLE"));
        assert!(utf16_len(&snippets[0]) <= TASK_SEARCH_SNIPPET_MAX_CHARS);
    }

    #[test]
    fn snippets_dedupe_overlapping_windows() {
        let text = "needle needle needle";
        let snippets = build_search_snippets(text, Some("needle"));
        assert_eq!(snippets.len(), 1);
    }

    #[test]
    fn snippet_falls_back_when_only_title_matches() {
        let snippets = build_search_snippets("plain body", Some("title-only"));
        assert_eq!(snippets, vec!["plain body".to_owned()]);
    }

    #[test]
    fn empty_search_has_no_snippets() {
        assert!(build_search_snippets("body", Some("")).is_empty());
        assert!(build_search_snippets("body", None).is_empty());
        assert!(build_search_snippets("   ", Some("x")).is_empty());
    }

    #[test]
    fn truncation_counts_utf16_units() {
        let text = "😀".repeat(10); // 每个 emoji 2 个 UTF-16 码元
        let truncated = truncate_utf16(&text, 5);
        // 5 个码元 = 2 个完整 emoji + 半个代理项（lossy 成 U+FFFD）。
        assert_eq!(utf16_len(&truncated), 5);
    }
}
