//! Node `fs` / `path` / 排序语义的最小复刻。
//!
//! 只覆盖宿主服务用到的部分；与 TS 的差异集中记录在对应服务的 spec 里。

use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};

/// JS `\s` 字符集（比 Rust `char::is_whitespace` 多 U+FEFF）。NFKC 归一不参与。
pub fn is_js_space(value: char) -> bool {
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
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// JS `String.prototype.trim()`。
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

/// JS `String.prototype.trimEnd()`。
pub fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(is_js_space)
}

/// Node `access(path)`：悬空软链（目标不存在）为 false。
pub fn exists(path: &Path) -> bool {
    path.exists()
}

/// Node `readFile(path, "utf-8")`：非法 UTF-8 以 U+FFFD 替换。
pub fn read_text(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Node `realpath`：Windows 的 verbatim 前缀必须剥掉，否则与 TS 生成的路径不相等。
pub fn realpath(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok().map(strip_verbatim)
}

fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path
}

/// Node `path.isAbsolute`（Windows：盘符绝对路径、UNC、单个前导分隔符都算绝对）。
pub fn is_absolute(path: &str) -> bool {
    #[cfg(windows)]
    {
        let bytes = path.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return true;
        }
        if path.starts_with("\\\\") {
            return true;
        }
        path.starts_with('\\') || path.starts_with('/')
    }
    #[cfg(not(windows))]
    {
        path.starts_with('/')
    }
}

/// 与 `path.resolve(root, raw)` 等价（两个参数都按 Node 的规范化规则处理）。
pub fn resolve_join(root: &Path, raw: &Path) -> PathBuf {
    normalize_lexical(&root.join(raw))
}

/// 词法规范化：解析 `.` / `..`，不触碰文件系统（Node `path.resolve` 的口径）。
pub fn normalize_lexical(path: &Path) -> PathBuf {
    let mut parts: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match parts.last() {
                Some(Component::Normal(_)) => {
                    parts.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => parts.push(component),
            },
            other => parts.push(other),
        }
    }
    parts.into_iter().collect()
}

fn comparable_parts(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| {
            let text = component.as_os_str().to_string_lossy().into_owned();
            if cfg!(windows) {
                text.to_lowercase()
            } else {
                text
            }
        })
        .collect()
}

/// `path.relative(root, candidate)` 的越界判定：候选等于根或位于根内。
/// Windows 上与 Node 一致地不区分大小写。
pub fn contains_path(root: &Path, candidate: &Path) -> bool {
    let root_parts = comparable_parts(root);
    let candidate_parts = comparable_parts(candidate);
    candidate_parts.len() >= root_parts.len()
        && root_parts
            .iter()
            .zip(candidate_parts.iter())
            .all(|(left, right)| left == right)
}

/// `localeCompare` 的 ASCII 近似：先比小写折叠序列，再比大小写次序（小写在前）。
pub fn locale_compare(left: &str, right: &str) -> Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| case_rank(left).cmp(&case_rank(right)))
}

fn case_rank(text: &str) -> Vec<u8> {
    text.chars()
        .map(|value| u8::from(value.is_uppercase()))
        .collect()
}

