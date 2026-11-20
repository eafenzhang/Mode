//! 路径安全辅助：只允许单段标识符，并校验拼接结果仍位于根目录内。
//!
//! 用于附件/插件等把调用方字符串拼进落盘路径的入口；`is_safe_segment` 拒绝空串、
//! `.`/`..`、路径分隔符和平台保留字符，`is_within` 用词法规范化后做前缀校验。

use std::path::{Component, Path, PathBuf};

/// 单个路径段：非空、≤128 字符、只含 `[A-Za-z0-9._+-]`，且不是 `.`/`..`。
///
/// 拒绝路径分隔符和 `:`（Windows 前缀）、控制字符；`+` 只为兼容 semver 构建元数据。
pub fn is_safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+'))
}

/// 词法规范化（不触盘）：消掉 `.`/`..`，保留绝对/相对形态。
fn normalize(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

/// `target` 是否位于 `root` 内（词法包含；不要求目标已存在）。
pub fn is_within(root: &Path, target: &Path) -> bool {
    let root = normalize(root);
    let target = normalize(target);
    target.starts_with(&root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_reject_traversal_and_separators() {
        assert!(is_safe_segment("sess_abc-1.2"));
        assert!(!is_safe_segment(""));
        assert!(!is_safe_segment("."));
        assert!(!is_safe_segment(".."));
        assert!(!is_safe_segment("../x"));
        assert!(!is_safe_segment("a/b"));
        assert!(!is_safe_segment("a\\b"));
        assert!(!is_safe_segment("C:"));
    }

    #[test]
    fn containment_normalizes_traversal() {
        let root = Path::new("C:/data/cli/artifacts");
        assert!(is_within(
            root,
            Path::new("C:/data/cli/artifacts/sess/out.txt")
        ));
        assert!(!is_within(
            root,
            Path::new("C:/data/cli/artifacts/../../Windows/out.txt")
        ));
        assert!(!is_within(
            root,
            Path::new("C:/data/cli/artifacts-other/out.txt")
        ));
    }
}