/// `localeCompare(other, undefined, { numeric: true, sensitivity: "base" })` 的近似：
/// 数字串按数值比较，其余按小写折叠比较。
pub fn locale_compare_numeric(left: &str, right: &str) -> Ordering {
    let left_chunks = split_numeric_chunks(left);
    let right_chunks = split_numeric_chunks(right);
    for (left_chunk, right_chunk) in left_chunks.iter().zip(right_chunks.iter()) {
        let ordering = match (left_chunk, right_chunk) {
            (Chunk::Digits(left), Chunk::Digits(right)) => compare_digits(left, right),
            (Chunk::Text(left), Chunk::Text(right)) => left.cmp(right),
            // ICU 数字排序里数字整体先于字母。
            (Chunk::Digits(_), Chunk::Text(_)) => Ordering::Less,
            (Chunk::Text(_), Chunk::Digits(_)) => Ordering::Greater,
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left_chunks.len().cmp(&right_chunks.len())
}

/// 降序包装：`right.localeCompare(left, …, { numeric: true })`。
pub fn locale_compare_numeric_desc(left: &str, right: &str) -> Ordering {
    locale_compare_numeric(right, left)
}

enum Chunk {
    Digits(String),
    Text(String),
}

fn split_numeric_chunks(text: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_is_digit = false;
    for character in text.to_lowercase().chars() {
        let is_digit = character.is_ascii_digit();
        if current.is_empty() {
            current_is_digit = is_digit;
            current.push(character);
            continue;
        }
        if is_digit == current_is_digit {
            current.push(character);
        } else {
            chunks.push(finish_chunk(std::mem::take(&mut current), current_is_digit));
            current_is_digit = is_digit;
            current.push(character);
        }
    }
    if !current.is_empty() {
        chunks.push(finish_chunk(current, current_is_digit));
    }
    chunks
}

fn finish_chunk(value: String, is_digit: bool) -> Chunk {
    if is_digit {
        Chunk::Digits(value)
    } else {
        Chunk::Text(value)
    }
}

fn compare_digits(left: &str, right: &str) -> Ordering {
    let left_trimmed = left.trim_start_matches('0');
    let right_trimmed = right.trim_start_matches('0');
    left_trimmed
        .len()
        .cmp(&right_trimmed.len())
        .then_with(|| left_trimmed.cmp(right_trimmed))
}

/// Node `os.homedir()`：Windows 先 `USERPROFILE`，再 `HOMEDRIVE` + `HOMEPATH`；Unix 用 `HOME`。
pub fn node_homedir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(profile) = non_empty_env("USERPROFILE") {
            return PathBuf::from(profile);
        }
        if let (Some(drive), Some(home_path)) =
            (non_empty_env("HOMEDRIVE"), non_empty_env("HOMEPATH"))
        {
            return PathBuf::from(format!("{drive}{home_path}"));
        }
    }
    non_empty_env("HOME").map(PathBuf::from).unwrap_or_default()
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_trim_covers_bom() {
        assert_eq!(js_trim("\u{FEFF} x \u{FEFF}"), "x");
        assert_eq!(js_trim_end("x\n  "), "x");
    }

    #[test]
    fn lexical_normalization_resolves_dot_segments() {
        let root = Path::new("/a/b");
        assert_eq!(resolve_join(root, Path::new("c/../d")), Path::new("/a/b/d"));
        assert_eq!(resolve_join(root, Path::new("../d")), Path::new("/a/d"));
    }

    #[test]
    fn containment_is_prefix_based() {
        assert!(contains_path(Path::new("/a/b"), Path::new("/a/b")));
        assert!(contains_path(Path::new("/a/b"), Path::new("/a/b/c")));
        assert!(!contains_path(Path::new("/a/b"), Path::new("/a/bc")));
        assert!(!contains_path(Path::new("/a/b"), Path::new("/a")));
    }

    #[test]
    fn locale_compare_orders_lowercase_first() {
        assert_eq!(locale_compare("alpha", "Beta"), Ordering::Less);
        assert_eq!(locale_compare("a", "A"), Ordering::Less);
        assert_eq!(locale_compare("10", "9"), Ordering::Less);
    }

    #[test]
    fn numeric_compare_handles_versions() {
        assert_eq!(locale_compare_numeric("2.0.0", "10.0.0"), Ordering::Less);
        assert_eq!(locale_compare_numeric("10.0.0", "9.0.0"), Ordering::Greater);
        assert_eq!(locale_compare_numeric("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(
            locale_compare_numeric("1.0.0-beta", "1.0.0"),
            Ordering::Greater
        );
    }
}
